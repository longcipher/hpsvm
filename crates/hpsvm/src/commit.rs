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
