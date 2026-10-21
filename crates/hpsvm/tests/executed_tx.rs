//! `ExecutedTx` commit/discard golden tests.
//!
//! Goldens below were captured from the legacy `transact` +
//! `commit_transaction` path before the migration:
//! success: fee 5000, compute units 150, payer 10_000 -> 4_936, recipient 64;
//! failed-but-included: `InstructionError(0, Custom(1))`, fee 5000,
//! payer 10_000 -> 5_000, no recipient account, history holds the error;
//! rejected pre-execution (`BlockhashNotFound`): fee 0, payer unchanged,
//! no history entry.

use hpsvm::HPSVM;
use solana_address::{Address, address};
use solana_compute_budget::compute_budget::ComputeBudget;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::{InstructionError, Transaction};
use solana_transaction_error::TransactionError;

fn funded_vm(payer: &Keypair, lamports: u64) -> HPSVM {
    let mut svm = HPSVM::new();
    svm.airdrop(&payer.pubkey(), lamports).unwrap();
    svm
}

fn transfer_tx(
    payer: &Keypair,
    recipient: &Address,
    lamports: u64,
    blockhash: solana_hash::Hash,
) -> Transaction {
    Transaction::new(
        &[payer],
        Message::new(&[transfer(&payer.pubkey(), recipient, lamports)], Some(&payer.pubkey())),
        blockhash,
    )
}

#[test]
fn transact_commit_matches_the_legacy_golden_path() {
    let payer = Keypair::new();
    let recipient = Address::new_unique();
    let mut svm = funded_vm(&payer, 10_000);
    let tx = transfer_tx(&payer, &recipient, 64, svm.latest_blockhash());
    let signature = tx.signatures[0];

    let staged = svm.transact(tx).expect("transact should stage a valid transfer");
    let meta = staged.commit().unwrap();

    assert_eq!(meta.fee, 5000);
    assert_eq!(meta.compute_units_consumed, 150);
    assert_eq!(svm.get_balance(&payer.pubkey()), Some(4_936));
    assert_eq!(svm.get_balance(&recipient), Some(64));
    assert!(svm.get_transaction(&signature).unwrap().is_ok());
}

#[test]
fn transact_discard_returns_the_result_without_touching_state() {
    let payer = Keypair::new();
    let recipient = Address::new_unique();
    let mut svm = funded_vm(&payer, 10_000);
    let pre_payer = svm.get_account(&payer.pubkey()).unwrap();
    let tx = transfer_tx(&payer, &recipient, 64, svm.latest_blockhash());
    let signature = tx.signatures[0];

    let staged = svm.transact(tx).expect("transact should stage a valid transfer");
    let meta = staged.discard().unwrap();

    assert_eq!(meta.fee, 5000);
    assert_eq!(meta.compute_units_consumed, 150);
    assert_eq!(svm.get_account(&payer.pubkey()).unwrap(), pre_payer);
    assert_eq!(svm.get_balance(&recipient), None);
    assert!(svm.get_transaction(&signature).is_none());
}

#[test]
fn transact_discard_of_a_failed_transaction_restores_state_byte_identically() {
    let payer = Keypair::new();
    let recipient = Address::new_unique();
    let mut svm = funded_vm(&payer, 10_000);
    let pre_payer = svm.get_account(&payer.pubkey()).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, u64::MAX)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );
    let signature = tx.signatures[0];

    let staged = svm.transact(tx).expect("a failing execution must still stage");
    let err = staged.discard().expect_err("discard must surface the execution failure");

    assert_eq!(err.err, TransactionError::InstructionError(0, InstructionError::Custom(1)));
    assert_eq!(err.meta.fee, 5000);
    assert_eq!(svm.get_account(&payer.pubkey()).unwrap(), pre_payer);
    assert_eq!(svm.get_balance(&recipient), None);
    assert!(svm.get_transaction(&signature).is_none());
}

#[test]
fn transact_commit_of_a_failed_transaction_charges_the_fee_like_the_legacy_path() {
    let payer = Keypair::new();
    let recipient = Address::new_unique();
    let mut svm = funded_vm(&payer, 10_000);
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, u64::MAX)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );
    let signature = tx.signatures[0];

    let staged = svm.transact(tx).expect("a failing execution must still stage");
    let err = staged.commit().expect_err("commit must surface the execution failure");

    assert_eq!(err.err, TransactionError::InstructionError(0, InstructionError::Custom(1)));
    assert_eq!(err.meta.fee, 5000);
    assert_eq!(svm.get_balance(&payer.pubkey()), Some(5_000));
    assert_eq!(svm.get_balance(&recipient), None);
    assert!(svm.get_transaction(&signature).unwrap().is_err());
}

