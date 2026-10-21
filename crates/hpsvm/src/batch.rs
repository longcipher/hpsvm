use std::{collections::HashSet, sync::Arc};

use rayon::prelude::*;
use solana_address::Address;
use solana_message::VersionedMessage;
use solana_transaction::{sanitized::SanitizedTransaction, versioned::VersionedTransaction};
use solana_transaction_error::TransactionError;
use thiserror::Error;

use crate::{
    AccountSourceError, StateChangeSet, Svm, SvmTypesHost, TransactionOrigin,
    account_source::AccountSourceErrorKind, accounts_db::AccountsDb, apply_commit_delta,
    error::ExecutionError, history::TransactionHistory, types::TransactionResult,
};

/// A conflict-free stage in a transaction batch plan.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TransactionBatchStage {
    /// Indexes into the original caller-provided transaction list.
    pub transaction_indexes: Vec<usize>,
}

/// A greedy conflict-aware schedule for a transaction batch.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TransactionBatchPlan {
    /// Conflict-free stages in scheduling order.
    pub stages: Vec<TransactionBatchStage>,
}

/// The outcome of a batch submission.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TransactionBatchExecutionResult {
    /// The conflict-aware schedule computed for the batch.
    pub plan: TransactionBatchPlan,
    /// Per-transaction execution results in the original input order.
    pub results: Vec<TransactionResult>,
}

/// Errors encountered while planning a transaction batch.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransactionBatchError {
    /// The transaction could not be sanitized for scheduling.
    #[error("failed to sanitize transaction #{index} for batch scheduling: {source}")]
    Sanitize {
        /// Original transaction index in the submitted batch.
        index: usize,
        /// Underlying transaction sanitization error.
        source: TransactionError,
    },
    /// The configured account source failed while loading data needed for scheduling.
    #[error(
        "account source failed while sanitizing transaction #{index} account {pubkey}: {source}"
    )]
    AccountSource {
        /// Original transaction index in the submitted batch.
        index: usize,
        /// Account address that triggered the source read.
        pubkey: Address,
        /// Underlying account source error.
        source: AccountSourceError,
    },
}

#[derive(Debug)]
enum BatchSanitizeError {
    Transaction(TransactionError),
    AccountSource { pubkey: Address, source: AccountSourceError },
}

