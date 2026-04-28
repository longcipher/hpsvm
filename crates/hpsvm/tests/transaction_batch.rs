use std::path::PathBuf;
#[cfg(feature = "invocation-inspect-callback")]
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
#[cfg(feature = "invocation-inspect-callback")]
use std::thread;
#[cfg(feature = "invocation-inspect-callback")]
use std::time::{Duration, Instant};

use hpsvm::HPSVM;
#[cfg(feature = "invocation-inspect-callback")]
use hpsvm::InvocationInspectCallback;
use solana_account::Account;
use solana_address::{Address, address};
use solana_address_lookup_table_interface::instruction::{
    create_lookup_table, deactivate_lookup_table, extend_lookup_table,
};
use solana_hash::Hash;
use solana_instruction::{Instruction, account_meta::AccountMeta, error::InstructionError};
use solana_keypair::Keypair;
use solana_message::{
    AddressLookupTableAccount, Message, VersionedMessage, v0::Message as MessageV0,
};
#[cfg(feature = "invocation-inspect-callback")]
use solana_program_runtime::invoke_context::InvokeContext;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
#[cfg(feature = "invocation-inspect-callback")]
use solana_transaction::sanitized::SanitizedTransaction;
use solana_transaction::{Transaction, versioned::VersionedTransaction};
#[cfg(feature = "invocation-inspect-callback")]
use solana_transaction_context::IndexOfAccount;
use solana_transaction_error::TransactionError;

fn transfer_tx(
    payer: &Keypair,
    recipient: &Address,
    lamports: u64,
    blockhash: Hash,
) -> Transaction {
    Transaction::new(
        &[payer],
        Message::new(&[transfer(&payer.pubkey(), recipient, lamports)], Some(&payer.pubkey())),
        blockhash,
    )
}

fn read_counter_program() -> Vec<u8> {
    let mut so_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    so_path.push("test_programs/target/deploy/counter.so");
    std::fs::read(so_path).unwrap()
}

fn read_failure_program() -> Vec<u8> {
    let mut so_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    so_path.push("test_programs/target/deploy/failure.so");
    std::fs::read(so_path).unwrap()
}

#[test]
fn transaction_batch_plan_groups_non_conflicting_transactions() {
    let mut svm = HPSVM::new();
    let payer_a = Keypair::new();
    let payer_b = Keypair::new();
    let blockhash = svm.latest_blockhash();

    svm.airdrop(&payer_a.pubkey(), 1_000_000_000).unwrap();
    svm.airdrop(&payer_b.pubkey(), 1_000_000_000).unwrap();

    let tx_a1 = transfer_tx(&payer_a, &Address::new_unique(), 10, blockhash);
    let tx_b = transfer_tx(&payer_b, &Address::new_unique(), 20, blockhash);
    let tx_a2 = transfer_tx(&payer_a, &Address::new_unique(), 30, blockhash);

    let plan = svm.plan_transaction_batch([tx_a1, tx_b, tx_a2]).expect("batch plan should succeed");

    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0].transaction_indexes, vec![0, 1]);
    assert_eq!(plan.stages[1].transaction_indexes, vec![2]);
}

#[test]
fn send_transaction_batch_executes_transactions_and_returns_results_in_input_order() {
    let mut svm = HPSVM::new();
    let payer_a = Keypair::new();
    let payer_b = Keypair::new();
    let recipient_a = Address::new_unique();
    let recipient_b = Address::new_unique();

    svm.airdrop(&payer_a.pubkey(), 1_000_000_000).unwrap();
    svm.airdrop(&payer_b.pubkey(), 1_000_000_000).unwrap();

    let blockhash = svm.latest_blockhash();
    let tx_a = transfer_tx(&payer_a, &recipient_a, 10, blockhash);
    let tx_b = transfer_tx(&payer_b, &recipient_b, 20, blockhash);

    let batch =
        svm.send_transaction_batch([tx_a, tx_b]).expect("batch execution plan should succeed");

    assert_eq!(batch.plan.stages.len(), 1);
    assert_eq!(batch.plan.stages[0].transaction_indexes, vec![0, 1]);
    assert_eq!(batch.results.len(), 2);
    assert!(batch.results.iter().all(Result::is_ok));
    assert_eq!(svm.get_balance(&recipient_a), Some(10));
    assert_eq!(svm.get_balance(&recipient_b), Some(20));
}

