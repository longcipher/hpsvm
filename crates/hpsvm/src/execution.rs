use std::{cell::RefCell, rc::Rc};

use agave_feature_set::FeatureSet;
use solana_account::{AccountSharedData, ReadableAccount, WritableAccount};
use solana_address::Address;
use solana_compute_budget::compute_budget_limits::ComputeBudgetLimits;
use solana_fee::FeeFeatures;
use solana_program_runtime::invoke_context::{EnvironmentConfig, InvokeContext};
use solana_rent::Rent;
use solana_sdk_ids::native_loader;
use solana_svm_log_collector::LogCollector;
use solana_svm_timings::ExecuteTimings;
use solana_svm_transaction::svm_message::SVMStaticMessage;
use solana_transaction::{
    sanitized::{MessageHash, SanitizedTransaction},
    versioned::VersionedTransaction,
};
use solana_transaction_context::{DropOnBailOut, IndexOfAccount, transaction::TransactionContext};
use solana_transaction_error::TransactionError;

use crate::{
    FeatureGate, InspectMode, Svm, SvmTypesHost, TxFeatureFlags,
    account_source::AccountSourceError,
    accounts_db::AccountSourceTrackingAddressLoader,
    agave_compat::{AgaveMessage, to_agave_instruction},
    error::HPSVMError,
    helpers::execute_tx_helper,
    message_processor::process_message,
    types::{AccountSourceFailure, ExecutionResult},
    utils::{
        construct_instructions_account,
        rent::{check_rent_state_with_account, get_account_rent_state},
    },
};

struct CheckAndProcessTransactionSuccessCore<'ix_data> {
    result: Result<(), TransactionError>,
    compute_units_consumed: u64,
    context: Option<TransactionContext<'ix_data>>,
}

struct CheckAndProcessTransactionSuccess<'ix_data> {
    core: CheckAndProcessTransactionSuccessCore<'ix_data>,
    fee: u64,
    payer_key: Option<Address>,
    account_source_failures: Vec<AccountSourceFailure>,
    inspector_live: bool,
}

pub(crate) fn map_sanitize_result<F>(
    res: Result<SanitizedTransaction, ExecutionResult>,
    op: F,
) -> ExecutionResult
where
    F: FnOnce(SanitizedTransaction) -> ExecutionResult,
{
    match res {
        Ok(s_tx) => op(s_tx),
        Err(e) => e,
    }
}

fn execution_result_if_context(
    sanitized_tx: &SanitizedTransaction,
    ctx: TransactionContext<'_>,
    result: Result<(), TransactionError>,
    compute_units_consumed: u64,
    fee: u64,
    fee_payer: Option<Address>,
    account_source_failures: Vec<AccountSourceFailure>,
) -> ExecutionResult {
    let (signature, return_data, inner_instructions, execution_trace, post_accounts) =
        execute_tx_helper(sanitized_tx, ctx);
    ExecutionResult {
        tx_result: result,
        signature,
        post_accounts,
        inner_instructions,
        compute_units_consumed,
        return_data,
        execution_trace,
        included: true,
        fee,
        fee_payer,
        account_source_failures,
        fatal_error: None,
        // Set by the caller after construction (see
        // `execute_sanitized_transaction_impl`).
        inspector_live: false,
    }
}

#[cold]
fn execution_result_with_account_source_error(
    pubkey: Address,
    source: AccountSourceError,
    tx_error: TransactionError,
    fee: u64,
    context: &'static str,
) -> ExecutionResult {
    tracing::error!(?pubkey, %source, "{context}");

    ExecutionResult {
        tx_result: Err(tx_error),
        fee,
        account_source_failures: vec![AccountSourceFailure { pubkey, error: source.to_string() }],
        fatal_error: Some(HPSVMError::AccountSource { pubkey, source }),
        ..Default::default()
    }
}