impl BatchSanitizeError {
    fn into_batch_error(self, index: usize) -> TransactionBatchError {
        match self {
            Self::Transaction(source) => TransactionBatchError::Sanitize { index, source },
            Self::AccountSource { pubkey, source } => {
                TransactionBatchError::AccountSource { index, pubkey, source }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct HpsvmRuntimeState {
    accounts: Arc<AccountsDb>,
    history: Arc<TransactionHistory>,
}

impl Default for HpsvmRuntimeState {
    fn default() -> Self {
        Self {
            accounts: Arc::new(AccountsDb::default()),
            history: Arc::new(TransactionHistory::new()),
        }
    }
}

impl HpsvmRuntimeState {
    fn from_vm<T: SvmTypesHost>(vm: &Svm<'_, T>) -> Self {
        Self {
            accounts: Arc::new(vm.state.accounts.clone()),
            history: Arc::new(vm.state.history.clone()),
        }
    }
}

#[derive(Clone, Debug)]
struct BatchExecutionSnapshot {
    runtime: HpsvmRuntimeState,
}

impl BatchExecutionSnapshot {
    fn from_vm<T: SvmTypesHost>(vm: &Svm<'_, T>) -> Self {
        Self { runtime: HpsvmRuntimeState::from_vm(vm) }
    }
}

/// Plans a conflict-aware transaction batch without committing any state.
///
/// The returned stages contain indexes into the original input order. Stages
/// are built greedily from account read/write conflicts and form the basis
/// for higher-level batch schedulers.
///
/// A transaction joins the *smallest* conflict-free stage (best fit) rather
/// than the first one. Filling the tightest stage first keeps the heavily
/// locked stages free for the transactions that need them, which tends to
/// produce fewer serial rounds than first fit — and the number of rounds is
/// exactly the parallelisation limit of the batch.
#[cfg_attr(feature = "hotpath", hotpath::measure)]
pub(crate) fn plan_transaction_batch<T: SvmTypesHost>(
    vm: &Svm<'_, T>,
    txs: &[VersionedTransaction],
) -> Result<TransactionBatchPlan, TransactionBatchError> {
    let mut stages = Vec::<ScheduledTransactionBatchStage>::new();

    for (index, tx) in txs.iter().enumerate() {
        let sanitized = sanitize_transaction_for_batch(vm, tx.clone())
            .map_err(|source| source.into_batch_error(index))?;
        let lock_set = TransactionLockSet::from_transaction(&sanitized, &tx.message);

        match best_fit_stage(&stages, &lock_set) {
            Some(stage_index) => {
                let stage = &mut stages[stage_index];
                stage.transaction_indexes.push(index);
                stage.lock_set.extend(&lock_set);
            }
            None => {
                stages.push(ScheduledTransactionBatchStage {
                    transaction_indexes: vec![index],
                    lock_set,
                });
            }
        }
    }

    Ok(TransactionBatchPlan {
        stages: stages
            .into_iter()
            .map(|stage| TransactionBatchStage { transaction_indexes: stage.transaction_indexes })
            .collect(),
    })
}

/// Returns the index of the smallest conflict-free stage, if any.
///
/// "Smallest" is measured by the number of accounts the stage locks, which
/// is the classic best-fit heuristic for graph colouring: the transaction
/// takes the tightest slot that still fits.
fn best_fit_stage(
    stages: &[ScheduledTransactionBatchStage],
    lock_set: &TransactionLockSet,
) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None;
    for (index, stage) in stages.iter().enumerate() {
        if stage.lock_set.conflicts_with(lock_set) {
            continue;
        }
        let weight = stage.lock_set.len();
        if best.is_none_or(|(_, best_weight)| weight < best_weight) {
            best = Some((index, weight));
        }
    }
    best.map(|(index, _)| index)
}

#[cfg_attr(feature = "hotpath", hotpath::measure)]
pub(crate) fn send_transaction_batch<T: SvmTypesHost>(
    vm: &mut Svm<'_, T>,
    transactions: Vec<VersionedTransaction>,
) -> Result<TransactionBatchExecutionResult, TransactionBatchError> {
    let plan = plan_transaction_batch(vm, &transactions)?;
    let mut results = vec![None; transactions.len()];

    for (stage_index, stage) in plan.stages.iter().enumerate() {
        if stage.transaction_indexes.len() == 1 {
            let index = stage.transaction_indexes[0];
            results[index] = Some(vm.with_transaction_origin(
                TransactionOrigin::Batch { stage_index, transaction_index: index },
                |vm| vm.send_transaction(transactions[index].clone()),
            ));
            continue;
        }

        // Prewarm the stage read-set with a single batched
        // source load, so workers clone an already-warm overlay instead of
        // issuing per-transaction source round-trips. Single-transaction
        // stages skip this: they run serially on the live overlay, which is
        // warm by construction.
        let read_set = stage_read_set(stage, &transactions);
        if let Err(db_error) = vm.state.accounts.prewarm(&read_set) {
            // `prewarm` only fails on the batched source load, which implies a
            // non-empty missing set and hence a non-empty read set. A batched
            // load cannot name the failing key, so attribute the error to the
            // stage head (first transaction, read-set head). The `DbError`
            // still carries the fatal classification for future conflict
            // logic; the legacy `TransactionBatchError` shape keeps the
            // concrete `AccountSourceError`, recovered by downcast.
            let index = stage.transaction_indexes[0];
            let pubkey = read_set[0];
            let source =
                db_error.downcast_ref::<AccountSourceError>().cloned().unwrap_or_else(|| {
                    AccountSourceError::with_kind(
                        AccountSourceErrorKind::Other,
                        db_error.to_string(),
                    )
                });
            return Err(TransactionBatchError::AccountSource { index, pubkey, source });
        }

        // Program-side twin of the account prewarm (JIT spec F3): resolve
        // the stage's program set before spawning workers so the first
        // transaction never pays the program-set load serially. Under `Lazy`
        // the set is additionally scheduled for promotion.
        if let Err(error) = vm.prewarm_programs(&read_set) {
            // Like the account prewarm above, this only fails when `read_set`
            // is non-empty (empty prewarm is a no-op), so indexing the head
            // is safe. `Svm::prewarm_programs` only surfaces `Database`
            // errors; anything else is defensively mapped to `Other`.
            let index = stage.transaction_indexes[0];
            let pubkey = read_set[0];
            let source = match &error {
                crate::error::HPSVMError::Database(db) => {
                    db.downcast_ref::<AccountSourceError>().cloned().unwrap_or_else(|| {
                        AccountSourceError::with_kind(AccountSourceErrorKind::Other, db.to_string())
                    })
                }
                other => {
                    AccountSourceError::with_kind(AccountSourceErrorKind::Other, other.to_string())
                }
            };
            return Err(TransactionBatchError::AccountSource { index, pubkey, source });
        }

        let mut stage_results =
            execute_transaction_batch_stage(vm, stage_index, stage, &transactions);
        stage_results.sort_by_key(|result| result.index);

        for stage_result in stage_results {
            apply_commit_delta(&mut vm.state.accounts, &mut vm.state.history, stage_result.delta)
                .expect("batch stage merge should only apply valid account states");
            results[stage_result.index] = Some(stage_result.result);
        }
    }

    let results = results
        .into_iter()
        .map(|result| {
            result.expect("internal invariant: every transaction index in the batch plan must have a corresponding result slot")
        })
        .collect();
    Ok(TransactionBatchExecutionResult { plan, results })
}

/// Union of static account keys referenced by a stage's transactions.
///
/// This is the read-set adaptation: the public plan carries only
/// `transaction_indexes` (lock sets are consumed at plan time), so the
/// prewarm set is re-derived from each stage transaction's static keys.
/// Lookup-table dynamic addresses are excluded — planning already resolved
/// them against the source during sanitize, and legacy transactions have no
/// dynamic addresses at all. Keys absent here simply fall through to per-key
/// source loads during execution: prewarm is purely an optimization and never
/// changes execution semantics.
fn stage_read_set(
    stage: &TransactionBatchStage,
    transactions: &[VersionedTransaction],
) -> Vec<Address> {
    let mut seen = HashSet::new();
    let mut union = Vec::new();
    for &index in &stage.transaction_indexes {
        let Some(tx) = transactions.get(index) else { continue };
        for key in tx.message.static_account_keys() {
            if seen.insert(*key) {
                union.push(*key);
            }
        }
    }
    union
}

#[cfg_attr(feature = "hotpath", hotpath::measure)]
fn sanitize_transaction_for_batch<T: SvmTypesHost>(
    vm: &Svm<'_, T>,
    tx: VersionedTransaction,
) -> Result<SanitizedTransaction, BatchSanitizeError> {
    let result = if vm.version.sigverify {
        vm.sanitize_transaction(tx)
    } else {
        vm.sanitize_transaction_no_verify(tx)
    };

    result.map_err(|execution| match execution.fatal_error {
        Some(ExecutionError::AccountSource { pubkey, source }) => {
            BatchSanitizeError::AccountSource { pubkey, source }
        }
        _ => BatchSanitizeError::Transaction(
            execution.tx_result.err().unwrap_or(TransactionError::SanitizeFailure),
        ),
    })
}

/// Execute every transaction in a conflict-free stage in parallel.
///
/// Uses Rayon's work-stealing scheduler rather than a static chunk split.
/// Compute-unit cost varies by orders of magnitude between transactions (a bare
/// `SystemProgram::transfer` versus a deeply nested CPI), so an even split of
/// indexes across workers leaves most threads idle while the unluckiest chunk
/// finishes. Work stealing keeps every worker busy until the stage drains.
///
/// Whether a parallel stage would drop the installed inspector's hooks.
///
/// The parallel workers run on inspector-less worker VMs, so an inspector
/// never observes the transactions of a parallel stage. The condition is a
/// pure function of the VM's inspector so it can be asserted directly
/// instead of only being reported through a log line.
///
/// The check deliberately does **not** consult the inspector: the VM
/// consults `accepts_origin` exactly once per execution, and an extra
/// consultation would be observable to an origin-recording inspector.
fn parallel_stage_drops_inspector<T: SvmTypesHost>(vm: &Svm<'_, T>) -> bool {
    vm.has_inspector()
}

/// Execute every transaction in a conflict-free stage in parallel.
///
/// Uses Rayon's work-stealing scheduler rather than a static chunk split.
/// Compute-unit cost varies by orders of magnitude between transactions (a bare
/// `SystemProgram::transfer` versus a deeply nested CPI), so an even split of
/// indexes across workers leaves most threads idle while the unluckiest chunk
/// finishes. Work stealing keeps every worker busy until the stage drains.
///
/// [`rayon::iter::ParallelIterator::map_init`] builds the per-worker [`Svm`]
/// clone lazily, once per Rayon worker thread rather than once per transaction,
/// which preserves the amortized setup cost of the previous chunked design.
#[cfg_attr(feature = "hotpath", hotpath::measure)]
fn execute_transaction_batch_stage<'a, T: SvmTypesHost>(
    vm: &Svm<'a, T>,
    stage_index: usize,
    stage: &TransactionBatchStage,
    transactions: &[VersionedTransaction],
) -> Vec<BatchStageResult> {
    let snapshot = BatchExecutionSnapshot::from_vm(vm);

    // Parallel workers run with no inspector: the boxed inspector cannot be
    // cloned, and sharing one box across Rayon threads would serialize
    // execution on its lock. Only the serial single-transaction stage path
    // (which runs on the live VM) fires hooks.
    //
    // Silently dropping hooks would make an installed inspector observe a
    // different number of transactions depending on how the scheduler
    // happened to split the batch, so the loss is reported explicitly.
    if parallel_stage_drops_inspector(vm) {
        tracing::warn!(
            stage_index,
            "an inspector is installed; parallel batch stages execute on worker VMs that carry no \
             inspector, so those transactions are not observed by it — use single-transaction \
             stages to observe every transaction"
        );
    }

    stage
        .transaction_indexes
        .par_iter()
        .map_init(
            || {
                worker_vm(
                    vm,
                    snapshot.runtime.clone(),
                    // The origin's `transaction_index` is overwritten per
                    // transaction below; the stage index is what matters here.
                    TransactionOrigin::Batch { stage_index, transaction_index: 0 },
                )
            },
            |local, &index| {
                local.extensions.inspection_origin =
                    TransactionOrigin::Batch { stage_index, transaction_index: index };
                let (result, delta) = crate::commit::detached_into_result_and_state_change_set(
                    local.transact_detached(transactions[index].clone()),
                    local.state.history.is_enabled(),
                );
                BatchStageResult { index, result, delta }
            },
        )
        .collect()
}

/// Construct a per-worker VM snapshot for parallel batch execution.
///
/// # Performance note
///
/// This clones the account overlay and `TransactionHistory` from the
/// `Arc`-wrapped runtime state, while the committed program cache is shared
/// through its `Arc` handle (no deep clone). For workloads with large account
/// sets the overlay clone can dominate batch setup time. A future
/// optimization would introduce COW (copy-on-write) semantics so that
/// read-only workers share backing storage and only fork on mutation.
///
/// The worker is built through [`Svm::clone_with_state`], the single place
/// the VM's field list is written down, so the worker can never drift from
/// the VM it is derived from.
fn worker_vm<'a, T: SvmTypesHost>(
    vm: &Svm<'a, T>,
    runtime: HpsvmRuntimeState,
    origin: TransactionOrigin,
) -> Svm<'a, T> {
    // Share the committed, immutable program cache across workers: each worker
    // still gets its own overlay (cloned above), but the committed cache is
    // shared via the handle, so a batch does not deep-clone the map on every
    // worker thread — only per-worker mutation would require a copy, and the
    // transaction execution below never commits back into the committed handle
    // (its scratch is drained before `execute_transaction_batch_stage`
    // returns).
    //
    // Parallel workers must be able to mutate the program cache *during*
    // execution (agave's `process_message` requires `&mut` and replenishes
    // scratch entries), but those mutations must NOT be shared between
    // workers or with the live VM: they are transaction-scratch only and get
    // drained before `execute_transaction_batch_stage` returns. The committed
    // cache is still shared across VM clones via the handle everywhere else,
    // so this deep clone happens on the worker-setup path (amortised per
    // Rayon worker thread, not per transaction), preserving `#[inline]`-level
    // sharing on the single-transaction hot path. Reverting this to
    // `Arc::clone` makes the parallel-stage test regress to a single thread.
    let state = crate::SvmState {
        accounts: runtime.accounts.clone_for_worker(),
        history: (*runtime.history).clone(),
    };
    vm.clone_with_state(state, origin)
}

