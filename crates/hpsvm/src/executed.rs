//! Staged transaction execution: [`ExecutedTx`] and [`DetachedTx`].
//!
//! `transact` runs the existing readonly execution, then stages the resulting
//! `post_accounts` into the live overlay (journaled, so [`ExecutedTx::discard`]
//! restores the pre-transaction state via rollback). This replaces the old
//! `transact` + `commit_transaction` pair: borrowing (`&mut self`) replaces the
//! runtime provenance check (`origin_vm_instance_id` / `origin_state_version`),
//! and the second full-vector apply at commit time is gone because the state is
//! already live.
//!
//! [`DetachedTx`] is the owned readonly outcome (`ExecutionOutcome` minus
//! provenance). It serves the paths that never touch live state: `simulate_*`,
//! `try_transact`, the fixture/CLI snapshot builders, and the parallel batch
//! workers (each worker owns its `Svm`, so staging into a shared overlay is
//! neither possible nor wanted there).

use solana_account::{AccountSharedData, ReadableAccount};
use solana_address::Address;
use solana_signature::Signature;
use solana_transaction_error::TransactionError;

use crate::{
    InspectMode, Svm, SvmTypesHost,
    accounts_db::{StateCheckpoint, is_managed_sysvar_account},
    error::{ExecutionError, TxError},
    state_change_sink::{AccountChange, AccountChangeRecord, StateChangeSink},
    types::{TransactionMetadata, TransactionResult},
};

/// Owned readonly execution outcome: what `transact` used to return, minus the
/// deleted provenance fields.
///
/// Produced by [`Svm::transact_detached`] and [`Svm::try_transact`]. Carries no
/// borrow, so it can cross VM instances (batch workers, fixture snapshots).
///
/// `fatal_error` is a crate-internal transport: the default handler embeds the
/// readonly execution's fatal error here (instead of dropping it) so the
/// fallible entry points (`try_transact`, `transact_core`) keep their exact
/// legacy conventions for it, while infallible paths (`transact_detached`,
/// batch workers) keep reading the failure from `status` exactly as before.
/// It is invisible outside the crate and never participates in equality, which
/// is why this type implements `Debug` but not `PartialEq`/`Eq`.
#[derive(Debug)]
pub struct DetachedTx {
    pub(crate) meta: TransactionMetadata,
    pub(crate) post_accounts: Vec<(Address, AccountSharedData)>,
    pub(crate) status: Result<(), TransactionError>,
    pub(crate) included: bool,
    pub(crate) fee_payer: Option<Address>,
    pub(crate) fatal_error: Option<ExecutionError>,
    /// Whether the installed inspector accepted this execution, captured at
    /// execution time so the staged commit path does not consult it again.
    pub(crate) inspector_live: bool,
}

impl DetachedTx {
    /// Returns the transaction metadata captured during execution.
    pub fn meta(&self) -> &TransactionMetadata {
        &self.meta
    }

    /// Returns the writable post-execution account snapshot.
    pub fn post_accounts(&self) -> &[(Address, AccountSharedData)] {
        &self.post_accounts
    }

    /// Returns the execution status.
    pub fn status(&self) -> &Result<(), TransactionError> {
        &self.status
    }

    /// Returns whether this outcome is eligible for commit-time side effects.
    pub fn included(&self) -> bool {
        self.included
    }

    /// Returns the fee payer address, if one was validated for this transaction.
    pub fn fee_payer(&self) -> Option<Address> {
        self.fee_payer
    }

    /// Takes the transported fatal error, if the readonly execution produced one.
    ///
    /// Used by the fallible entry points to preserve their legacy
    /// fatal-error conventions without losing the detached outcome.
    #[inline]
    pub(crate) fn take_fatal(&mut self) -> Option<ExecutionError> {
        self.fatal_error.take()
    }

    /// Builds the minimal failed outcome for a dispatch failure that cannot
    /// propagate as an error.
    ///
    /// Used by the infallible `transact_detached` (and the result half of the
    /// `transact_core` failure leg) when no handler ran, so there is no
    /// execution data to report. Unreachable in practice — the defaults cover
    /// every tag [`tag_of`](crate::tag_of) can derive — but infallible paths
    /// still need a well-typed answer.
    #[cold]
    pub(crate) fn for_dispatch_failure(error: &TxError, signature: Signature) -> Self {
        Self {
            meta: TransactionMetadata { signature, ..Default::default() },
            post_accounts: Vec::new(),
            status: Err(error.transaction_error()),
            included: false,
            fee_payer: None,
            fatal_error: None,
            inspector_live: false,
        }
    }
}