#[cold]
fn sanitize_error_into_execution_result(
    loader: &AccountSourceTrackingAddressLoader<'_>,
    err: TransactionError,
) -> ExecutionResult {
    if let Some(failure) = loader.take_failure() {
        execution_result_with_account_source_error(
            failure.pubkey,
            failure.source,
            err,
            0,
            "failed to load address lookup table account from source",
        )
    } else {
        ExecutionResult { tx_result: Err(err), ..Default::default() }
    }
}

fn get_compute_budget_limits(
    sanitized_tx: &SanitizedTransaction,
    feature_set: &FeatureSet,
) -> Result<ComputeBudgetLimits, ExecutionResult> {
    // `process_compute_budget_instructions` is typed against the agave-4.5
    // (`solana-svm-transaction` 6.0.0) `SVMInstruction`, so each instruction is
    // re-wrapped on the way in (see `agave_compat`). The mapping iterator stays
    // `Clone` because the underlying message iterator is.
    solana_compute_budget_instruction::instructions_processor::process_compute_budget_instructions(
        sanitized_tx
            .message()
            .program_instructions_iter()
            .map(|(program_id, instruction)| (program_id, to_agave_instruction(instruction))),
        feature_set,
    )
    .map_err(|e| ExecutionResult { tx_result: Err(e), ..Default::default() })
}

fn get_transaction_account_lock_limit(flags: &TxFeatureFlags) -> usize {
    use solana_transaction::sanitized::MAX_TX_ACCOUNT_LOCKS;
    if flags.is_enabled(FeatureGate::IncreaseTxAccountLockLimit) {
        MAX_TX_ACCOUNT_LOCKS
    } else {
        64
    }
}