#[test]
fn send_transaction_batch_merges_stage_deltas_before_following_stages() {
    let mut svm = HPSVM::new();
    let payer_a = Keypair::new();
    let payer_b = Keypair::new();
    let recipient_a = Address::new_unique();
    let recipient_b = Address::new_unique();
    let recipient_c = Address::new_unique();

    svm.airdrop(&payer_a.pubkey(), 25_000).unwrap();
    svm.airdrop(&payer_b.pubkey(), 1_000_000_000).unwrap();

    let blockhash = svm.latest_blockhash();
    let tx_a1 = transfer_tx(&payer_a, &recipient_a, 10_000, blockhash);
    let tx_b = transfer_tx(&payer_b, &recipient_b, 1_000, blockhash);
    let tx_a2 = transfer_tx(&payer_a, &recipient_c, 10_000, blockhash);

    let batch =
        svm.send_transaction_batch([tx_a1, tx_b, tx_a2]).expect("batch execution should succeed");

    assert_eq!(batch.plan.stages.len(), 2);
    assert_eq!(batch.plan.stages[0].transaction_indexes, vec![0, 1]);
    assert_eq!(batch.plan.stages[1].transaction_indexes, vec![2]);
    assert!(batch.results[0].is_ok());
    assert!(batch.results[1].is_ok());
    assert!(batch.results[2].is_err());
    assert_eq!(svm.get_balance(&recipient_a), Some(10_000));
    assert_eq!(svm.get_balance(&recipient_b), Some(1_000));
    assert_eq!(svm.get_balance(&recipient_c), None);
}

#[test]
fn send_transaction_batch_preserves_mixed_success_and_failure_semantics_within_stage() {
    let mut svm = HPSVM::new();
    let failing_payer = Keypair::new();
    let successful_payer = Keypair::new();
    let recipient = Address::new_unique();
    let failure_program_id = address!("HvrRMSshMx3itvsyWDnWg2E3cy5h57iMaR7oVxSZJDSA");
    let initial_balance = 1_000_000_000;

    svm.add_program(failure_program_id, &read_failure_program()).unwrap();
    svm.airdrop(&failing_payer.pubkey(), initial_balance).unwrap();
    svm.airdrop(&successful_payer.pubkey(), initial_balance).unwrap();

    let blockhash = svm.latest_blockhash();
    let failed_tx = Transaction::new(
        &[&failing_payer],
        Message::new(
            &[Instruction { program_id: failure_program_id, accounts: vec![], data: vec![] }],
            Some(&failing_payer.pubkey()),
        ),
        blockhash,
    );
    let failed_signature = failed_tx.signatures[0];

    let successful_tx = transfer_tx(&successful_payer, &recipient, 25, blockhash);
    let successful_signature = successful_tx.signatures[0];

    let batch = svm
        .send_transaction_batch([failed_tx, successful_tx])
        .expect("batch execution should succeed");

    assert_eq!(batch.plan.stages.len(), 1);
    assert!(batch.results[0].is_err());
    assert!(batch.results[1].is_ok());
    assert_eq!(
        batch.results[0].as_ref().unwrap_err().err,
        TransactionError::InstructionError(0, InstructionError::Custom(0))
    );
    assert_eq!(svm.get_balance(&failing_payer.pubkey()), Some(initial_balance - 5000));
    assert_eq!(svm.get_balance(&successful_payer.pubkey()), Some(initial_balance - 5000 - 25));
    assert_eq!(svm.get_balance(&recipient), Some(25));
    assert!(svm.get_transaction(&failed_signature).unwrap().is_err());
    assert!(svm.get_transaction(&successful_signature).unwrap().is_ok());
}

#[test]
fn send_transaction_batch_plans_lookup_table_updates_before_lookup_users() {
    let mut svm = HPSVM::new();
    let authority = Keypair::new();
    let lookup_user = Keypair::new();
    let authority_pk = authority.pubkey();
    let lookup_user_pk = lookup_user.pubkey();
    let program_id = address!("GtdambwDgHWrDJdVPBkEHGhCwokqgAoch162teUjJse2");
    let counter_address = address!("J39wvrFY2AkoAUCke5347RMNk3ditxZfVidoZ7U6Fguf");

    svm.add_program(program_id, &read_counter_program()).unwrap();
    svm.airdrop(&authority_pk, 1_000_000_000).unwrap();
    svm.airdrop(&lookup_user_pk, 1_000_000_000).unwrap();
    svm.set_account(
        counter_address,
        Account {
            lamports: 5,
            data: vec![0_u8; std::mem::size_of::<u32>()],
            owner: program_id,
            ..Default::default()
        },
    )
    .unwrap();

    let setup_blockhash = svm.latest_blockhash();
    let (create_lookup_ix, lookup_table_address) =
        create_lookup_table(authority_pk, authority_pk, 0);
    let extend_lookup_ix = extend_lookup_table(
        lookup_table_address,
        authority_pk,
        Some(authority_pk),
        vec![counter_address],
    );
    let setup_lookup_tx = Transaction::new(
        &[&authority],
        Message::new(&[create_lookup_ix, extend_lookup_ix], Some(&authority_pk)),
        setup_blockhash,
    );
    svm.send_transaction(setup_lookup_tx).unwrap();
    svm.warp_to_slot(1);

    let batch_blockhash = svm.latest_blockhash();
    let deactivate_tx = Transaction::new(
        &[&authority],
        Message::new(
            &[deactivate_lookup_table(lookup_table_address, authority_pk)],
            Some(&authority_pk),
        ),
        batch_blockhash,
    );
    let lookup_table =
        AddressLookupTableAccount { key: lookup_table_address, addresses: vec![counter_address] };
    let lookup_message = MessageV0::try_compile(
        &lookup_user_pk,
        &[Instruction {
            program_id,
            accounts: vec![AccountMeta::new(counter_address, false)],
            data: vec![0, 0],
        }],
        &[lookup_table],
        batch_blockhash,
    )
    .unwrap();
    let lookup_tx =
        VersionedTransaction::try_new(VersionedMessage::V0(lookup_message), &[&lookup_user])
            .unwrap();

    let batch = svm.send_transaction_batch(vec![deactivate_tx.into(), lookup_tx]).unwrap();

    assert_eq!(batch.plan.stages.len(), 2);
    assert_eq!(batch.plan.stages[0].transaction_indexes, vec![0]);
    assert_eq!(batch.plan.stages[1].transaction_indexes, vec![1]);
    assert!(batch.results[0].is_ok());
    assert!(batch.results[1].is_ok());
    assert_eq!(svm.get_account(&counter_address).unwrap().data, 1u32.to_le_bytes().to_vec());
}