#[derive(Default)]
struct ScheduledTransactionBatchStage {
    transaction_indexes: Vec<usize>,
    lock_set: TransactionLockSet,
}

struct BatchStageResult {
    index: usize,
    result: TransactionResult,
    delta: StateChangeSet,
}

impl BatchStageResult {
    #[cfg_attr(feature = "hotpath", hotpath::measure)]
    fn new<T: SvmTypesHost>(
        stage_index: usize,
        index: usize,
        vm: &Svm<'_, T>,
        snapshot: BatchExecutionSnapshot,
        tx: VersionedTransaction,
    ) -> Self {
        let local = worker_vm(
            vm,
            snapshot.runtime,
            TransactionOrigin::Batch { stage_index, transaction_index: index },
        );
        let (result, delta) = crate::commit::detached_into_result_and_state_change_set(
            local.transact_detached(tx),
            local.state.history.is_enabled(),
        );

        Self { index, result, delta }
    }
}

#[derive(Default)]
struct TransactionLockSet {
    readonly: HashSet<Address>,
    writable: HashSet<Address>,
}

impl TransactionLockSet {
    fn from_transaction(tx: &SanitizedTransaction, versioned_message: &VersionedMessage) -> Self {
        let message = tx.message();
        let mut lock_set = Self::default();

        for (index, key) in message.account_keys().iter().enumerate() {
            if message.is_writable(index) {
                lock_set.writable.insert(*key);
            } else {
                lock_set.readonly.insert(*key);
            }
        }

        if let Some(lookups) = versioned_message.address_table_lookups() {
            lock_set.readonly.extend(lookups.iter().map(|lookup| lookup.account_key));
        }

        lock_set
    }