impl<T: SvmTypesHost> Svm<'_, T> {
    fn create_transaction_context(
        &self,
        compute_budget: solana_compute_budget::compute_budget::ComputeBudget,
        accounts: Vec<(Address, AccountSharedData)>,
        number_of_top_level_instructions: usize,
        rent: solana_rent::Rent,
    ) -> TransactionContext<'_> {
        // `DropOnBailOut::Disabled` matches how `solana-program-runtime` builds its
        // own contexts: an `InstructionError::BailOut` surfaces to the caller as an
        // error instead of silently dropping the transaction, which is what a
        // simulator needs so tests observe the failure.
        TransactionContext::new_with_feature_flags(
            accounts,
            rent,
            compute_budget.max_instruction_stack_depth,
            compute_budget.max_instruction_trace_length,
            number_of_top_level_instructions,
            DropOnBailOut::Disabled,
        )
    }

    #[cfg_attr(feature = "hotpath", hotpath::measure)]
    pub(crate) fn sanitize_transaction_no_verify(
        &self,
        tx: VersionedTransaction,
    ) -> Result<SanitizedTransaction, ExecutionResult> {
        let loader = AccountSourceTrackingAddressLoader::new(&self.accounts);
        SanitizedTransaction::try_create(
            tx,
            MessageHash::Compute,
            Some(false),
            &loader,
            &self.reserved_account_keys.active,
        )
        .map_err(|err| sanitize_error_into_execution_result(&loader, err))
    }

    pub(crate) fn sanitize_transaction(
        &self,
        tx: VersionedTransaction,
    ) -> Result<SanitizedTransaction, ExecutionResult> {
        // Planning / inspection callers have no in-flight flags; one load here
        // keeps the per-call consultation bounded. The execution entries below
        // load once and use `sanitize_transaction_with_flags` instead.
        let flags = TxFeatureFlags::load(&self.version.feature_set);
        self.sanitize_transaction_with_flags(tx, &flags)
    }

    pub(crate) fn sanitize_transaction_with_flags(
        &self,
        tx: VersionedTransaction,
        flags: &TxFeatureFlags,
    ) -> Result<SanitizedTransaction, ExecutionResult> {
        let tx = self.sanitize_transaction_no_verify(tx)?;

        tx.verify().map_err(|err| ExecutionResult { tx_result: Err(err), ..Default::default() })?;
        SanitizedTransaction::validate_account_locks(
            tx.message(),
            get_transaction_account_lock_limit(flags),
        )
        .map_err(|err| ExecutionResult { tx_result: Err(err), ..Default::default() })?;

        Ok(tx)
    }

    #[cfg_attr(feature = "hotpath", hotpath::measure)]
    fn process_transaction<'a, 'b, const MODE: InspectMode>(
        &'a self,
        tx: &'b SanitizedTransaction,
        // Agave-4.5 view of the same message: the runtime APIs below
        // (`calculate_fee`, `prepare_top_level_instructions`) are typed against
        // the older interface crates (see `agave_compat`). It borrows the
        // sanitized transaction, so it must outlive the returned transaction
        // context.
        agave_message: &'b AgaveMessage<'b>,
        compute_budget_limits: ComputeBudgetLimits,
        log_collector: Rc<RefCell<LogCollector>>,
        flags: &TxFeatureFlags,
    ) -> Result<CheckAndProcessTransactionSuccess<'b>, ExecutionResult>
    where
        'a: 'b,
    {
        let mut account_source_failures = Vec::new();
        let compute_budget = hotpath_block!("hpsvm::process_transaction::compute_budget", {
            self.version.compute_budget.unwrap_or_else(|| {
                solana_compute_budget::compute_budget::ComputeBudget {
                    compute_unit_limit: u64::from(compute_budget_limits.compute_unit_limit),
                    heap_size: compute_budget_limits.updated_heap_bytes,
                    ..solana_compute_budget::compute_budget::ComputeBudget::new_with_defaults(
                        flags.is_enabled(FeatureGate::RaiseCpiNestingLimitTo8),
                    )
                }
            })
        });
        let rent = hotpath_block!("hpsvm::process_transaction::load_rent", {
            self.accounts.sysvar_cache().get_rent()
        });
        // The rent sysvar is materialized by `set_sysvars` and guarded by
        // `require_sysvars_loaded` on the way in, but the account can still be
        // removed from the overlay afterwards. Report the missing component as
        // an execution failure instead of panicking.
        let rent = match rent {
            Ok(rent) => rent,
            Err(error) => {
                tracing::error!("rent sysvar is not materialized: {error}");
                return Err(ExecutionResult {
                    tx_result: Err(TransactionError::SanitizeFailure),
                    ..Default::default()
                });
            }
        };
        let message = tx.message();
        let blockhash = message.recent_blockhash();
        // Share the committed program cache: only the `Arc` handle is cloned
        // (a pointer bump) and the write lock is taken for the `&mut` the
        // `InvokeContext` API requires. The per-transaction `HashMap` deep
        // clone this replaced was the documented `clone_program_cache`
        // hot-path cost (REQ-02). The guard is held for the whole execution;
        // nothing under it re-locks the cache (the entry-presence check below
        // reads through the guard), and transaction-scratch state is drained
        // before the guard drops so only committed entries persist.
        let shared_program_cache = hotpath_block!(
            "hpsvm::process_transaction::shared_program_cache",
            self.accounts.shared_programs_cache()
        );
        let mut program_cache_guard = shared_program_cache.write();
        let mut accumulated_consume_units = 0;
        let account_keys = message.account_keys();
        let prioritization_fee = compute_budget_limits.get_prioritization_fee();
        let fee = hotpath_block!("hpsvm::process_transaction::calculate_fee", {
            solana_fee::calculate_fee(
                agave_message,
                self.version.fee_structure.lamports_per_signature,
                prioritization_fee,
                FeeFeatures {},
            )
        });
        let mut validated_fee_payer = false;
        let mut payer_key = None;
        let accounts = hotpath_block!("hpsvm::process_transaction::load_accounts", {
            let mut accounts = Vec::with_capacity(account_keys.len());

            for (i, key) in account_keys.iter().enumerate() {
                let account = if solana_sdk_ids::sysvar::instructions::check_id(key) {
                    construct_instructions_account(message)
                } else {
                    let is_instruction_account =
                        SVMStaticMessage::is_instruction_account(message, i);
                    // One load path for every account key.
                    //
                    // This used to have a second "read-only program account
                    // that is in the program cache" branch that called
                    // `get_account` and then `expect`ed the account to be
                    // present. `get_account` swallows account-source errors
                    // and returns `None`, so an unreachable source (or a
                    // source that does not know the account) turned a
                    // recoverable, classified failure into a panic and
                    // bypassed the fatal/non-fatal taxonomy entirely.
                    //
                    // `try_get_account` resolves the overlay first and the
                    // source second, so the fast path gained nothing over
                    // the general one — it only cost an extra program-cache
                    // lookup and a panic site.
                    let mut account = match self.accounts.try_get_account(key) {
                        Ok(Some(account)) => account,
                        Ok(None) => {
                            let mut default_account = AccountSharedData::default();
                            default_account.set_rent_epoch(0);
                            default_account
                        }
                        Err(error) => {
                            return Err(execution_result_with_account_source_error(
                                *key,
                                error,
                                TransactionError::AccountNotFound,
                                fee,
                                "failed to load transaction account from source",
                            ));
                        }
                    };

                    if !validated_fee_payer && (!message.is_invoked(i) || is_instruction_account) {
                        if let Err(error) = crate::validate_fee_payer(
                            key,
                            &mut account,
                            i as IndexOfAccount,
                            &rent,
                            fee,
                        ) {
                            return Err(ExecutionResult {
                                tx_result: Err(error),
                                compute_units_consumed: accumulated_consume_units,
                                fee,
                                ..Default::default()
                            });
                        }
                        validated_fee_payer = true;
                        payer_key = Some(*key);
                    }

                    account
                };

                accounts.push((*key, account));
            }

            Ok(accounts)
        })?;

        if !validated_fee_payer {
            tracing::error!("Failed to validate fee payer");
            return Err(ExecutionResult {
                tx_result: Err(TransactionError::AccountNotFound),
                compute_units_consumed: accumulated_consume_units,
                fee,
                ..Default::default()
            });
        }
        let program_indices = hotpath_block!(
            "hpsvm::process_transaction::resolve_program_indices",
            self.resolve_program_indices(
                tx,
                &accounts,
                fee,
                accumulated_consume_units,
                &mut account_source_failures,
            )
        )?;

        let mut context = hotpath_block!(
            "hpsvm::process_transaction::create_transaction_context",
            self.create_transaction_context(
                compute_budget,
                accounts,
                tx.message().instructions().len(),
                rent.as_ref().clone(),
            )
        );

        let rent_check = hotpath_block!(
            "hpsvm::process_transaction::check_accounts_rent",
            self.check_accounts_rent(tx, &context, &rent, &mut account_source_failures)
        );
        if let Err(mut error) = rent_check {
            error.compute_units_consumed = accumulated_consume_units;
            error.fee = fee;
            error.account_source_failures = account_source_failures;
            return Err(error);
        }

        // `runtime_features` is an agave API taking the full `FeatureSet`;
        // the hoisted static gates above already cover this transaction's
        // cached decisions, so no extra `is_active` consultation happens here.
        let svm_feature_set = self.version.feature_set.runtime_features();
        // Cloned before `log_collector` moves into the invoke context below:
        // the inspector `log` hooks drain through this handle after
        // instruction processing.
        let log_sink = Rc::clone(&log_collector);
        let mut invoke_context =
            hotpath_block!("hpsvm::process_transaction::build_invoke_context", {
                InvokeContext::new(
                    &mut context,
                    &mut program_cache_guard,
                    EnvironmentConfig::new(
                        *blockhash,
                        self.version.fee_structure.lamports_per_signature,
                        false,
                        self,
                        &svm_feature_set,
                        self.accounts.runtime_environments(),
                        self.accounts.sysvar_cache(),
                    ),
                    Some(log_collector),
                    compute_budget.to_budget(),
                    compute_budget.to_cost(),
                )
            });

        #[cfg(feature = "invocation-inspect-callback")]
        self.invocation_inspect_callback.before_invocation(
            self,
            tx,
            &program_indices,
            &invoke_context,
        );

        // The mode was selected once at the readonly entry
        // (`execute_readonly`): it already encodes whether the installed
        // inspector accepted the current origin, so no second consultation
        // happens here. Under `NoInspector` every dispatcher below resolves
        // to its empty arm at compile time.
        self.on_transaction_start::<MODE>(tx);

        let tx_result = hotpath_block!("hpsvm::process_transaction::process_message", {
            process_message::<MODE, T>(
                self,
                agave_message,
                &program_indices,
                &mut invoke_context,
                &mut ExecuteTimings::default(),
                &mut accumulated_consume_units,
            )
        });

        if MODE.is_live() {
            for message in log_sink.borrow().get_recorded_content() {
                self.on_log::<MODE>(message);
            }
        }

        self.on_transaction_end::<MODE>(&tx_result);

        #[cfg(feature = "invocation-inspect-callback")]
        self.invocation_inspect_callback.after_invocation(
            self,
            &invoke_context,
            self.enable_register_tracing,
        );

        // Discard transaction-scratch cache state: entries deployed or
        // loaded mid-transaction (`modified_entries`) plus the per-batch
        // flags. The pre-Arc code dropped a per-transaction clone here, so
        // scratch never reached the committed cache; draining preserves that
        // exactly. Committed updates still flow through account staging
        // (`sync_accounts` → `add_account` → `replenish`) and checkpoint
        // rollback (`rebuild_program_cache`), both of which run after this
        // guard is dropped.
        program_cache_guard.drain_modified_entries();
        program_cache_guard.hit_max_limit = false;
        program_cache_guard.loaded_missing = false;
        program_cache_guard.merged_modified = false;

        Ok(CheckAndProcessTransactionSuccess {
            core: CheckAndProcessTransactionSuccessCore {
                result: tx_result,
                compute_units_consumed: accumulated_consume_units,
                context: Some(context),
            },
            fee,
            payer_key,
            account_source_failures,
            // `MODE` is gone after this function returns, so its liveness is
            // recorded as a plain bool for the commit boundary (see
            // `ExecutionResult::inspector_live`).
            inspector_live: MODE.is_live(),
        })
    }

    /// Resolve the program account indices for each instruction in the transaction.
    ///
    /// For each top-level instruction, verifies that the referenced program is
    /// executable and owned by the native loader (directly or transitively).
    /// Loads missing owner accounts from the account source on demand.
    fn resolve_program_indices(
        &self,
        tx: &SanitizedTransaction,
        accounts: &[(Address, AccountSharedData)],
        fee: u64,
        accumulated_consume_units: u64,
        account_source_failures: &mut Vec<AccountSourceFailure>,
    ) -> Result<Vec<IndexOfAccount>, ExecutionResult> {
        let mut program_indices = Vec::with_capacity(tx.message().instructions().len());

        for compiled_instruction in tx.message().instructions() {
            let program_index = compiled_instruction.program_id_index as usize;
            let (program_id, program_account) =
                accounts.get(program_index).expect("program account should exist");
            if native_loader::check_id(program_id) {
                program_indices.push(program_index as IndexOfAccount);
                continue;
            }
            if !program_account.executable() {
                tracing::error!("Program account {program_id} is not executable.");
                return Err(ExecutionResult {
                    tx_result: Err(TransactionError::InvalidProgramForExecution),
                    compute_units_consumed: accumulated_consume_units,
                    fee,
                    ..Default::default()
                });
            }

            let owner_id = program_account.owner();
            if native_loader::check_id(owner_id) {
                program_indices.push(program_index as IndexOfAccount);
                continue;
            }

            // The owner account must exist, be owned by the native loader,
            // and be executable. It is validated here but intentionally NOT
            // added to the transaction context: agave requires the context
            // account set to match the message account keys exactly.
            let owner_account = match self.accounts.try_get_account(owner_id) {
                Ok(Some(account)) => account,
                Ok(None) => {
                    return Err(ExecutionResult {
                        tx_result: Err(TransactionError::ProgramAccountNotFound),
                        compute_units_consumed: accumulated_consume_units,
                        fee,
                        ..Default::default()
                    });
                }
                Err(error) => {
                    account_source_failures
                        .push(AccountSourceFailure { pubkey: *owner_id, error: error.to_string() });
                    AccountSharedData::default()
                }
            };
            if !native_loader::check_id(owner_account.owner()) {
                tracing::error!(
                    "Owner account {owner_id} is not owned by the native loader program."
                );
                return Err(ExecutionResult {
                    tx_result: Err(TransactionError::InvalidProgramForExecution),
                    compute_units_consumed: accumulated_consume_units,
                    fee,
                    account_source_failures: account_source_failures.clone(),
                    ..Default::default()
                });
            }
            if !owner_account.executable() {
                tracing::error!("Owner account {owner_id} is not executable");
                return Err(ExecutionResult {
                    tx_result: Err(TransactionError::InvalidProgramForExecution),
                    compute_units_consumed: accumulated_consume_units,
                    fee,
                    account_source_failures: account_source_failures.clone(),
                    ..Default::default()
                });
            }

            program_indices.push(program_index as IndexOfAccount);
        }

        Ok(program_indices)
    }

    fn check_accounts_rent(
        &self,
        tx: &SanitizedTransaction,
        context: &TransactionContext<'_>,
        rent: &Rent,
        account_source_failures: &mut Vec<AccountSourceFailure>,
    ) -> Result<(), ExecutionResult> {
        let message = tx.message();
        for index in 0..message.account_keys().len() {
            if message.is_writable(index) {
                let account =
                    context.accounts().try_borrow(index as IndexOfAccount).map_err(|err| {
                        ExecutionResult {
                            tx_result: Err(TransactionError::InstructionError(index as u8, err)),
                            ..Default::default()
                        }
                    })?;

                let pubkey = context.get_key_of_account_at_index(index as IndexOfAccount).map_err(
                    |err| ExecutionResult {
                        tx_result: Err(TransactionError::InstructionError(index as u8, err)),
                        ..Default::default()
                    },
                )?;

                let post_rent_state =
                    get_account_rent_state(rent, account.lamports(), account.data().len());
                let pre_rent_state = match self.accounts.try_get_account(pubkey) {
                    Ok(Some(acc)) => get_account_rent_state(rent, acc.lamports(), acc.data().len()),
                    Ok(None) => crate::utils::rent::RentState::Uninitialized,
                    Err(error) => {
                        account_source_failures.push(AccountSourceFailure {
                            pubkey: *pubkey,
                            error: error.to_string(),
                        });
                        crate::utils::rent::RentState::Uninitialized
                    }
                };

                check_rent_state_with_account(
                    &pre_rent_state,
                    &post_rent_state,
                    pubkey,
                    index as IndexOfAccount,
                )
                .map_err(|error| ExecutionResult { tx_result: Err(error), ..Default::default() })?;
            }
        }
        Ok(())
    }

    pub(crate) fn execute_sanitized_transaction<const MODE: InspectMode>(
        &self,
        sanitized_tx: &SanitizedTransaction,
        log_collector: Rc<RefCell<LogCollector>>,
        flags: &TxFeatureFlags,
    ) -> ExecutionResult {
        self.execute_sanitized_transaction_impl::<MODE>(sanitized_tx, log_collector, flags)
    }

    fn execute_sanitized_transaction_impl<const MODE: InspectMode>(
        &self,
        sanitized_tx: &SanitizedTransaction,
        log_collector: Rc<RefCell<LogCollector>>,
        flags: &TxFeatureFlags,
    ) -> ExecutionResult {
        // Agave-4.5 view of the sanitized message (see `agave_compat`). Created
        // here so the borrow outlives the transaction context returned from
        // `check_and_process_transaction`, whose instruction data is borrowed
        // through this view.
        let agave_message = AgaveMessage::new(sanitized_tx.message());
        let CheckAndProcessTransactionSuccess {
            core: CheckAndProcessTransactionSuccessCore { result, compute_units_consumed, context },
            fee,
            payer_key,
            account_source_failures,
            inspector_live,
        } = match self.check_and_process_transaction::<MODE>(
            sanitized_tx,
            &agave_message,
            log_collector,
            flags,
        ) {
            Ok(value) => value,
            Err(value) => return value,
        };
        if let Some(ctx) = context {
            let mut outcome = execution_result_if_context(
                sanitized_tx,
                ctx,
                result,
                compute_units_consumed,
                fee,
                payer_key,
                account_source_failures,
            );
            outcome.inspector_live = inspector_live;
            outcome
        } else {
            ExecutionResult {
                tx_result: result,
                compute_units_consumed,
                fee,
                account_source_failures,
                inspector_live,
                ..Default::default()
            }
        }
    }

    fn check_and_process_transaction<'a, 'b, const MODE: InspectMode>(
        &'a self,
        sanitized_tx: &'b SanitizedTransaction,
        agave_message: &'b AgaveMessage<'b>,
        log_collector: Rc<RefCell<LogCollector>>,
        flags: &TxFeatureFlags,
    ) -> Result<CheckAndProcessTransactionSuccess<'b>, ExecutionResult>
    where
        'a: 'b,
    {
        if self.require_sysvars_loaded().is_err() {
            return Err(ExecutionResult {
                tx_result: Err(TransactionError::SanitizeFailure),
                ..Default::default()
            });
        }
        self.maybe_blockhash_check(sanitized_tx)?;
        // `process_compute_budget_instructions` is an agave API taking the full
        // `FeatureSet`; it cannot take `TxFeatureFlags` and performs no
        // per-gate `is_active` consultation attributable to this crate.
        let compute_budget_limits =
            get_compute_budget_limits(sanitized_tx, &self.version.feature_set)?;
        self.maybe_history_check(sanitized_tx)?;
        self.process_transaction::<MODE>(
            sanitized_tx,
            agave_message,
            compute_budget_limits,
            log_collector,
            flags,
        )
    }

    fn maybe_history_check(
        &self,
        sanitized_tx: &SanitizedTransaction,
    ) -> Result<(), ExecutionResult> {
        if self.version.sigverify && self.history.check_transaction(sanitized_tx.signature()) {
            return Err(ExecutionResult {
                tx_result: Err(TransactionError::AlreadyProcessed),
                ..Default::default()
            });
        }
        Ok(())
    }

    fn maybe_blockhash_check(
        &self,
        sanitized_tx: &SanitizedTransaction,
    ) -> Result<(), ExecutionResult> {
        if self.version.blockhash_check {
            self.check_transaction_age(sanitized_tx)?;
        }
        Ok(())
    }

    pub(crate) fn execute_transaction_readonly<const MODE: InspectMode>(
        &self,
        tx: VersionedTransaction,
        log_collector: Rc<RefCell<LogCollector>>,
    ) -> ExecutionResult {
        // Single flag load for this transaction; sanitize and execution share it.
        let flags = TxFeatureFlags::load(&self.version.feature_set);
        map_sanitize_result(self.sanitize_transaction_with_flags(tx, &flags), |s_tx| {
            self.execute_sanitized_transaction::<MODE>(&s_tx, log_collector, &flags)
        })
    }

    pub(crate) fn execute_transaction_no_verify_readonly<const MODE: InspectMode>(
        &self,
        tx: VersionedTransaction,
        log_collector: Rc<RefCell<LogCollector>>,
    ) -> ExecutionResult {
        // Single flag load for this transaction; sanitize and execution share it.
        let flags = TxFeatureFlags::load(&self.version.feature_set);
        map_sanitize_result(self.sanitize_transaction_no_verify(tx), |s_tx| {
            self.execute_sanitized_transaction::<MODE>(&s_tx, log_collector, &flags)
        })
    }
}

