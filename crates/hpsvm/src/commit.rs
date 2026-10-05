//! Commit-delta logic for atomically applying execution outcomes to the VM state.
//!
//! This module encapsulates the two-phase commit plumbing: constructing a
//! [`CommitDelta`] from an execution outcome, verifying origin provenance
//! (instance-id + state-version), and applying the delta to accounts and
//! transaction history.

use solana_account::AccountSharedData;
use solana_address::Address;
use solana_signature::Signature;
use solana_transaction_error::TransactionError;

use crate::{
    HPSVM,
    accounts_db::AccountsDb,
    error::HPSVMError,
    history::TransactionHistory,
    types::{ExecutionOutcome, FailedTransactionMetadata, TransactionResult},
};

/// A self-contained set of mutations produced by a single transaction execution.
///
/// Applying a `CommitDelta` to the VM state is the second half of the
/// two-phase commit model (`transact` → `commit_transaction`).
#[derive(Debug, Clone)]
pub(crate) struct CommitDelta {
    post_accounts: Vec<(Address, AccountSharedData)>,
    history_entry: Option<(Signature, TransactionResult)>,
}

impl CommitDelta {
    pub(crate) const fn new(
        post_accounts: Vec<(Address, AccountSharedData)>,
        history_entry: Option<(Signature, TransactionResult)>,
    ) -> Self {
        Self { post_accounts, history_entry }
    }

    pub(crate) const fn mutates_state(&self) -> bool {
        !self.post_accounts.is_empty() || self.history_entry.is_some()
    }
}

/// Apply a commit delta to the VM's account store and transaction history.
///
/// # Errors
///
/// Returns [`HPSVMError`] if writing accounts fails (e.g. invalid sysvar data).
pub(crate) fn apply_commit_delta(
    accounts: &mut AccountsDb,
    history: &mut TransactionHistory,
    delta: CommitDelta,
) -> Result<(), HPSVMError> {
    accounts.sync_accounts(delta.post_accounts)?;
    if let Some((signature, entry)) = delta.history_entry {
        history.add_new_transaction(signature, entry);
    }
    Ok(())
}

/// Decompose an [`ExecutionOutcome`] into its transaction result and the
/// corresponding commit delta.
///
/// When `history_enabled` is `false`, no transaction-history entry is built,
/// which lets the metadata move into the returned result instead of being
/// cloned. This keeps the steady-state hot path (history disabled) free of the
/// two `TransactionMetadata` clones the previous implementation performed per
/// transaction.
pub(crate) fn outcome_into_result_and_delta(
    outcome: ExecutionOutcome,
    history_enabled: bool,
) -> (TransactionResult, CommitDelta) {
    let ExecutionOutcome { meta, post_accounts, status, included, .. } = outcome;
    let signature = meta.signature;
    // Move `meta` into the result instead of cloning it.
    let result = match status {
        Ok(()) => TransactionResult::Ok(meta),
        Err(err) => TransactionResult::Err(FailedTransactionMetadata { err, meta }),
    };
    let delta = if included {
        // Only clone the full result when history is actually going to store it.
        let history_entry = history_enabled.then(|| (signature, result.clone()));
        CommitDelta::new(post_accounts, history_entry)
    } else {
        CommitDelta::new(Vec::new(), None)
    };
    (result, delta)
}

