use std::path::PathBuf;

use hpsvm::{HPSVM, Inspector};
use solana_account::{Account, ReadableAccount};
use solana_address::{Address, address};
use solana_compute_budget::compute_budget::ComputeBudget;
use solana_instruction::{Instruction, error::InstructionError};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::Transaction;
use solana_transaction_error::TransactionError;

type VmConfigMutator = fn(&mut HPSVM);

struct SwapInspector;

impl Inspector for SwapInspector {}

fn with_reduced_compute_budget(svm: &mut HPSVM) {
    let mut compute_budget = ComputeBudget::new_with_defaults(false, false);
    compute_budget.compute_unit_limit = 10;
    svm.set_compute_budget(compute_budget);
}

fn without_sigverify(svm: &mut HPSVM) {
    svm.set_sigverify(false);
}

fn without_blockhash_check(svm: &mut HPSVM) {
    svm.set_blockhash_check(false);
}

fn with_shorter_log_limit(svm: &mut HPSVM) {
    svm.set_log_bytes_limit(Some(32));
}

#[test]
fn transact_returns_state_without_committing_it() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );

    let outcome = svm.transact(tx);

    assert!(outcome.status().is_ok());
    assert_eq!(svm.get_balance(&recipient), None);
    assert!(
        outcome
            .post_accounts()
            .iter()
            .any(|(key, account)| key == &recipient && account.lamports() == 64)
    );
}

#[test]
fn commit_transaction_applies_a_transacted_outcome() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );

    let outcome = svm.transact(tx);
    let result = svm.commit_transaction(outcome);

    assert!(result.is_ok());
    assert_eq!(svm.get_balance(&recipient), Some(64));
}

#[test]
fn commit_transaction_charges_failed_transaction_fees_for_transacted_outcomes() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let program_id = address!("HvrRMSshMx3itvsyWDnWg2E3cy5h57iMaR7oVxSZJDSA");
    let mut so_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    so_path.push("test_programs/target/deploy/failure.so");
    svm.add_program_from_file(program_id, &so_path).unwrap();

    let initial_balance = 1_000_000_000;
    svm.airdrop(&payer.pubkey(), initial_balance).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(
            &[Instruction { program_id, accounts: vec![], data: vec![] }],
            Some(&payer.pubkey()),
        ),
        svm.latest_blockhash(),
    );
    let signature = tx.signatures[0];

    let outcome = svm.transact(tx);

    assert!(outcome.status().is_err());
    assert_eq!(svm.get_balance(&payer.pubkey()), Some(initial_balance));

    let result = svm.commit_transaction(outcome);

    assert_eq!(
        result.unwrap_err().err,
        TransactionError::InstructionError(0, InstructionError::Custom(0))
    );
    assert_eq!(svm.get_balance(&payer.pubkey()), Some(initial_balance - 5000));
    assert!(svm.get_transaction(&signature).unwrap().is_err());
}

#[test]
fn commit_transaction_rejects_stale_outcomes_after_same_vm_mutation() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );
    let signature = tx.signatures[0];

    let outcome = svm.transact(tx);
    svm.set_account(
        recipient,
        Account {
            lamports: 777,
            owner: solana_sdk_ids::system_program::id(),
            ..Default::default()
        },
    )
    .unwrap();

    let result = svm.commit_transaction(outcome);

    assert_eq!(result.unwrap_err().err, TransactionError::ResanitizationNeeded);
    assert_eq!(svm.get_balance(&payer.pubkey()), Some(10_000));
    assert_eq!(svm.get_balance(&recipient), Some(777));
    assert!(svm.get_transaction(&signature).is_none());
}

#[test]
fn commit_transaction_rejects_stale_outcomes_after_vm_config_mutation() {
    let cases: [(&str, VmConfigMutator); 4] = [
        ("compute budget", with_reduced_compute_budget),
        ("sigverify", without_sigverify),
        ("blockhash check", without_blockhash_check),
        ("log bytes limit", with_shorter_log_limit),
    ];

    for (label, mutate) in cases {
        let mut svm = HPSVM::new();
        let payer = Keypair::new();
        let recipient = Address::new_unique();

        svm.airdrop(&payer.pubkey(), 10_000).unwrap();
        let tx = Transaction::new(
            &[&payer],
            Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
            svm.latest_blockhash(),
        );
        let signature = tx.signatures[0];

        let outcome = svm.transact(tx);
        mutate(&mut svm);

        let result = svm.commit_transaction(outcome);

        assert_eq!(
            result.unwrap_err().err,
            TransactionError::ResanitizationNeeded,
            "{label} should invalidate transacted outcomes"
        );
        assert_eq!(
            svm.get_balance(&payer.pubkey()),
            Some(10_000),
            "{label} should leave the payer balance unchanged"
        );
        assert_eq!(svm.get_balance(&recipient), None, "{label} should not commit the post-state");
        assert!(
            svm.get_transaction(&signature).is_none(),
            "{label} should not record the stale outcome"
        );
    }
}