/// A staged transaction holding a mutable borrow on its VM.
///
/// The post-state is already live in the overlay; [`commit`](ExecutedTx::commit)
/// keeps it (and records the history entry), [`discard`](ExecutedTx::discard)
/// rolls the overlay back to the pre-transaction checkpoint. Either consumes
/// `self`, so double-commit is impossible by construction. Holds no owned
/// `post_accounts` vector: the state lives in the overlay journal, not in a
/// detached copy.
#[must_use = "call commit to keep the staged post-state or discard to roll it back"]
pub struct ExecutedTx<'svm, 'a, T: SvmTypesHost> {
    svm: &'svm mut Svm<'a, T>,
    checkpoint: StateCheckpoint,
    result: TransactionResult,
    history_entry: Option<(Signature, TransactionResult)>,
    cache_dirty: bool,
    changes: Vec<AccountChangeRecord>,
    /// Whether the installed inspector accepted this execution. Decided once
    /// during execution and carried here so commit-time `account_change`
    /// hooks fire without consulting the inspector a second time.
    ///
    /// A plain `bool` survives here deliberately, not the `InspectMode` const
    /// the execution ran under: `ExecutedTx` is a runtime value, so the mode
    /// is gone by commit time. Re-consulting at commit would also be wrong —
    /// the inspector could have been installed or cleared between `transact`
    /// and `commit`, and execution-time liveness is what the hooks must
    /// reflect. Commit therefore runtime-branches on this bit into the
    /// `DynInspector` dispatcher instantiation below.
    inspector_live: bool,
}

impl<'svm, 'a, T: SvmTypesHost> ExecutedTx<'svm, 'a, T> {
    pub(crate) fn new(
        svm: &'svm mut Svm<'a, T>,
        checkpoint: StateCheckpoint,
        result: TransactionResult,
        history_entry: Option<(Signature, TransactionResult)>,
        cache_dirty: bool,
        changes: Vec<AccountChangeRecord>,
        inspector_live: bool,
    ) -> Self {
        Self { svm, checkpoint, result, history_entry, cache_dirty, changes, inspector_live }
    }

    /// Returns the execution result without resolving the staged state.
    pub const fn result(&self) -> &TransactionResult {
        &self.result
    }

    /// Keeps the staged post-state and records the history entry (if any).
    ///
    /// When the installed inspector accepted this execution, every recorded
    /// account change is also forwarded to its `account_change` hook, in
    /// staging order, before the history entry is recorded.
    pub fn commit(self) -> TransactionResult {
        let Self { svm, history_entry, result, changes, inspector_live, .. } = self;
        if inspector_live {
            for record in &changes {
                svm.on_account_change::<{ InspectMode::DynInspector }>(AccountChange {
                    address: record.address,
                    before: record.before.as_ref(),
                    after: record.after.as_ref(),
                });
            }
        }
        if let Some((signature, entry)) = history_entry {
            svm.history.add_new_transaction(signature, entry);
        }
        result
    }

    /// Keeps the staged post-state, streams every recorded account change into
    /// `sink`, and records the history entry (if any).
    ///
    /// On a sink error the staged state is rolled back (journal restored and
    /// derived caches rebuilt when needed) and the error is propagated; the
    /// transaction is left uncommitted and no history entry is recorded.
    pub fn commit_with<S: StateChangeSink>(
        self,
        sink: &mut S,
    ) -> Result<TransactionResult, S::Error> {
        let Self { svm, checkpoint, result, history_entry, cache_dirty, changes, inspector_live } =
            self;
        for record in &changes {
            let change = AccountChange {
                address: record.address,
                before: record.before.as_ref(),
                after: record.after.as_ref(),
            };
            if let Err(error) = sink.on_account_change(change) {
                svm.accounts.rollback(checkpoint);
                if cache_dirty {
                    svm.accounts.rebuild_sysvar_cache();
                    svm.sync_jit_policy();
                    svm.accounts.rebuild_program_cache().expect(
                        "rollback restores previously valid state, so the cache rebuild cannot fail",
                    );
                }
                return Err(error);
            }
        }
        if inspector_live {
            for record in &changes {
                svm.on_account_change::<{ InspectMode::DynInspector }>(AccountChange {
                    address: record.address,
                    before: record.before.as_ref(),
                    after: record.after.as_ref(),
                });
            }
        }
        if let Some((signature, entry)) = history_entry {
            svm.history.add_new_transaction(signature, entry);
        }
        let _ = checkpoint;
        Ok(result)
    }

    /// Rolls the overlay back to the pre-transaction checkpoint and returns
    /// the execution result without recording history.
    ///
    /// Account state is restored via the overlay journal, not via a saved
    /// copy. Program/sysvar caches are derived from account state and are not
    /// journaled, so they are rebuilt here when (and only when) the staged
    /// accounts could have touched them.
    pub fn discard(self) -> TransactionResult {
        self.svm.accounts.rollback(self.checkpoint);
        if self.cache_dirty {
            self.svm.accounts.rebuild_sysvar_cache();
            self.svm.sync_jit_policy();
            self.svm.accounts.rebuild_program_cache().expect(
                "rollback restores previously valid state, so the cache rebuild cannot fail",
            );
        }
        self.result
    }
}

