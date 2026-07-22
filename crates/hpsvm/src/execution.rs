use std::{cell::RefCell, rc::Rc};

use agave_feature_set::{FeatureSet, raise_cpi_nesting_limit_to_8};
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
use solana_transaction_context::{IndexOfAccount, transaction::TransactionContext};
use solana_transaction_error::TransactionError;

use crate::{
    HPSVM,
    account_source::AccountSourceError,
    accounts_db::AccountSourceTrackingAddressLoader,
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
    solana_compute_budget_instruction::instructions_processor::process_compute_budget_instructions(
        sanitized_tx.program_instructions_iter(),
        feature_set,
    )
    .map_err(|e| ExecutionResult { tx_result: Err(e), ..Default::default() })
}

fn get_transaction_account_lock_limit(svm: &HPSVM) -> usize {
    use solana_transaction::sanitized::MAX_TX_ACCOUNT_LOCKS;
    if svm.cfg.feature_set.is_active(&agave_feature_set::increase_tx_account_lock_limit::id()) {
        MAX_TX_ACCOUNT_LOCKS
    } else {
        64
    }
}

impl HPSVM {
    fn create_transaction_context(
        &self,
        compute_budget: solana_compute_budget::compute_budget::ComputeBudget,
        accounts: Vec<(Address, AccountSharedData)>,
        number_of_top_level_instructions: usize,
        rent: solana_rent::Rent,
    ) -> TransactionContext<'_> {
        TransactionContext::new(
            accounts,
            rent,
            compute_budget.max_instruction_stack_depth,
            compute_budget.max_instruction_trace_length,
            number_of_top_level_instructions,
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
        let tx = self.sanitize_transaction_no_verify(tx)?;

        tx.verify().map_err(|err| ExecutionResult { tx_result: Err(err), ..Default::default() })?;
        SanitizedTransaction::validate_account_locks(
            tx.message(),
            get_transaction_account_lock_limit(self),
        )
        .map_err(|err| ExecutionResult { tx_result: Err(err), ..Default::default() })?;

        Ok(tx)
    }