#[test]
fn commit_transaction_rejects_stale_outcomes_after_inspector_reconfiguration() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );
    let signature = tx.signatures[0];

    let outcome = svm.transact(tx);
    svm = svm.with_inspector(SwapInspector);

    let result = svm.commit_transaction(outcome);

    assert_eq!(result.unwrap_err().err, TransactionError::ResanitizationNeeded);
    assert_eq!(svm.get_balance(&payer.pubkey()), Some(10_000));
    assert_eq!(svm.get_balance(&recipient), None);
    assert!(svm.get_transaction(&signature).is_none());
}

#[cfg(feature = "nodejs-internal")]
#[test]
fn commit_transaction_rejects_stale_outcomes_after_direct_nodejs_internal_setter_mutation() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );

    let outcome = svm.transact(tx);
    svm.set_lamports(2_000_000);

    let result = svm.commit_transaction(outcome);

    assert_eq!(result.unwrap_err().err, TransactionError::ResanitizationNeeded);
    assert_eq!(svm.get_balance(&payer.pubkey()), Some(10_000));
    assert_eq!(svm.get_balance(&recipient), None);
}

#[test]
fn commit_transaction_rejects_stale_outcomes_after_mutating_send_transaction_batch() {
    let mut svm = HPSVM::new();
    let transacted_payer = Keypair::new();
    let batch_payer_a = Keypair::new();
    let batch_payer_b = Keypair::new();
    let transacted_recipient = Address::new_unique();
    let batch_recipient_a = Address::new_unique();
    let batch_recipient_b = Address::new_unique();

    svm.airdrop(&transacted_payer.pubkey(), 10_000).unwrap();
    svm.airdrop(&batch_payer_a.pubkey(), 10_000).unwrap();
    svm.airdrop(&batch_payer_b.pubkey(), 10_000).unwrap();

    let blockhash = svm.latest_blockhash();
    let transacted_tx = Transaction::new(
        &[&transacted_payer],
        Message::new(
            &[transfer(&transacted_payer.pubkey(), &transacted_recipient, 64)],
            Some(&transacted_payer.pubkey()),
        ),
        blockhash,
    );
    let transacted_signature = transacted_tx.signatures[0];
    let outcome = svm.transact(transacted_tx);

    let batch_tx_a = Transaction::new(
        &[&batch_payer_a],
        Message::new(
            &[transfer(&batch_payer_a.pubkey(), &batch_recipient_a, 32)],
            Some(&batch_payer_a.pubkey()),
        ),
        blockhash,
    );
    let batch_tx_b = Transaction::new(
        &[&batch_payer_b],
        Message::new(
            &[transfer(&batch_payer_b.pubkey(), &batch_recipient_b, 16)],
            Some(&batch_payer_b.pubkey()),
        ),
        blockhash,
    );

    let batch = svm
        .send_transaction_batch([batch_tx_a, batch_tx_b])
        .expect("multi-transaction batch should execute successfully");

    assert_eq!(batch.plan.stages.len(), 1);
    assert_eq!(batch.plan.stages[0].transaction_indexes, vec![0, 1]);
    assert_eq!(batch.results.len(), 2);
    assert!(batch.results[0].is_ok());
    assert!(batch.results[1].is_ok());
    assert_eq!(svm.get_balance(&batch_recipient_a), Some(32));
    assert_eq!(svm.get_balance(&batch_recipient_b), Some(16));

    let result = svm.commit_transaction(outcome);

    assert_eq!(result.unwrap_err().err, TransactionError::ResanitizationNeeded);
    assert_eq!(svm.get_balance(&transacted_recipient), None);
    assert!(svm.get_transaction(&transacted_signature).is_none());
}

#[test]
fn commit_transaction_rejects_outcomes_from_a_different_vm_instance() {
    let payer = Keypair::new();
    let recipient = Address::new_unique();
    let mut origin_svm = HPSVM::new();
    let mut foreign_svm = HPSVM::new();

    origin_svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    foreign_svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        origin_svm.latest_blockhash(),
    );
    let signature = tx.signatures[0];

    let outcome = origin_svm.transact(tx);
    let result = foreign_svm.commit_transaction(outcome);

    assert_eq!(result.unwrap_err().err, TransactionError::ResanitizationNeeded);
    assert_eq!(foreign_svm.get_balance(&payer.pubkey()), Some(10_000));
    assert_eq!(foreign_svm.get_balance(&recipient), None);
    assert!(foreign_svm.get_transaction(&signature).is_none());
}