#[test]
fn transact_discard_of_a_rejected_transaction_leaves_state_untouched() {
    let payer = Keypair::new();
    let recipient = Address::new_unique();
    let mut svm = funded_vm(&payer, 10_000);
    let pre_payer = svm.get_account(&payer.pubkey()).unwrap();
    let tx = transfer_tx(&payer, &recipient, 64, svm.latest_blockhash());
    svm.expire_blockhash();

    let staged = svm.transact(tx).expect("a rejected execution must still stage");
    let err = staged.discard().expect_err("discard must surface the rejection");

    assert_eq!(err.err, TransactionError::BlockhashNotFound);
    assert_eq!(err.meta.fee, 0);
    assert_eq!(svm.get_account(&payer.pubkey()).unwrap(), pre_payer);
    assert_eq!(svm.get_balance(&recipient), None);
}

#[test]
fn commit_records_history_while_discard_does_not() {
    let payer = Keypair::new();
    let recipient = Address::new_unique();
    let mut svm = funded_vm(&payer, 1_000_000);

    let commit_tx = transfer_tx(&payer, &recipient, 64, svm.latest_blockhash());
    let commit_sig = commit_tx.signatures[0];
    svm.transact(commit_tx).expect("transact should stage").commit().unwrap();
    assert!(svm.get_transaction(&commit_sig).is_some());

    let discard_recipient = Address::new_unique();
    let discard_tx = transfer_tx(&payer, &discard_recipient, 32, svm.latest_blockhash());
    let discard_sig = discard_tx.signatures[0];
    svm.transact(discard_tx).expect("transact should stage").discard().unwrap();
    assert!(svm.get_transaction(&discard_sig).is_none());
}

// --- Ported golden assertions from the retired `execution_outcome.rs` ---
//
// The readonly `transact` became `transact_detached` (same shape, minus
// provenance); the stale-outcome rejection tests have no counterpart by
// design — the borrow held by `ExecutedTx` makes committing after an
// intervening mutation a compile error instead of a `ResanitizationNeeded`.

#[test]
fn detached_leaves_state_untouched() {
    let payer = Keypair::new();
    let recipient = Address::new_unique();
    let svm = funded_vm(&payer, 10_000);
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );

    let detached = svm.transact_detached(tx);

    assert!(detached.status().is_ok());
    assert_eq!(svm.get_balance(&recipient), None);
    assert!(detached.post_accounts().iter().any(|(key, account)| {
        use solana_account::ReadableAccount;
        key == &recipient && account.lamports() == 64
    }));
}

#[test]
fn detached_records_the_fee_payer_on_success() {
    let payer = Keypair::new();
    let recipient = Address::new_unique();
    let svm = funded_vm(&payer, 10_000);
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );

    let detached = svm.transact_detached(tx);

    assert!(detached.status().is_ok());
    assert_eq!(
        detached.fee_payer(),
        Some(payer.pubkey()),
        "fee payer must match the transaction's first signer"
    );
    assert_ne!(detached.fee_payer(), Some(recipient));
}

#[test]
fn detached_reports_compute_budget_exceeded() {
    let payer = Keypair::new();
    let recipient = Address::new_unique();
    let svm = funded_vm(&payer, 10_000);
    let tx = Transaction::new(
        &[&payer],
        Message::new(
            &[
                ComputeBudgetInstruction::set_compute_unit_limit(1),
                transfer(&payer.pubkey(), &recipient, 64),
            ],
            Some(&payer.pubkey()),
        ),
        svm.latest_blockhash(),
    );

    let detached = svm.transact_detached(tx);

    assert_eq!(
        detached.status().as_ref().unwrap_err(),
        &TransactionError::InstructionError(0, InstructionError::ComputationalBudgetExceeded)
    );
    assert_eq!(svm.get_balance(&recipient), None, "a failed transaction must not move lamports");
}

#[test]
fn detached_reports_insufficient_funds_for_rent() {
    use solana_account::Account;

    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    // Exactly rent-exempt: paying the fee plus the transfer would drop the fee
    // payer below the rent-exempt minimum.
    svm.set_account(
        payer.pubkey(),
        Account { lamports: 1, owner: solana_sdk_ids::system_program::id(), ..Default::default() },
    )
    .unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 100)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );

    let detached = svm.transact_detached(tx);

    let err = detached.status().as_ref().unwrap_err();
    assert!(
        matches!(
            err,
            TransactionError::InsufficientFundsForRent { .. } |
                TransactionError::InsufficientFundsForFee
        ),
        "expected an insufficient-funds variant, got {err:?}"
    );
    assert_eq!(svm.get_balance(&recipient), None);
}