#[cfg(feature = "invocation-inspect-callback")]
#[test]
fn send_transaction_batch_runs_independent_stage_transactions_in_parallel() {
    struct ConcurrentInvocationCallback {
        active: Arc<AtomicUsize>,
        max_seen: Arc<AtomicUsize>,
    }

    impl InvocationInspectCallback for ConcurrentInvocationCallback {
        fn before_invocation(
            &self,
            _: &HPSVM,
            _: &SanitizedTransaction,
            _: &[IndexOfAccount],
            _: &InvokeContext<'_, '_>,
        ) {
            let current = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_seen.fetch_max(current, Ordering::SeqCst);

            let start = Instant::now();
            while start.elapsed() < Duration::from_millis(50) {
                thread::yield_now();
            }
        }

        fn after_invocation(&self, _: &HPSVM, _: &InvokeContext<'_, '_>, _: bool) {
            self.active.fetch_sub(1, Ordering::SeqCst);
        }
    }

    let active = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));
    let mut svm = HPSVM::new().with_sigverify(true);
    svm.set_invocation_inspect_callback(ConcurrentInvocationCallback {
        active: Arc::clone(&active),
        max_seen: Arc::clone(&max_seen),
    });

    let payer_a = Keypair::new();
    let payer_b = Keypair::new();
    let program_id = address!("GtdambwDgHWrDJdVPBkEHGhCwokqgAoch162teUjJse2");
    let counter_a = address!("J39wvrFY2AkoAUCke5347RMNk3ditxZfVidoZ7U6Fguf");
    let counter_b = Address::new_unique();

    svm.add_program(program_id, &read_counter_program()).unwrap();
    svm.airdrop(&payer_a.pubkey(), 1_000_000_000).unwrap();
    svm.airdrop(&payer_b.pubkey(), 1_000_000_000).unwrap();
    svm.set_account(
        counter_a,
        Account {
            lamports: 5,
            data: vec![0_u8; std::mem::size_of::<u32>()],
            owner: program_id,
            ..Default::default()
        },
    )
    .unwrap();
    svm.set_account(
        counter_b,
        Account {
            lamports: 5,
            data: vec![0_u8; std::mem::size_of::<u32>()],
            owner: program_id,
            ..Default::default()
        },
    )
    .unwrap();

    let blockhash = svm.latest_blockhash();
    let tx_a = Transaction::new(
        &[&payer_a],
        Message::new(
            &[solana_instruction::Instruction {
                program_id,
                accounts: vec![solana_instruction::account_meta::AccountMeta::new(
                    counter_a, false,
                )],
                data: vec![0, 1],
            }],
            Some(&payer_a.pubkey()),
        ),
        blockhash,
    );
    let tx_b = Transaction::new(
        &[&payer_b],
        Message::new(
            &[solana_instruction::Instruction {
                program_id,
                accounts: vec![solana_instruction::account_meta::AccountMeta::new(
                    counter_b, false,
                )],
                data: vec![0, 2],
            }],
            Some(&payer_b.pubkey()),
        ),
        blockhash,
    );

    let batch = svm.send_transaction_batch([tx_a, tx_b]).expect("batch execution should succeed");

    assert!(batch.results.iter().all(Result::is_ok));
    assert!(max_seen.load(Ordering::SeqCst) > 1);
}