/// Verify the origin provenance and commit the execution outcome to the VM.
///
/// The two-phase commit model requires that the execution outcome was produced
/// by the same VM instance at the same state version. If either check fails,
/// the outcome is rejected with [`TransactionError::ResanitizationNeeded`].
pub(crate) fn commit_execution_outcome(
    vm: &mut HPSVM,
    outcome: ExecutionOutcome,
) -> TransactionResult {
    let origin_vm_instance_id = outcome.origin_vm_instance_id;
    let origin_state_version = outcome.origin_state_version;

    if origin_vm_instance_id != vm.instance_id || origin_state_version != vm.state_version {
        return TransactionResult::Err(FailedTransactionMetadata {
            err: TransactionError::ResanitizationNeeded,
            meta: outcome.meta,
        });
    }

    let history_enabled = vm.history.is_enabled();
    let (result, delta) = crate::hotpath_block!(
        "hpsvm::commit::outcome_into_result_and_delta",
        outcome_into_result_and_delta(outcome, history_enabled)
    );
    let mutates_state = delta.mutates_state();

    crate::hotpath_block!("hpsvm::commit::apply_commit_delta", {
        apply_commit_delta(&mut vm.accounts, &mut vm.history, delta)
            .expect("It shouldn't be possible to write invalid sysvars in send_transaction.");
    });
    if mutates_state {
        vm.invalidate_execution_outcomes();
    }
    result
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use solana_account::Account;
    use solana_address::Address;
    use solana_signature::Signature;

    use super::*;
    use crate::types::TransactionMetadata;

    fn outcome_with(
        status: Result<(), TransactionError>,
        included: bool,
        post_accounts: Vec<(Address, AccountSharedData)>,
    ) -> ExecutionOutcome {
        let signature = Signature::from([7; 64]);
        ExecutionOutcome {
            meta: TransactionMetadata { signature, ..Default::default() },
            post_accounts,
            status,
            included,
            origin_vm_instance_id: 1,
            origin_state_version: 1,
            fee_payer: Some(Address::new_unique()),
        }
    }

    fn account(lamports: u64) -> AccountSharedData {
        AccountSharedData::new(lamports, 0, &Address::new_unique())
    }

    // -----------------------------------------------------------------
    // CommitDelta
    // -----------------------------------------------------------------

    #[test]
    fn an_empty_delta_does_not_mutate_state() {
        assert!(!CommitDelta::new(Vec::new(), None).mutates_state());
    }

    #[test]
    fn a_delta_with_post_accounts_mutates_state() {
        let delta = CommitDelta::new(vec![(Address::new_unique(), account(1))], None);
        assert!(delta.mutates_state());
    }

    #[test]
    fn a_delta_with_only_a_history_entry_mutates_state() {
        let delta = CommitDelta::new(
            Vec::new(),
            Some((Signature::from([1; 64]), TransactionResult::Ok(TransactionMetadata::default()))),
        );
        assert!(delta.mutates_state());
    }

    // -----------------------------------------------------------------
    // outcome_into_result_and_delta
    // -----------------------------------------------------------------

    #[test]
    fn a_successful_outcome_moves_the_metadata_into_the_result() {
        let outcome = outcome_with(Ok(()), true, vec![(Address::new_unique(), account(1))]);
        let (result, delta) = outcome_into_result_and_delta(outcome, false);

        assert!(result.is_ok());
        assert_eq!(delta.post_accounts.len(), 1);
        // History disabled: no entry is built at all.
        assert!(delta.history_entry.is_none());
        assert!(delta.mutates_state());
    }

    #[test]
    fn a_failed_outcome_produces_failed_metadata() {
        let outcome = outcome_with(
            Err(TransactionError::AccountNotFound),
            true,
            vec![(Address::new_unique(), account(1))],
        );
        let (result, delta) = outcome_into_result_and_delta(outcome, true);

        match result {
            TransactionResult::Err(meta) => {
                assert!(matches!(meta.err, TransactionError::AccountNotFound));
            }
            other => panic!("expected TransactionResult::Err, got {}", is_ok(&other)),
        }
        assert!(delta.history_entry.is_some());
    }

    /// An excluded outcome is not eligible for commit-time side effects, so its
    /// post accounts must be discarded rather than applied.
    #[test]
    fn an_excluded_outcome_discards_its_post_accounts() {
        let outcome = outcome_with(Ok(()), false, vec![(Address::new_unique(), account(1))]);
        let (result, delta) = outcome_into_result_and_delta(outcome, true);

        assert!(result.is_ok());
        assert!(delta.post_accounts.is_empty());
        assert!(delta.history_entry.is_none());
        assert!(!delta.mutates_state());
    }

    /// With history enabled the result is cloned so both the caller and the
    /// history hold the same value.
    #[test]
    fn an_included_outcome_with_history_enabled_records_an_entry() {
        let outcome = outcome_with(Ok(()), true, Vec::new());
        let (result, delta) = outcome_into_result_and_delta(outcome, true);

        let (signature, stored) = delta.history_entry.clone().expect("history entry must be built");
        assert_eq!(signature, Signature::from([7; 64]));
        assert_eq!(stored, result);
        assert!(delta.mutates_state());
    }

    /// With history disabled the metadata must move (not be cloned) into the
    /// result, and no entry may be built.
    #[test]
    fn an_included_outcome_with_history_disabled_records_no_entry() {
        let outcome = outcome_with(Ok(()), true, Vec::new());
        let (result, delta) = outcome_into_result_and_delta(outcome, false);

        assert!(delta.history_entry.is_none());
        assert!(!delta.mutates_state());
        assert!(result.is_ok());
    }

    // Property: `included` alone decides whether post accounts survive, and
    // `history_enabled` alone decides whether an entry is built.
    proptest! {
        #[test]
        fn included_and_history_enabled_independently_control_the_delta(
            included in any::<bool>(),
            history_enabled in any::<bool>(),
            lamports in any::<u64>(),
        ) {
            let post_accounts = vec![(Address::new_unique(), account(lamports))];
            let outcome = outcome_with(Ok(()), included, post_accounts);
            let (_, delta) = outcome_into_result_and_delta(outcome, history_enabled);

            prop_assert_eq!(delta.post_accounts.is_empty(), !included);
            prop_assert_eq!(delta.history_entry.is_some(), included && history_enabled);
            prop_assert_eq!(delta.mutates_state(), included);
        }
    }

    // Property: the result and any stored history entry are always equal, for
    // both success and failure statuses.
    proptest! {
        #[test]
        fn the_history_entry_always_mirrors_the_result(
            included in any::<bool>(),
            history_enabled in any::<bool>(),
            fails in any::<bool>(),
        ) {
            let status = if fails { Err(TransactionError::AlreadyProcessed) } else { Ok(()) };
            let outcome = outcome_with(status, included, Vec::new());
            let (result, delta) = outcome_into_result_and_delta(outcome, history_enabled);

            match delta.history_entry {
                Some((_, stored)) => prop_assert_eq!(stored, result),
                None => prop_assert!(!(included && history_enabled)),
            }
        }
    }

    // -----------------------------------------------------------------
    // apply_commit_delta
    // -----------------------------------------------------------------

    #[test]
    fn applying_a_delta_writes_post_accounts() {
        let mut svm = HPSVM::new();
        let address = Address::new_unique();

        let delta = CommitDelta::new(
            vec![(address, AccountSharedData::new(123, 0, &solana_sdk_ids::system_program::id()))],
            None,
        );
        apply_commit_delta(&mut svm.accounts, &mut svm.history, delta).unwrap();

        assert_eq!(svm.get_balance(&address), Some(123));
    }

    #[test]
    fn applying_a_delta_records_the_history_entry() {
        let mut svm = HPSVM::new();
        let signature = Signature::from([9; 64]);

        let delta = CommitDelta::new(
            Vec::new(),
            Some((signature, TransactionResult::Ok(TransactionMetadata::default()))),
        );
        apply_commit_delta(&mut svm.accounts, &mut svm.history, delta).unwrap();

        assert!(svm.history.check_transaction(&signature));
        assert!(svm.history.get_transaction(&signature).is_some());
    }

    #[test]
    fn applying_an_empty_delta_is_a_no_op() {
        let mut svm = HPSVM::new();
        apply_commit_delta(&mut svm.accounts, &mut svm.history, CommitDelta::new(Vec::new(), None))
            .unwrap();
    }

    /// A disabled history must silently drop the entry rather than storing it.
    #[test]
    fn applying_a_delta_with_a_disabled_history_drops_the_entry() {
        let mut svm = HPSVM::new();
        svm.set_transaction_history(0);
        let signature = Signature::from([9; 64]);

        let delta = CommitDelta::new(
            Vec::new(),
            Some((signature, TransactionResult::Ok(TransactionMetadata::default()))),
        );
        apply_commit_delta(&mut svm.accounts, &mut svm.history, delta).unwrap();

        assert!(!svm.history.is_enabled());
        assert!(!svm.history.check_transaction(&signature));
    }

    /// A write into a sysvar slot with malformed data must surface as an error
    /// rather than silently corrupting the sysvar.
    #[test]
    fn applying_a_sysvar_account_with_invalid_data_is_rejected() {
        let mut svm = HPSVM::new();
        let sysvar = solana_sdk_ids::sysvar::clock::id();

        let broken = Account {
            lamports: 1,
            data: vec![0xff; 7],
            owner: sysvar,
            executable: false,
            rent_epoch: 0,
        };

        let error = apply_commit_delta(
            &mut svm.accounts,
            &mut svm.history,
            CommitDelta::new(vec![(sysvar, AccountSharedData::from(broken))], None),
        )
        .expect_err("a malformed sysvar must be rejected");
        assert!(matches!(error, crate::error::HPSVMError::InvalidSysvarData(_)), "got {error:?}");
    }

    /// A valid sysvar account must be accepted and take effect.
    #[test]
    fn applying_a_valid_sysvar_account_succeeds() {
        let mut svm = HPSVM::new();
        let sysvar = solana_sdk_ids::sysvar::clock::id();
        // Round-trip the existing sysvar account so the payload is well formed.
        let existing =
            svm.get_account(&sysvar).expect("a fresh VM must have a clock sysvar account");

        apply_commit_delta(
            &mut svm.accounts,
            &mut svm.history,
            CommitDelta::new(vec![(sysvar, AccountSharedData::from(existing))], None),
        )
        .expect("a valid clock sysvar must be accepted");

        let expected = svm.block_env().slot;
        let clock: solana_clock::Clock = svm.get_sysvar();
        assert_eq!(clock.slot, expected, "the sysvar must survive the delta");
    }

    /// A non-sysvar account is written without any validation.
    #[test]
    fn applying_a_non_sysvar_account_is_never_validated() {
        let mut svm = HPSVM::new();
        let address = Address::new_unique();
        let account = Account {
            lamports: 9,
            data: vec![0xff; 3],
            owner: Address::new_unique(),
            executable: false,
            rent_epoch: 0,
        };

        apply_commit_delta(
            &mut svm.accounts,
            &mut svm.history,
            CommitDelta::new(vec![(address, AccountSharedData::from(account))], None),
        )
        .expect("a regular account must be accepted");

        assert_eq!(svm.get_balance(&address), Some(9));
    }

    fn is_ok(result: &TransactionResult) -> bool {
        result.is_ok()
    }
}