#[test]
fn detached_reports_invalid_program_for_execution() {
    let payer = Keypair::new();
    let missing_program = Address::new_unique();
    let svm = funded_vm(&payer, 10_000);
    let tx = Transaction::new(
        &[&payer],
        Message::new(
            &[Instruction { program_id: missing_program, accounts: vec![], data: vec![] }],
            Some(&payer.pubkey()),
        ),
        svm.latest_blockhash(),
    );

    let detached = svm.transact_detached(tx);

    assert_eq!(
        detached.status().as_ref().unwrap_err(),
        &TransactionError::InvalidProgramForExecution
    );
}

#[test]
fn detached_reports_blockhash_not_found_after_expiry() {
    let payer = Keypair::new();
    let recipient = Address::new_unique();
    let mut svm = funded_vm(&payer, 10_000);
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );

    svm.expire_blockhash();
    let detached = svm.transact_detached(tx);

    assert_eq!(detached.status().as_ref().unwrap_err(), &TransactionError::BlockhashNotFound);
    assert_eq!(svm.get_balance(&payer.pubkey()), Some(10_000), "no fee may be charged");
}

#[test]
fn staged_commit_charges_failed_program_fees_like_the_legacy_path() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let program_id = address!("HvrRMSshMx3itvsyWDnWg2E3cy5h57iMaR7oVxSZJDSA");
    let Some(failure) = hpsvm_test_support::read_program(hpsvm_test_support::FAILURE) else {
        return;
    };
    svm.add_program(program_id, &failure).unwrap();

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

    let staged = svm.transact(tx).expect("a failing execution must still stage");
    assert!(staged.result().is_err());

    let err = staged.commit().unwrap_err();

    assert_eq!(err.err, TransactionError::InstructionError(0, InstructionError::Custom(0)));
    assert_eq!(svm.get_balance(&payer.pubkey()), Some(initial_balance - 5000));
    assert!(svm.get_transaction(&signature).unwrap().is_err());
}

#[test]
fn staged_commit_with_a_reduced_compute_budget_still_applies() {
    let payer = Keypair::new();
    let recipient = Address::new_unique();
    let mut svm = funded_vm(&payer, 10_000);
    let mut compute_budget = ComputeBudget::new_with_defaults(false);
    compute_budget.compute_unit_limit = 10;
    svm.set_compute_budget(compute_budget);
    // Config mutation before staging is fine; the borrow only forbids mutation
    // while the staged execution is alive.
    let tx = transfer_tx(&payer, &recipient, 64, svm.latest_blockhash());

    let err = svm.transact(tx).expect("transact should stage").commit().unwrap_err();

    assert_eq!(
        err.err,
        TransactionError::InstructionError(0, InstructionError::ComputationalBudgetExceeded)
    );
}

#[cfg(test)]
mod state_change_sink_red {
    use hpsvm::{AccountChange, StateChangeSink};
    use solana_account::ReadableAccount;

    use super::*;

    #[derive(Default)]
    struct RecordingSink {
        changes: Vec<(solana_address::Address, Option<u64>, Option<u64>)>,
    }

    impl StateChangeSink for RecordingSink {
        type Error = std::convert::Infallible;
        fn on_account_change(&mut self, change: AccountChange<'_>) -> Result<(), Self::Error> {
            self.changes.push((
                change.address,
                change.before.map(|a| a.lamports()),
                change.after.map(|a| a.lamports()),
            ));
            Ok(())
        }
    }

    #[test]
    fn commit_with_records_exact_changes_for_transfer() {
        let payer = Keypair::new();
        let recipient = Address::new_unique();
        let mut svm = funded_vm(&payer, 10_000);
        let pre_payer = svm.get_balance(&payer.pubkey()).unwrap();
        let tx = transfer_tx(&payer, &recipient, 64, svm.latest_blockhash());

        let staged = svm.transact(tx).expect("transact should stage");
        let mut sink = RecordingSink::default();
        let meta = staged.commit_with(&mut sink).unwrap();
        assert!(meta.is_ok());

        assert_eq!(sink.changes.len(), 2);
        // One entry per touched account, with exact before/after lamports.
        let payer_change = sink.changes.iter().find(|c| c.0 == payer.pubkey()).unwrap();
        assert_eq!(payer_change.1, Some(pre_payer));
        assert_eq!(payer_change.2, Some(pre_payer - 64 - 5000));
        let recipient_change = sink.changes.iter().find(|c| c.0 == recipient).unwrap();
        assert_eq!(recipient_change.1, None);
        assert_eq!(recipient_change.2, Some(64));
    }
}

#[cfg(test)]
mod state_change_sink_extra {
    use hpsvm::{AccountChange, StateChangeSink};
    use solana_account::ReadableAccount;

    use super::*;