#[cfg(test)]
mod tests {
    use solana_account::AccountSharedData;
    use solana_address::Address;
    use solana_instruction::Instruction;
    use solana_keypair::Keypair;
    use solana_message::{Message, VersionedMessage};
    use solana_signer::Signer;
    use solana_transaction::versioned::VersionedTransaction;
    use solana_transaction_error::TransactionError;

    use super::*;
    use crate::{
        Hpsvm,
        account_source::{AccountSource, AccountSourceError},
    };

    /// An account source that is permanently unavailable.
    ///
    /// `AccountSourceError::new` maps to `AccountSourceErrorKind::Unavailable`,
    /// which the taxonomy classifies as fatal (retrying is pointless).
    struct UnavailableSource;

    impl AccountSource for UnavailableSource {
        fn get_account(
            &self,
            _pubkey: &Address,
        ) -> Result<Option<AccountSharedData>, AccountSourceError> {
            Err(AccountSourceError::new("account source is down"))
        }
    }

    /// Builds a VM whose committed program cache holds `program_id` while the
    /// overlay does not hold its account.
    ///
    /// The clone is taken *before* the program is added: a clone shares the
    /// committed program cache but deep-copies the overlay, so the program
    /// lands in the shared cache while the clone's overlay never sees the
    /// account (and never tombstones it either). That is the state the old
    /// "read-only program account in the program cache" fast path mishandled.
    fn vm_with_program_missing_from_overlay() -> (Hpsvm, Address) {
        let program_id = Address::new_unique();
        let mut svm = Hpsvm::new();
        let worker = svm.clone();
        let elf = include_bytes!("../elf/spl_memo-3.0.0.so");
        svm.add_program(program_id, &elf[..]).expect("program should load");
        (worker, program_id)
    }