/// Fatal staging failure: carries both the [`HPSVMError`] for fallible
/// callers (`transact`, `try_transact`) and the legacy-identical
/// [`TransactionResult`] for infallible callers (`send_transaction`,
/// `process_instruction_chain`), which historically surfaced fatal errors as
/// an `Err` result rather than propagating them.
pub(crate) struct StagedFailure {
    error: crate::error::HPSVMError,
    result: TransactionResult,
}

impl StagedFailure {
    pub(crate) const fn new(error: crate::error::HPSVMError, result: TransactionResult) -> Self {
        Self { error, result }
    }

    pub(crate) fn into_error(self) -> crate::error::HPSVMError {
        self.error
    }

    pub(crate) fn into_result(self) -> TransactionResult {
        self.result
    }
}

/// Returns `true` when staging `post_accounts` over the current overlay could
/// invalidate the derived program/sysvar caches, i.e. a staged account is
/// executable, targets a managed sysvar slot, or overwrites an account that
/// currently backs a program-cache entry.
pub(crate) fn staging_touches_caches(
    post_accounts: &[(Address, AccountSharedData)],
    overlay: &crate::accounts_db::AccountsDb,
) -> bool {
    post_accounts.iter().any(|(address, account)| {
        account.executable() ||
            is_managed_sysvar_account(address) ||
            overlay.get_account_ref(address).is_some_and(|pre| pre.executable())
    })
}

#[cfg(test)]
mod tests {
    use solana_keypair::Keypair;
    use solana_message::Message;
    use solana_signer::Signer;
    use solana_system_interface::instruction::transfer;
    use solana_transaction::Transaction;

    use super::*;
    use crate::Hpsvm;

    fn transfer_fixture() -> (Hpsvm, Keypair, Address, Transaction, Signature) {
        let mut svm = Hpsvm::new();
        let payer = Keypair::new();
        let payer_address = payer.pubkey();
        let recipient = Address::new_unique();
        svm.airdrop(&payer_address, 10_000).unwrap();
        let tx = Transaction::new(
            &[&payer],
            Message::new(&[transfer(&payer_address, &recipient, 64)], Some(&payer_address)),
            svm.latest_blockhash(),
        );
        let signature = tx.signatures[0];
        (svm, payer, recipient, tx, signature)
    }

    #[test]
    fn commit_keeps_the_journal_entries_that_stage_the_post_state() {
        let (mut svm, _, _, tx, signature) = transfer_fixture();
        let before = svm.accounts.journal_len();

        let result = svm.transact(tx).expect("transact should stage").commit();

        assert!(result.is_ok());
        // Staging wrote through the journaled overlay: the entries are
        // retained (commit keeps staged state, it does not re-apply a copy).
        assert!(svm.accounts.journal_len() > before);
        assert!(svm.history.check_transaction(&signature));
    }

    #[test]
    fn discard_truncates_the_journal_back_to_the_checkpoint() {
        let (mut svm, payer, recipient, tx, signature) = transfer_fixture();
        let payer_address = payer.pubkey();
        let before = svm.accounts.journal_len();
        let payer_before = svm.get_account(&payer_address).unwrap();

        let result = svm.transact(tx).expect("transact should stage").discard();

        assert!(result.is_ok());
        // Rollback pops exactly the staged entries: no full account clone was
        // retained anywhere, the journal itself is the undo log.
        assert_eq!(svm.accounts.journal_len(), before);
        assert!(!svm.history.check_transaction(&signature));
        // Byte-identical restore of every touched account.
        assert_eq!(svm.get_account(&payer_address).unwrap(), payer_before);
        assert_eq!(svm.get_balance(&recipient), None);
    }

    #[test]
    fn discard_of_a_failed_execution_restores_state_without_history() {
        let (mut svm, payer, recipient, _, _) = transfer_fixture();
        let payer_address = payer.pubkey();
        let payer_before = svm.get_account(&payer_address).unwrap();
        let failing = Transaction::new(
            &[&payer],
            Message::new(&[transfer(&payer_address, &recipient, u64::MAX)], Some(&payer_address)),
            svm.latest_blockhash(),
        );
        // Unsigned garbage signatures: disable sigverify so execution reaches
        // the failing instruction instead of rejecting the signatures.
        svm.set_sigverify(false);
        let before = svm.accounts.journal_len();

        let err = svm.transact(failing).expect("a failing execution must still stage").discard();

        let err = err.expect_err("discard must surface the execution failure");
        assert_eq!(svm.accounts.journal_len(), before);
        assert_eq!(svm.get_account(&payer_address).unwrap(), payer_before);
        assert_eq!(svm.get_balance(&recipient), None);
        assert!(svm.get_transaction(&err.meta.signature).is_none());
    }
}