    #[derive(Default)]
    struct RecordingSink {
        changes: Vec<(solana_address::Address, Option<u64>, Option<u64>)>,
    }
    impl StateChangeSink for RecordingSink {
        type Error = std::convert::Infallible;
        fn on_account_change(&mut self, change: AccountChange<'_>) -> Result<(), Self::Error> {
            self.changes.push((
                change.address,
                change.before.map(|a| a.lamports()),
                change.after.map(|a| a.lamports()),
            ));
            Ok(())
        }
    }

    #[derive(Default)]
    struct FailingSink {
        fail_on: usize,
        calls: usize,
    }
    impl StateChangeSink for FailingSink {
        type Error = &'static str;
        fn on_account_change(&mut self, _change: AccountChange<'_>) -> Result<(), Self::Error> {
            self.calls += 1;
            if self.calls == self.fail_on { Err("sink boom") } else { Ok(()) }
        }
    }

    #[derive(Default)]
    struct NoopSink;
    impl StateChangeSink for NoopSink {
        type Error = std::convert::Infallible;
        fn on_account_change(&mut self, _change: AccountChange<'_>) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[test]
    fn commit_with_records_exact_changes_for_failed_transfer() {
        let payer = Keypair::new();
        let recipient = Address::new_unique();
        let mut svm = funded_vm(&payer, 10_000);
        let pre_payer = svm.get_balance(&payer.pubkey()).unwrap();
        let tx = Transaction::new(
            &[&payer],
            Message::new(&[transfer(&payer.pubkey(), &recipient, u64::MAX)], Some(&payer.pubkey())),
            svm.latest_blockhash(),
        );

        let staged = svm.transact(tx).expect("transact should stage");
        let mut sink = RecordingSink::default();
        let result = staged.commit_with(&mut sink).unwrap();
        assert!(result.is_err());

        assert_eq!(sink.changes.len(), 2);
        let payer_change = sink.changes.iter().find(|c| c.0 == payer.pubkey()).unwrap();
        assert_eq!(payer_change.1, Some(pre_payer));
        assert_eq!(payer_change.2, Some(pre_payer - 5000));
        let recipient_change = sink.changes.iter().find(|c| c.0 == recipient).unwrap();
        assert_eq!(recipient_change.1, None);
        assert_eq!(recipient_change.2, None);
    }

    #[test]
    fn commit_with_sink_error_mid_stream_rolls_back_state() {
        let payer = Keypair::new();
        let recipient = Address::new_unique();
        let mut svm = funded_vm(&payer, 10_000);
        let pre_payer_lamports = svm.get_balance(&payer.pubkey()).unwrap();
        let tx = transfer_tx(&payer, &recipient, 64, svm.latest_blockhash());
        let signature = tx.signatures[0];

        let staged = svm.transact(tx).expect("transact should stage");
        let mut sink = FailingSink { fail_on: 1, calls: 0 };
        let err = staged.commit_with(&mut sink).unwrap_err();
        assert_eq!(err, "sink boom");

        // State rolled back: balances restored, no history entry.
        assert_eq!(svm.get_balance(&payer.pubkey()), Some(pre_payer_lamports));
        assert_eq!(svm.get_balance(&recipient), None);
        assert!(svm.get_transaction(&signature).is_none());
    }

    #[test]
    fn commit_with_noop_sink_matches_commit() {
        let payer = Keypair::new();
        let recipient = Address::new_unique();

        // commit() path
        let mut svm_a = funded_vm(&payer, 10_000);
        let tx_a = transfer_tx(&payer, &recipient, 64, svm_a.latest_blockhash());
        let sig_a = tx_a.signatures[0];
        let meta_a = svm_a.transact(tx_a).expect("transact should stage").commit().unwrap();
        assert_eq!(svm_a.get_balance(&payer.pubkey()), Some(4_936));
        assert_eq!(svm_a.get_balance(&recipient), Some(64));
        assert!(svm_a.get_transaction(&sig_a).is_some());

        // commit_with(NoopSink) path
        let mut svm_b = funded_vm(&payer, 10_000);
        let recipient_b = Address::new_unique();
        let tx_b = transfer_tx(&payer, &recipient_b, 64, svm_b.latest_blockhash());
        let sig_b = tx_b.signatures[0];
        let mut noop = NoopSink;
        let result_b =
            svm_b.transact(tx_b).expect("transact should stage").commit_with(&mut noop).unwrap();
        let meta_b = result_b.unwrap();
        assert_eq!(meta_a.fee, meta_b.fee);
        assert_eq!(meta_a.compute_units_consumed, meta_b.compute_units_consumed);
        assert_eq!(svm_b.get_balance(&payer.pubkey()), Some(4_936));
        assert_eq!(svm_b.get_balance(&recipient_b), Some(64));
        assert!(svm_b.get_transaction(&sig_b).is_some());
    }
}