    fn conflicts_with(&self, other: &Self) -> bool {
        self.writable.iter().any(|key| other.writable.contains(key) || other.readonly.contains(key)) ||
            self.readonly.iter().any(|key| other.writable.contains(key))
    }

    /// The number of accounts this lock set covers.
    ///
    /// Used as the "size" of a stage when the planner picks the tightest
    /// conflict-free stage for a transaction.
    fn len(&self) -> usize {
        self.writable.len() + self.readonly.len()
    }

    fn extend(&mut self, other: &Self) {
        self.readonly.extend(other.readonly.iter().copied());
        self.writable.extend(other.writable.iter().copied());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use solana_account::{Account, AccountSharedData, WritableAccount};
    use solana_address::Address;
    use solana_address_lookup_table_interface::instruction::{
        create_lookup_table, extend_lookup_table,
    };
    use solana_keypair::Keypair;
    use solana_message::{
        AddressLookupTableAccount, Message, VersionedMessage, v0::Message as MessageV0,
    };
    use solana_signature::Signature;
    use solana_signer::Signer;
    use solana_system_interface::instruction::transfer;
    use solana_transaction::{Transaction, versioned::VersionedTransaction};
    use solana_transaction_error::TransactionError;

    use super::*;
    use crate::{
        BaseSvmTypes, DetachedTx, Inspector, StateChangeSet, Svm, TransactionOrigin, TxHandler,
        TxTag, apply_commit_delta, types::TransactionMetadata,
    };

    /// A handler that counts the transactions it executes and then runs the
    /// default execution path.
    struct CountingHandler {
        counter: Arc<AtomicUsize>,
    }

    impl<T: SvmTypesHost> TxHandler<T, VersionedTransaction, DetachedTx> for CountingHandler {
        fn validate(
            &self,
            _tx: &VersionedTransaction,
            _host: &Svm<'_, T>,
        ) -> Result<(), crate::error::TxValidationError> {
            Ok(())
        }

        fn execute(
            &self,
            tx: VersionedTransaction,
            host: &Svm<'_, T>,
        ) -> Result<DetachedTx, crate::error::ExecutionError> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            Ok(host.default_execute_detached(tx))
        }
    }