    #[cfg_attr(feature = "hotpath", hotpath::measure)]
    fn process_transaction<'a, 'b>(
        &'a self,
        tx: &'b SanitizedTransaction,
        compute_budget_limits: ComputeBudgetLimits,
        log_collector: Rc<RefCell<LogCollector>>,
    ) -> Result<CheckAndProcessTransactionSuccess<'b>, ExecutionResult>
    where
        'a: 'b,
    {
        let mut account_source_failures = Vec::new();
        let compute_budget = hotpath_block!("hpsvm::process_transaction::compute_budget", {
            self.runtime_env.compute_budget.unwrap_or_else(|| {
                solana_compute_budget::compute_budget::ComputeBudget {
                    compute_unit_limit: u64::from(compute_budget_limits.compute_unit_limit),
                    heap_size: compute_budget_limits.updated_heap_bytes,
                    ..solana_compute_budget::compute_budget::ComputeBudget::new_with_defaults(
                        self.cfg.feature_set.is_active(&raise_cpi_nesting_limit_to_8::ID),
                    )
                }
            })
        });
        let rent = hotpath_block!("hpsvm::process_transaction::load_rent", {
            self.accounts.sysvar_cache().get_rent().expect("rent sysvar should always be available")
        });
        let message = tx.message();
        let blockhash = message.recent_blockhash();
        // reload program cache
        let mut program_cache_for_tx_batch = hotpath_block!(
            "hpsvm::process_transaction::clone_program_cache",
            self.accounts.cloned_programs_cache()
        );
        let mut accumulated_consume_units = 0;
        let account_keys = message.account_keys();
        let prioritization_fee = compute_budget_limits.get_prioritization_fee();
        let fee = hotpath_block!("hpsvm::process_transaction::calculate_fee", {
            solana_fee::calculate_fee(
                message,
                self.cfg.fee_structure.lamports_per_signature,
                prioritization_fee,
                FeeFeatures::from(&self.cfg.feature_set),
            )
        });
        let mut validated_fee_payer = false;
        let mut payer_key = None;
        let mut accounts = hotpath_block!("hpsvm::process_transaction::load_accounts", {
            let mut accounts = Vec::with_capacity(account_keys.len());

            for (i, key) in account_keys.iter().enumerate() {
                let account = if solana_sdk_ids::sysvar::instructions::check_id(key) {
                    construct_instructions_account(message)
                } else {
                    let is_instruction_account = message.is_instruction_account(i);
                    let mut account = if !is_instruction_account &&
                        !message.is_writable(i) &&
                        self.accounts.has_program_cache_entry(key)
                    {
                        self.accounts
                            .get_account(key)
                            .expect("account should exist during processing")
                    } else {
                        match self.accounts.try_get_account(key) {
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
        let builtins_start_index = accounts.len();
        let program_indices = hotpath_block!(
            "hpsvm::process_transaction::resolve_program_indices",
            self.resolve_program_indices(
                tx,
                &mut accounts,
                builtins_start_index,
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

        let svm_feature_set = self.cfg.feature_set.runtime_features();
        let mut invoke_context =
            hotpath_block!("hpsvm::process_transaction::build_invoke_context", {
                InvokeContext::new(
                    &mut context,
                    &mut program_cache_for_tx_batch,
                    EnvironmentConfig::new(
                        *blockhash,
                        self.cfg.fee_structure.lamports_per_signature,
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

        self.on_transaction_start(tx);

        let tx_result = hotpath_block!("hpsvm::process_transaction::process_message", {
            process_message(
                self,
                message,
                &program_indices,
                &mut invoke_context,
                &mut ExecuteTimings::default(),
                &mut accumulated_consume_units,
            )
        });

        self.on_transaction_end(&tx_result);

        #[cfg(feature = "invocation-inspect-callback")]
        self.invocation_inspect_callback.after_invocation(
            self,
            &invoke_context,
            self.enable_register_tracing,
        );

        Ok(CheckAndProcessTransactionSuccess {
            core: CheckAndProcessTransactionSuccessCore {
                result: tx_result,
                compute_units_consumed: accumulated_consume_units,
                context: Some(context),
            },
            fee,
            payer_key,
            account_source_failures,
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
        accounts: &mut Vec<(Address, AccountSharedData)>,
        builtins_start_index: usize,
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

            let Some(cached_program_accounts) = accounts.get(builtins_start_index..) else {
                return Err(ExecutionResult {
                    tx_result: Err(TransactionError::ProgramAccountNotFound),
                    compute_units_consumed: accumulated_consume_units,
                    fee,
                    ..Default::default()
                });
            };

            if !cached_program_accounts.iter().any(|(key, _)| key == owner_id) {
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
                        account_source_failures.push(AccountSourceFailure {
                            pubkey: *owner_id,
                            error: error.to_string(),
                        });
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
                accounts.push((*owner_id, owner_account));
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

    pub(crate) fn execute_transaction_no_verify(
        &mut self,
        tx: VersionedTransaction,
        log_collector: Rc<RefCell<LogCollector>>,
    ) -> ExecutionResult {
        map_sanitize_result(self.sanitize_transaction_no_verify(tx), |s_tx| {
            self.execute_sanitized_transaction(&s_tx, log_collector)
        })
    }

    pub(crate) fn execute_transaction(
        &mut self,
        tx: VersionedTransaction,
        log_collector: Rc<RefCell<LogCollector>>,
    ) -> ExecutionResult {
        map_sanitize_result(self.sanitize_transaction(tx), |s_tx| {
            self.execute_sanitized_transaction(&s_tx, log_collector)
        })
    }

    pub(crate) fn execute_sanitized_transaction(
        &self,
        sanitized_tx: &SanitizedTransaction,
        log_collector: Rc<RefCell<LogCollector>>,
    ) -> ExecutionResult {
        self.execute_sanitized_transaction_impl(sanitized_tx, log_collector)
    }

    fn execute_sanitized_transaction_impl(
        &self,
        sanitized_tx: &SanitizedTransaction,
        log_collector: Rc<RefCell<LogCollector>>,
    ) -> ExecutionResult {
        let CheckAndProcessTransactionSuccess {
            core: CheckAndProcessTransactionSuccessCore { result, compute_units_consumed, context },
            fee,
            payer_key,
            account_source_failures,
        } = match self.check_and_process_transaction(sanitized_tx, log_collector) {
            Ok(value) => value,
            Err(value) => return value,
        };
        if let Some(ctx) = context {
            execution_result_if_context(
                sanitized_tx,
                ctx,
                result,
                compute_units_consumed,
                fee,
                payer_key,
                account_source_failures,
            )
        } else {
            ExecutionResult {
                tx_result: result,
                compute_units_consumed,
                fee,
                account_source_failures,
                ..Default::default()
            }
        }
    }

    fn check_and_process_transaction<'a, 'b>(
        &'a self,
        sanitized_tx: &'b SanitizedTransaction,
        log_collector: Rc<RefCell<LogCollector>>,
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
        let compute_budget_limits = get_compute_budget_limits(sanitized_tx, &self.cfg.feature_set)?;
        self.maybe_history_check(sanitized_tx)?;
        self.process_transaction(sanitized_tx, compute_budget_limits, log_collector)
    }

    fn maybe_history_check(
        &self,
        sanitized_tx: &SanitizedTransaction,
    ) -> Result<(), ExecutionResult> {
        if self.cfg.sigverify && self.history.check_transaction(sanitized_tx.signature()) {
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
        if self.cfg.blockhash_check {
            self.check_transaction_age(sanitized_tx)?;
        }
        Ok(())
    }

    pub(crate) fn execute_transaction_readonly(
        &self,
        tx: VersionedTransaction,
        log_collector: Rc<RefCell<LogCollector>>,
    ) -> ExecutionResult {
        map_sanitize_result(self.sanitize_transaction(tx), |s_tx| {
            self.execute_sanitized_transaction(&s_tx, log_collector)
        })
    }

    pub(crate) fn execute_transaction_no_verify_readonly(
        &self,
        tx: VersionedTransaction,
        log_collector: Rc<RefCell<LogCollector>>,
    ) -> ExecutionResult {
        map_sanitize_result(self.sanitize_transaction_no_verify(tx), |s_tx| {
            self.execute_sanitized_transaction(&s_tx, log_collector)
        })
    }
}