    /// Builds a transaction that invokes `program_id` with no accounts.
    fn program_transaction(
        program_id: &Address,
        payer: &Keypair,
        blockhash: solana_hash::Hash,
    ) -> VersionedTransaction {
        let instruction = Instruction { program_id: *program_id, accounts: vec![], data: vec![] };
        let message =
            Message::new_with_blockhash(&[instruction], Some(&payer.pubkey()), &blockhash);
        VersionedTransaction::try_new(VersionedMessage::Legacy(message), &[payer])
            .expect("transaction should build")
    }

    /// A program account that the overlay does not know must fail the
    /// transaction cleanly instead of panicking.
    #[test]
    fn a_program_account_missing_from_the_overlay_fails_cleanly() {
        let (mut worker, program_id) = vm_with_program_missing_from_overlay();
        let payer = Keypair::new();
        worker.airdrop(&payer.pubkey(), 1_000_000).expect("airdrop should succeed");
        let blockhash = worker.latest_blockhash();
        let tx = program_transaction(&program_id, &payer, blockhash);

        let staged = worker.transact(tx).expect("staging must not fail fatally");
        let error = match staged.result() {
            Ok(_) => panic!("a missing program account must not succeed"),
            Err(error) => error,
        };
        assert!(
            matches!(error.err, TransactionError::InvalidProgramForExecution),
            "unexpected error: {:?}",
            error.err
        );
    }

    /// An unreachable account source must surface as a classified, fatal
    /// error instead of panicking.
    #[test]
    fn an_unreachable_account_source_reports_the_source_failure() {
        let (mut worker, program_id) = vm_with_program_missing_from_overlay();
        let payer = Keypair::new();
        worker.airdrop(&payer.pubkey(), 1_000_000).expect("airdrop should succeed");
        worker.set_account_source(UnavailableSource);
        let blockhash = worker.latest_blockhash();
        let tx = program_transaction(&program_id, &payer, blockhash);

        let error = match worker.transact(tx) {
            Ok(_) => panic!("an unreachable account source must not stage successfully"),
            Err(error) => error,
        };
        assert!(matches!(error, HPSVMError::AccountSource { .. }), "unexpected error: {error:?}");
    }
}