    /// An inspector that observes every origin.
    struct AcceptAllInspector;

    impl Inspector<BaseSvmTypes> for AcceptAllInspector {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }

        fn accepts_origin(&self, _origin: TransactionOrigin) -> bool {
            true
        }
    }

    /// The batch workers are built from the live VM, so a handler registered
    /// on the VM is the one that runs inside the batch.
    #[test]
    fn batch_workers_inherit_the_live_vms_transaction_registry() {
        let mut svm = Svm::<BaseSvmTypes>::new();
        let blockhash = svm.latest_blockhash();

        let counter = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&counter);
        svm.register_tx_handler(TxTag::Legacy, CountingHandler { counter: observed });

        let senders: Vec<Keypair> = (0..4).map(|_| Keypair::new()).collect();
        for sender in &senders {
            svm.airdrop(&sender.pubkey(), 1_000_000_000).unwrap();
        }
        let transactions: Vec<VersionedTransaction> = senders
            .iter()
            .map(|sender| {
                let ix = transfer(&sender.pubkey(), &Address::new_unique(), 1);
                let msg = Message::new_with_blockhash(&[ix], Some(&sender.pubkey()), &blockhash);
                VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[sender]).unwrap()
            })
            .collect();

        // The airdrops above already went through the handler; only the batch
        // transactions are counted from here on.
        let before = counter.load(Ordering::SeqCst);
        svm.send_transaction_batch(transactions).unwrap();

        assert_eq!(
            counter.load(Ordering::SeqCst) - before,
            4,
            "every batch transaction must run through the VM's registered handler"
        );
    }

    /// A parallel stage drops the installed inspector's hooks, and the
    /// condition is reported instead of being silently swallowed.
    #[test]
    fn a_parallel_stage_reports_that_it_drops_the_inspector() {
        let mut svm = Svm::<BaseSvmTypes>::new();

        // Without an inspector there is nothing to drop.
        assert!(
            !parallel_stage_drops_inspector(&svm),
            "a VM without an inspector has nothing to drop"
        );

        // With one, the parallel stage cannot carry it onto the workers.
        svm.set_inspector(AcceptAllInspector);
        assert!(
            parallel_stage_drops_inspector(&svm),
            "an installed inspector must be reported as dropped"
        );
    }

    #[test]
    fn commit_delta_merges_runtime_updates() {
        let address = Address::new_unique();
        let signature = Signature::default();
        let mut accounts = AccountsDb::default();
        let mut history = TransactionHistory::new();
        let mut before = AccountSharedData::default();
        before.set_lamports(5);
        accounts.add_account_no_checks(address, before);

        let mut after = AccountSharedData::default();
        after.set_lamports(9);
        let history_entry = TransactionResult::Ok(TransactionMetadata {
            signature,
            fee: 5000,
            ..Default::default()
        });
        let delta = StateChangeSet {
            post_accounts: vec![(address, after.clone())],
            history_entry: Some((signature, history_entry.clone())),
        };

        apply_commit_delta(&mut accounts, &mut history, delta)
            .expect("commit delta merge should apply valid state");

        assert_eq!(accounts.get_account(&address), Some(after));
        assert_eq!(history.get_transaction(&signature), Some(&history_entry));
    }

    #[test]
    fn batch_stage_result_returns_transaction_error_when_lookup_table_becomes_unsanitizable() {
        let mut svm = Svm::<BaseSvmTypes>::new();
        let authority = Keypair::new();
        let lookup_user = Keypair::new();
        let authority_pk = authority.pubkey();
        let lookup_user_pk = lookup_user.pubkey();
        let recipient = Address::new_unique();

        svm.airdrop(&authority_pk, 1_000_000_000).unwrap();
        svm.airdrop(&lookup_user_pk, 1_000_000_000).unwrap();

        let setup_blockhash = svm.latest_blockhash();
        let (create_lookup_ix, lookup_table_address) =
            create_lookup_table(authority_pk, authority_pk, 0);
        let extend_lookup_ix = extend_lookup_table(
            lookup_table_address,
            authority_pk,
            Some(authority_pk),
            vec![recipient],
        );
        let setup_lookup_tx = Transaction::new(
            &[&authority],
            Message::new(&[create_lookup_ix, extend_lookup_ix], Some(&authority_pk)),
            setup_blockhash,
        );
        svm.send_transaction(setup_lookup_tx).unwrap();
        svm.warp_to_slot(1);

        let stage_blockhash = svm.latest_blockhash();
        let lookup_table =
            AddressLookupTableAccount { key: lookup_table_address, addresses: vec![recipient] };
        let lookup_message = MessageV0::try_compile(
            &lookup_user_pk,
            &[transfer(&lookup_user_pk, &recipient, 1)],
            &[lookup_table],
            stage_blockhash,
        )
        .unwrap();
        let lookup_tx =
            VersionedTransaction::try_new(VersionedMessage::V0(lookup_message), &[&lookup_user])
                .unwrap();

        assert!(sanitize_transaction_for_batch(&svm, lookup_tx.clone()).is_ok());

        svm.set_account(lookup_table_address, Account::default()).unwrap();

        let stage_result =
            BatchStageResult::new(1, 0, &svm, BatchExecutionSnapshot::from_vm(&svm), lookup_tx);

        assert_eq!(
            stage_result.result.unwrap_err().err,
            TransactionError::AddressLookupTableNotFound
        );
        assert!(!stage_result.delta.mutates_state());
    }

    #[test]
    fn batch_stage_preserves_per_transaction_origin_under_work_stealing() {
        let mut svm = Svm::<BaseSvmTypes>::new();
        let blockhash = svm.latest_blockhash();

        let senders: Vec<Keypair> = (0..8).map(|_| Keypair::new()).collect();
        for sender in &senders {
            svm.airdrop(&sender.pubkey(), 1_000_000_000).unwrap();
        }

        let transactions: Vec<VersionedTransaction> = senders
            .iter()
            .map(|sender| {
                let ix = transfer(&sender.pubkey(), &Address::new_unique(), 1);
                let msg = Message::new_with_blockhash(&[ix], Some(&sender.pubkey()), &blockhash);
                VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[sender]).unwrap()
            })
            .collect();

        let plan = plan_transaction_batch(&svm, &transactions).unwrap();
        let stage = plan.stages.first().expect("conflict-free txs share one stage");
        assert_eq!(stage.transaction_indexes.len(), transactions.len());

        let results = execute_transaction_batch_stage(&svm, 0, stage, &transactions);

        // Work stealing returns results out of order, but every index must be
        // present exactly once and carry its own successful outcome.
        let mut indexes: Vec<usize> = results.iter().map(|result| result.index).collect();
        indexes.sort_unstable();
        assert_eq!(indexes, (0..transactions.len()).collect::<Vec<_>>());
        assert!(results.iter().all(|result| result.result.is_ok()));
    }

    fn transfer_tx(
        payer: &Keypair,
        recipient: &Address,
        lamports: u64,
        blockhash: solana_hash::Hash,
    ) -> VersionedTransaction {
        let instruction = transfer(&payer.pubkey(), recipient, lamports);
        let message =
            Message::new_with_blockhash(&[instruction], Some(&payer.pubkey()), &blockhash);
        VersionedTransaction::try_new(VersionedMessage::Legacy(message), &[payer])
            .expect("transaction should build")
    }

    /// A transaction that writes two accounts, so its lock set is wider than
    /// a single transfer's.
    fn two_recipient_transfer_tx(
        payer: &Keypair,
        recipients: &[Address],
        blockhash: solana_hash::Hash,
    ) -> VersionedTransaction {
        let instructions: Vec<_> =
            recipients.iter().map(|recipient| transfer(&payer.pubkey(), recipient, 1)).collect();
        let message = Message::new_with_blockhash(&instructions, Some(&payer.pubkey()), &blockhash);
        VersionedTransaction::try_new(VersionedMessage::Legacy(message), &[payer])
            .expect("transaction should build")
    }

    /// The lock set's size counts every account it locks, whether the
    /// account is read or written.
    #[test]
    fn the_lock_set_size_counts_every_locked_account() {
        let mut lock_set = TransactionLockSet::default();
        lock_set.writable.insert(Address::new_unique());
        lock_set.writable.insert(Address::new_unique());
        lock_set.readonly.insert(Address::new_unique());

        assert_eq!(lock_set.len(), 3);
    }

    /// The planner fills the tightest conflict-free stage, not the first
    /// one, so the heavily locked stages stay free for the transactions that
    /// need them.
    ///
    /// The first stage locks three accounts (the payer and two recipients)
    /// and the second one two, so a transaction that fits in both belongs in
    /// the second.
    #[test]
    fn the_planner_fills_the_tightest_conflict_free_stage() {
        let mut svm = Svm::<BaseSvmTypes>::new();
        let blockhash = svm.latest_blockhash();

        let payer_a = Keypair::new();
        let payer_b = Keypair::new();
        for payer in [&payer_a, &payer_b] {
            svm.airdrop(&payer.pubkey(), 1_000_000_000).unwrap();
        }

        // The first transaction locks the payer and two recipients; the
        // second one locks the payer and one of them, so it cannot share the
        // first stage.
        let shared = Address::new_unique();
        let transactions = vec![
            two_recipient_transfer_tx(&payer_a, &[shared, Address::new_unique()], blockhash),
            transfer_tx(&payer_a, &shared, 1, blockhash),
            transfer_tx(&payer_b, &Address::new_unique(), 1, blockhash),
        ];

        let plan = plan_transaction_batch(&svm, &transactions).unwrap();

        assert_eq!(plan.stages.len(), 2);
        // The third transaction fits in both stages, and the tighter second
        // one wins.
        assert_eq!(plan.stages[0].transaction_indexes, vec![0]);
        assert_eq!(plan.stages[1].transaction_indexes, vec![1, 2]);
    }

    /// Two stages that lock the same number of accounts are equally tight, so
    /// the earlier one wins.
    #[test]
    fn the_planner_keeps_the_first_stage_on_a_tie() {
        let mut svm = Svm::<BaseSvmTypes>::new();
        let blockhash = svm.latest_blockhash();

        // The first two transactions write the same account, so they land in
        // two stages that each lock the same number of accounts.
        let payer_a = Keypair::new();
        let payer_b = Keypair::new();
        for payer in [&payer_a, &payer_b] {
            svm.airdrop(&payer.pubkey(), 1_000_000_000).unwrap();
        }
        let shared = Address::new_unique();
        let transactions = vec![
            transfer_tx(&payer_a, &shared, 1, blockhash),
            transfer_tx(&payer_a, &shared, 2, blockhash),
            transfer_tx(&payer_b, &Address::new_unique(), 3, blockhash),
        ];

        let plan = plan_transaction_batch(&svm, &transactions).unwrap();

        assert_eq!(plan.stages.len(), 2);
        // The third transaction fits in both stages, and the equally tight
        // earlier stage wins.
        assert_eq!(plan.stages[0].transaction_indexes, vec![0, 2]);
        assert_eq!(plan.stages[1].transaction_indexes, vec![1]);
    }

    #[test]
    fn batch_ten_conflict_free_transactions_produce_correct_balances() {
        let mut svm = Svm::<BaseSvmTypes>::new();
        let blockhash = svm.latest_blockhash();

        let initial_balance: u64 = 1_000_000_000;
        let transfer_amount: u64 = 100;

        let mut senders = Vec::new();
        let mut recipients = Vec::new();

        for _ in 0..10 {
            let sender = Keypair::new();
            let recipient = Address::new_unique();
            svm.airdrop(&sender.pubkey(), initial_balance).unwrap();
            senders.push(sender);
            recipients.push(recipient);
        }

        let transactions: Vec<VersionedTransaction> = senders
            .iter()
            .zip(recipients.iter())
            .map(|(sender, recipient)| {
                let ix = transfer(&sender.pubkey(), recipient, transfer_amount);
                let msg = Message::new_with_blockhash(&[ix], Some(&sender.pubkey()), &blockhash);
                VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[sender]).unwrap()
            })
            .collect();

        let result = svm.send_transaction_batch(transactions).unwrap();

        assert_eq!(result.results.len(), 10);

        for tx_result in &result.results {
            assert!(tx_result.is_ok(), "all transactions should succeed");
        }

        let fee = 5000u64;
        for (sender, recipient) in senders.iter().zip(recipients.iter()) {
            let sender_balance = svm.get_balance(&sender.pubkey()).unwrap();
            let recipient_balance = svm.get_balance(recipient).unwrap();

            assert_eq!(
                sender_balance,
                initial_balance - transfer_amount - fee,
                "sender should have initial - transfer - fee"
            );
            assert_eq!(
                recipient_balance, transfer_amount,
                "recipient should have just the transfer amount"
            );
        }
    }
}
