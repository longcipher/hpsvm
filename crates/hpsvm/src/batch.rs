use std::{collections::HashSet, thread};

use solana_address::Address;
use solana_message::VersionedMessage;
use solana_transaction::{sanitized::SanitizedTransaction, versioned::VersionedTransaction};
use solana_transaction_error::TransactionError;
use thiserror::Error;

use crate::{
    CommitDelta, HPSVM, accounts_db::AccountsDb, apply_commit_delta, history::TransactionHistory,
    next_vm_instance_id, outcome_into_result_and_delta, types::TransactionResult,
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

#[cfg_attr(feature = "hotpath", hotpath::measure)]
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

#[cfg_attr(feature = "hotpath", hotpath::measure)]
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
            let mutates_state = stage_result.delta.mutates_state();
            apply_commit_delta(&mut vm.accounts, &mut vm.history, stage_result.delta)
                .expect("batch stage merge should only apply valid account states");
            if mutates_state {
                vm.invalidate_execution_outcomes();
            }
            results[stage_result.index] = Some(stage_result.result);
        }
    }

    let results = results
        .into_iter()
        .map(|result| result.expect("each batch result slot should be filled exactly once"))
        .collect();
    Ok(TransactionBatchExecutionResult { plan, results })
}

#[cfg_attr(feature = "hotpath", hotpath::measure)]
fn sanitize_transaction_for_batch(
    vm: &HPSVM,
    tx: VersionedTransaction,
) -> Result<SanitizedTransaction, TransactionError> {
    if vm.cfg.sigverify {
        vm.sanitize_transaction_inner(tx)
    } else {
        vm.sanitize_transaction_no_verify_inner(tx)
    }
}

#[cfg_attr(feature = "hotpath", hotpath::measure)]
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

fn worker_vm(vm: &HPSVM, runtime: HpsvmRuntimeState) -> HPSVM {
    HPSVM {
        accounts: runtime.accounts,
        airdrop_kp: vm.airdrop_kp,
        builtins_loaded: vm.builtins_loaded,
        default_programs_loaded: vm.default_programs_loaded,
        spl_programs_loaded: vm.spl_programs_loaded,
        cfg: vm.cfg.clone(),
        feature_accounts_loaded: vm.feature_accounts_loaded,
        inspector: vm.inspector.clone(),
        reserved_account_keys: vm.reserved_account_keys.clone(),
        runtime_registry: vm.runtime_registry.clone(),
        instance_id: next_vm_instance_id(),
        state_version: vm.state_version,
        block_env: vm.block_env,
        history: runtime.history,
        runtime_env: vm.runtime_env,
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
    delta: CommitDelta,
}

impl BatchStageResult {
    #[cfg_attr(feature = "hotpath", hotpath::measure)]
    fn new(
        index: usize,
        vm: &HPSVM,
        snapshot: BatchExecutionSnapshot,
        tx: VersionedTransaction,
    ) -> Self {
        let local = worker_vm(vm, snapshot.runtime);
        let (result, delta) = outcome_into_result_and_delta(local.transact(tx));

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

    fn extend(&mut self, other: &Self) {
        self.readonly.extend(other.readonly.iter().copied());
        self.writable.extend(other.writable.iter().copied());
    }
}

#[cfg(test)]
mod tests {
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
    use crate::{CommitDelta, HPSVM, apply_commit_delta, types::TransactionMetadata};

    #[test]
    fn commit_delta_merges_runtime_updates() {
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
        let delta = CommitDelta::new(
            vec![(address, after.clone())],
            Some((signature, history_entry.clone())),
        );

        apply_commit_delta(&mut runtime.accounts, &mut runtime.history, delta)
            .expect("commit delta merge should apply valid state");

        assert_eq!(runtime.accounts.get_account(&address), Some(after));
        assert_eq!(runtime.history.get_transaction(&signature), Some(&history_entry));
    }

    #[test]
    fn batch_stage_result_returns_transaction_error_when_lookup_table_becomes_unsanitizable() {
        let mut svm = HPSVM::new();
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
            BatchStageResult::new(1, &svm, BatchExecutionSnapshot::from_vm(&svm), lookup_tx);

        assert_eq!(
            stage_result.result.unwrap_err().err,
            TransactionError::AddressLookupTableNotFound
        );
        assert!(!stage_result.delta.mutates_state());
    }
}
