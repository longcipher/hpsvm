use std::{collections::HashSet, thread};

use solana_account::AccountSharedData;
use solana_address::Address;
use solana_message::VersionedMessage;
use solana_signature::Signature;
use solana_transaction::{sanitized::SanitizedTransaction, versioned::VersionedTransaction};
use solana_transaction_error::TransactionError;
use thiserror::Error;

use crate::{
    HPSVM, accounts_db::AccountsDb, error::HPSVMError, history::TransactionHistory,
    types::TransactionResult,
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
pub enum TransactionBatchError {
    /// The transaction could not be sanitized for scheduling.
    #[error("failed to sanitize transaction #{index} for batch scheduling: {source}")]
    Sanitize {
        /// Original transaction index in the submitted batch.
        index: usize,
        /// Underlying transaction sanitization error.
        source: TransactionError,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct HpsvmRuntimeState {
    accounts: AccountsDb,
    history: TransactionHistory,
}

impl Default for HpsvmRuntimeState {
    fn default() -> Self {
        Self { accounts: AccountsDb::default(), history: TransactionHistory::new() }
    }
}

impl HpsvmRuntimeState {
    fn from_vm(vm: &HPSVM) -> Self {
        Self { accounts: vm.accounts.clone(), history: vm.history.clone() }
    }
}

#[derive(Clone, Debug)]
struct BatchExecutionSnapshot {
    runtime: HpsvmRuntimeState,
}

impl BatchExecutionSnapshot {
    fn from_vm(vm: &HPSVM) -> Self {
        Self { runtime: HpsvmRuntimeState::from_vm(vm) }
    }
}

#[derive(Debug, Clone)]
struct BatchExecutionDelta {
    post_accounts: Vec<(Address, AccountSharedData)>,
    history_entry: Option<(Signature, TransactionResult)>,
}

impl BatchExecutionDelta {
    fn new(
        post_accounts: Vec<(Address, AccountSharedData)>,
        history_entry: Option<(Signature, TransactionResult)>,
    ) -> Self {
        Self { post_accounts, history_entry }
    }

    #[cfg(test)]
    fn merge_into(&self, runtime: &mut HpsvmRuntimeState) -> Result<(), HPSVMError> {
        apply_post_accounts(&mut runtime.accounts, &self.post_accounts)?;
        apply_history_entry(&mut runtime.history, &self.history_entry);
        Ok(())
    }

    fn merge_into_vm(&self, vm: &mut HPSVM) -> Result<(), HPSVMError> {
        apply_post_accounts(&mut vm.accounts, &self.post_accounts)?;
        apply_history_entry(&mut vm.history, &self.history_entry);
        Ok(())
    }
}

pub(crate) fn plan_transaction_batch(
    vm: &HPSVM,
    txs: &[VersionedTransaction],
) -> Result<TransactionBatchPlan, TransactionBatchError> {
    let mut stages = Vec::<ScheduledTransactionBatchStage>::new();

    for (index, tx) in txs.iter().enumerate() {
        let sanitized = sanitize_transaction_for_batch(vm, tx.clone())
            .map_err(|source| TransactionBatchError::Sanitize { index, source })?;
        let lock_set = TransactionLockSet::from_transaction(&sanitized, &tx.message);

        if let Some(stage) =
            stages.iter_mut().find(|stage| !stage.lock_set.conflicts_with(&lock_set))
        {
            stage.transaction_indexes.push(index);
            stage.lock_set.extend(&lock_set);
        } else {
            stages.push(ScheduledTransactionBatchStage {
                transaction_indexes: vec![index],
                lock_set,
            });
        }
    }

    Ok(TransactionBatchPlan {
        stages: stages
            .into_iter()
            .map(|stage| TransactionBatchStage { transaction_indexes: stage.transaction_indexes })
            .collect(),
    })
}

pub(crate) fn send_transaction_batch(
    vm: &mut HPSVM,
    transactions: Vec<VersionedTransaction>,
) -> Result<TransactionBatchExecutionResult, TransactionBatchError> {
    let plan = plan_transaction_batch(vm, &transactions)?;
    let mut results = vec![None; transactions.len()];

    for stage in &plan.stages {
        if stage.transaction_indexes.len() == 1 {
            let index = stage.transaction_indexes[0];
            results[index] = Some(vm.send_transaction(transactions[index].clone()));
            continue;
        }

        let mut stage_results = execute_transaction_batch_stage(vm, stage, &transactions);
        stage_results.sort_by_key(|result| result.index);

        for stage_result in stage_results {
            stage_result
                .delta
                .merge_into_vm(vm)
                .expect("batch stage merge should only apply valid account states");
            results[stage_result.index] = Some(stage_result.result);
        }
    }

    let results = results
        .into_iter()
        .map(|result| result.expect("each batch result slot should be filled exactly once"))
        .collect();
    Ok(TransactionBatchExecutionResult { plan, results })
}

fn sanitize_transaction_for_batch(
    vm: &HPSVM,
    tx: VersionedTransaction,
) -> Result<SanitizedTransaction, TransactionError> {
    if vm.sigverify {
        vm.sanitize_transaction_inner(tx)
    } else {
        vm.sanitize_transaction_no_verify_inner(tx)
    }
}

fn execute_transaction_batch_stage(
    vm: &HPSVM,
    stage: &TransactionBatchStage,
    transactions: &[VersionedTransaction],
) -> Vec<BatchStageResult> {
    let snapshot = BatchExecutionSnapshot::from_vm(vm);

    thread::scope(|scope| {
        let handles = stage
            .transaction_indexes
            .iter()
            .map(|&index| {
                let tx = transactions[index].clone();
                let snapshot = snapshot.clone();

                scope.spawn(move || BatchStageResult::new(index, vm, snapshot, tx))
            })
            .collect::<Vec<_>>();

        handles
            .into_iter()
            .map(|handle| handle.join().expect("transaction batch worker should not panic"))
            .collect()
    })
}

fn apply_post_accounts(
    accounts: &mut AccountsDb,
    post_accounts: &[(Address, AccountSharedData)],
) -> Result<(), HPSVMError> {
    accounts.sync_accounts(post_accounts.to_vec())
}

fn apply_history_entry(
    history: &mut TransactionHistory,
    history_entry: &Option<(Signature, TransactionResult)>,
) {
    if let Some((signature, entry)) = history_entry {
        history.add_new_transaction(*signature, entry.clone());
    }
}

fn worker_vm(vm: &HPSVM, runtime: HpsvmRuntimeState) -> HPSVM {
    HPSVM {
        accounts: runtime.accounts,
        airdrop_kp: vm.airdrop_kp,
        builtins_loaded: vm.builtins_loaded,
        custom_syscalls: vm.custom_syscalls.clone(),
        default_programs_loaded: vm.default_programs_loaded,
        feature_set: vm.feature_set.clone(),
        feature_accounts_loaded: vm.feature_accounts_loaded,
        reserved_account_keys: vm.reserved_account_keys.clone(),
        latest_blockhash: vm.latest_blockhash,
        history: runtime.history,
        compute_budget: vm.compute_budget,
        sigverify: vm.sigverify,
        blockhash_check: vm.blockhash_check,
        fee_structure: vm.fee_structure.clone(),
        log_bytes_limit: vm.log_bytes_limit,
        #[cfg(feature = "precompiles")]
        precompiles_loaded: vm.precompiles_loaded,
        sysvars_loaded: vm.sysvars_loaded,
        #[cfg(feature = "invocation-inspect-callback")]
        invocation_inspect_callback: vm.invocation_inspect_callback.clone(),
        #[cfg(feature = "invocation-inspect-callback")]
        enable_register_tracing: vm.enable_register_tracing,
    }
}

#[derive(Default)]
struct ScheduledTransactionBatchStage {
    transaction_indexes: Vec<usize>,
    lock_set: TransactionLockSet,
}

struct BatchStageResult {
    index: usize,
    result: TransactionResult,
    delta: BatchExecutionDelta,
}

impl BatchStageResult {
    fn new(
        index: usize,
        vm: &HPSVM,
        snapshot: BatchExecutionSnapshot,
        tx: VersionedTransaction,
    ) -> Self {
        let mut local = worker_vm(vm, snapshot.runtime);
        let sanitized = sanitize_transaction_for_batch(&local, tx.clone())
            .expect("planned batch transaction should remain sanitizable during execution");
        let signature = *sanitized.signature();
        let had_history_entry = local.history.get_transaction(&signature).is_some();
        let writable_accounts = sanitized
            .message()
            .account_keys()
            .iter()
            .enumerate()
            .filter_map(|(account_index, key)| {
                sanitized
                    .message()
                    .is_writable(account_index)
                    .then_some((*key, local.accounts.get_account(key)))
            })
            .collect::<Vec<_>>();

        let result = local.send_transaction(tx);
        let history_entry = if had_history_entry {
            None
        } else {
            local.history.get_transaction(&signature).cloned().map(|entry| (signature, entry))
        };
        let post_accounts = writable_accounts
            .into_iter()
            .filter_map(|(address, before)| {
                let after = local.accounts.get_account(&address);
                (before != after).then_some((address, after.unwrap_or_default()))
            })
            .collect();

        Self { index, result, delta: BatchExecutionDelta::new(post_accounts, history_entry) }
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

    fn extend(&mut self, other: &Self) {
        self.readonly.extend(other.readonly.iter().copied());
        self.writable.extend(other.writable.iter().copied());
    }
}

#[cfg(test)]
mod tests {
    use solana_account::{AccountSharedData, WritableAccount};
    use solana_address::Address;
    use solana_signature::Signature;

    use super::*;
    use crate::types::TransactionMetadata;

    #[test]
    fn batch_execution_delta_merges_runtime_updates() {
        let address = Address::new_unique();
        let signature = Signature::default();
        let mut runtime = HpsvmRuntimeState::default();
        let mut before = AccountSharedData::default();
        before.set_lamports(5);
        runtime.accounts.add_account_no_checks(address, before);

        let mut after = AccountSharedData::default();
        after.set_lamports(9);
        let history_entry = TransactionResult::Ok(TransactionMetadata {
            signature,
            fee: 5000,
            ..Default::default()
        });
        let delta = BatchExecutionDelta::new(
            vec![(address, after.clone())],
            Some((signature, history_entry.clone())),
        );

        delta.merge_into(&mut runtime).expect("batch delta merge should apply valid state");

        assert_eq!(runtime.accounts.get_account(&address), Some(after));
        assert_eq!(runtime.history.get_transaction(&signature), Some(&history_entry));
    }
}
