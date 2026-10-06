//! Core [`HPSVM`] behaviour: transaction execution entry points, account and
//! sysvar state management, program loading, and the public transaction API.

use std::{cell::RefCell, path::Path, rc::Rc, sync::Arc};

use agave_feature_set::{FeatureSet, raise_cpi_nesting_limit_to_8};
#[cfg(feature = "precompiles")]
use agave_precompiles::get_precompiles;
use agave_reserved_account_keys::ReservedAccountKeys;
use serde::{Serialize, de::DeserializeOwned};
use solana_account::{
    Account, AccountSharedData, ReadableAccount, WritableAccount, state_traits::StateMut,
};
use solana_address::Address;
use solana_builtins::BUILTINS;
use solana_clock::Clock;
use solana_compute_budget::compute_budget::ComputeBudget;
use solana_epoch_rewards::EpochRewards;
use solana_epoch_schedule::EpochSchedule;
use solana_feature_gate_interface::{self as feature_gate, Feature};
use solana_fee_structure::FeeStructure;
use solana_hash::Hash;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_last_restart_slot::LastRestartSlot;
use solana_loader_v3_interface::{get_program_data_address, state::UpgradeableLoaderState};
use solana_message::{Message, SanitizedMessage, VersionedMessage};
use solana_nonce::{NONCED_TX_MARKER_IX_INDEX, state::DurableNonce};
use solana_nonce_account::verify_nonce_account;
use solana_program_runtime::{
    invoke_context::BuiltinFunctionRegisterer, program_cache_entry::ProgramCacheEntry,
};
use solana_rent::Rent;
use solana_sdk_ids::{bpf_loader, bpf_loader_deprecated, bpf_loader_upgradeable, system_program};
use solana_signature::Signature;
use solana_signer::Signer;
use solana_slot_hashes::{SlotHash, SlotHashes};
use solana_slot_history::SlotHistory;
use solana_stake_interface::stake_history::StakeHistory;
use solana_svm_log_collector::LogCollector;
use solana_svm_transaction::svm_message::{SVMMessage, SVMStaticMessage};
use solana_sysvar::Sysvar;
#[expect(deprecated)]
use solana_sysvar::{
    fees::Fees,
    recent_blockhashes::{IterItem, RecentBlockhashes},
};
use solana_sysvar_id::SysvarId;
use solana_transaction::{sanitized::SanitizedTransaction, versioned::VersionedTransaction};
use solana_transaction_error::TransactionError;

#[cfg(feature = "precompiles")]
use crate::precompiles::load_precompiles;
#[cfg(feature = "register-tracing")]
use crate::register_tracing::DefaultRegisterTracingCallback;
use crate::{
    AccountSource, AccountSourceError, AccountsView, BlockEnv, CustomSyscallRegistration, HPSVM,
    HpsvmBuilder, Inspector, RuntimeEnv, SvmCfg, TransactionOrigin,
    account_source::EmptyAccountSource,
    batch::{
        TransactionBatchError, TransactionBatchExecutionResult, TransactionBatchPlan,
        plan_transaction_batch, send_transaction_batch,
    },
    commit::commit_execution_outcome,
    error::HPSVMError,
    helpers::*,
    history::TransactionHistory,
    inspector::NoopInspector,
    instruction::InstructionCase,
    next_vm_instance_id,
    programs::{DEFAULT_PROGRAM_IDS, SPL_PROGRAM_IDS, load_default_programs, load_spl_programs},
    runtime_registry::RuntimeExtensionRegistry,
    types::{
        AccountDiff, AccountSourceFailure, ExecutionDiagnostics, ExecutionOutcome, ExecutionResult,
        ExecutionTrace, FailedTransactionMetadata, SimulatedTransactionInfo, TransactionMetadata,
        TransactionResult,
    },
    utils::create_blockhash,
};
#[cfg(feature = "invocation-inspect-callback")]
use crate::{EmptyInvocationInspectCallback, InvocationInspectCallback};

/// Size of the account buffer that holds a serialized sysvar.
///
/// Programs read sysvars through the `sol_get_sysvar` syscall using the sysvar's
/// *canonical* account size, not the length of the value currently being written.
/// Three sysvars have a canonical size that differs from the bincode length of their
/// default value, so they need their published `SIZE` constant; every other sysvar is
/// a fixed-layout struct whose default value already serializes to the right length.
///
/// This replaces `solana_sysvar::SysvarSerialize::size_of`, which was removed along
/// with the rest of that trait in solana-sysvar 5.0.0.
fn sysvar_account_size<T>(sysvar_id: &Address) -> usize
where
    T: Default + Serialize + SysvarId,
{
    if solana_sysvar::recent_blockhashes::check_id(sysvar_id) {
        solana_sysvar::recent_blockhashes::SIZE
    } else if solana_sysvar::slot_hashes::check_id(sysvar_id) {
        solana_sysvar::slot_hashes::SIZE
    } else if solana_sysvar::slot_history::check_id(sysvar_id) {
        solana_sysvar::slot_history::SIZE
    } else {
        // Measure with the runtime's own bincode codec so the result always matches
        // what `AccountSharedData::serialize_data` is about to write.
        AccountSharedData::new_data(0, &T::default(), &solana_sdk_ids::sysvar::id())
            .map_or(0, |account| account.data_clone().len())
    }
}

impl HPSVM {
    pub(crate) fn default_register_tracing_enabled() -> bool {
        // Allow users to virtually get register tracing data without doing any
        // changes to their code provided `SBF_TRACE_DIR` is set.
        #[cfg(feature = "register-tracing")]
        {
            return std::env::var("SBF_TRACE_DIR").is_ok();
        }
        #[cfg(not(feature = "register-tracing"))]
        {
            false
        }
    }

    pub(crate) const fn invalidate_execution_outcomes(&mut self) {
        self.state_version = self.state_version.wrapping_add(1);
    }

    pub(crate) fn sync_block_env_slot(&mut self) {
        self.block_env.slot = self.accounts.current_slot();
    }

    pub(crate) fn new_inner(_enable_register_tracing: bool) -> Self {
        let feature_set = FeatureSet::default();
        let latest_blockhash = create_blockhash(b"genesis");

        Self {
            accounts: Default::default(),
            airdrop_kp: Keypair::new().to_bytes(),
            builtins_loaded: false,
            default_programs_loaded: false,
            spl_programs_loaded: false,
            reserved_account_keys: Self::reserved_account_keys_for_feature_set(&feature_set),
            cfg: SvmCfg {
                feature_set,
                sigverify: false,
                blockhash_check: false,
                fee_structure: FeeStructure::default(),
                compute_diagnostics: true,
            },
            feature_accounts_loaded: false,
            inspector: Arc::new(NoopInspector),
            inspection_origin: TransactionOrigin::User,
            runtime_registry: RuntimeExtensionRegistry::default(),
            instance_id: next_vm_instance_id(),
            state_version: 0,
            block_env: BlockEnv { latest_blockhash, slot: 0 },
            history: TransactionHistory::new(),
            runtime_env: RuntimeEnv { compute_budget: None, log_bytes_limit: Some(10_000) },
            sysvars_loaded: false,
            #[cfg(feature = "invocation-inspect-callback")]
            enable_register_tracing: _enable_register_tracing,
            #[cfg(feature = "invocation-inspect-callback")]
            invocation_inspect_callback: {
                #[cfg(feature = "register-tracing")]
                if _enable_register_tracing {
                    Arc::new(DefaultRegisterTracingCallback::default())
                } else {
                    Arc::new(EmptyInvocationInspectCallback {})
                }
                #[cfg(not(feature = "register-tracing"))]
                Arc::new(EmptyInvocationInspectCallback {})
            },
        }
    }

    /// Creates the basic test environment.
    #[cfg_attr(feature = "hotpath", hotpath::measure)]
    pub fn new() -> Self {
        Self::builder()
            .with_program_test_defaults()
            .build()
            .expect("standard HPSVM construction should remain infallible")
    }

    /// Create a typed builder for explicit, compile-time-checked environment assembly.
    pub fn builder() -> HpsvmBuilder {
        HpsvmBuilder::new()
    }

    /// Installs an execution inspector that observes top-level transaction activity.
    pub fn with_inspector<I: Inspector + 'static>(mut self, inspector: I) -> Self {
        self.inspector = Arc::new(inspector);
        self.invalidate_execution_outcomes();
        self
    }

    pub(crate) fn on_transaction_start(&self, tx: &SanitizedTransaction) {
        self.inspector.on_transaction_start_with_origin(self.inspection_origin, self, tx);
    }

    pub(crate) fn on_instruction(&self, index: usize, program_id: &Address) {
        self.inspector.on_instruction_with_origin(self.inspection_origin, self, index, program_id);
    }

    pub(crate) fn on_transaction_end(
        &self,
        result: &solana_transaction_error::TransactionResult<()>,
    ) {
        self.inspector.on_transaction_end_with_origin(self.inspection_origin, self, result);
    }

    pub(crate) fn with_transaction_origin<T>(
        &mut self,
        origin: TransactionOrigin,
        op: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let previous_origin = self.inspection_origin;
        self.inspection_origin = origin;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| op(self)));
        self.inspection_origin = previous_origin;
        match result {
            Ok(result) => result,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    pub(crate) fn with_temporary_account_source<T>(
        &mut self,
        source: Arc<dyn AccountSource>,
        op: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let previous_source = self.accounts.replace_account_source(source);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| op(self)));
        self.accounts.set_account_source(previous_source);
        match result {
            Ok(result) => result,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    #[cfg(feature = "register-tracing")]
    /// Create a test environment with debugging features.
    ///
    /// This constructor allows enabling low-level VM debugging capabilities,
    /// such as register tracing, which are baked into program executables at
    /// load time and cannot be changed afterwards.
    ///
    /// When `enable_register_tracing` is `true`:
    /// - Programs are loaded with register tracing support
    /// - A default [`DefaultRegisterTracingCallback`] is installed
    /// - Trace data is written to `SBF_TRACE_DIR` (or `target/sbf/trace` by default)
    pub fn new_debuggable(enable_register_tracing: bool) -> Self {
        Self::builder()
            .with_register_tracing(enable_register_tracing)
            .with_program_test_defaults()
            .build()
            .expect("standard debuggable HPSVM construction should remain infallible")
    }

    pub(crate) fn clear_feature_accounts(&mut self, previous_feature_set: &FeatureSet) {
        previous_feature_set.active().iter().for_each(|(feature_id, _)| {
            self.accounts.remove_account(feature_id);
        });
    }

    pub(crate) fn clear_builtin_accounts(&mut self) {
        for builtin in BUILTINS {
            self.accounts.remove_account(&builtin.program_id);
        }
    }

    pub(crate) fn clear_default_programs(&mut self) {
        for program_id in &DEFAULT_PROGRAM_IDS {
            if self
                .accounts
                .get_account_ref(program_id)
                .is_some_and(|account| account.owner() == &bpf_loader_upgradeable::id())
            {
                self.accounts.remove_account(&get_program_data_address(program_id));
            }
            self.accounts.remove_account(program_id);
        }
    }

    pub(crate) fn clear_spl_programs(&mut self) {
        for program_id in &SPL_PROGRAM_IDS {
            if self
                .accounts
                .get_account_ref(program_id)
                .is_some_and(|account| account.owner() == &bpf_loader_upgradeable::id())
            {
                self.accounts.remove_account(&get_program_data_address(program_id));
            }
            self.accounts.remove_account(program_id);
        }
    }

    #[cfg(feature = "precompiles")]
    pub(crate) fn clear_precompile_accounts(&mut self) {
        get_precompiles().iter().for_each(|precompile| {
            self.accounts.remove_account(&precompile.program_id);
        });
    }

    pub(crate) fn reconfigure_materialized_feature_state(
        &mut self,
        previous_feature_set: &FeatureSet,
    ) -> Result<(), HPSVMError> {
        if self.feature_accounts_loaded {
            self.clear_feature_accounts(previous_feature_set);
            self.materialize_feature_accounts();
        }

        if self.default_programs_loaded {
            self.clear_default_programs();
        }

        if self.spl_programs_loaded {
            self.clear_spl_programs();
        }

        if self.builtins_loaded {
            self.clear_builtin_accounts();
            self.load_builtins();
        } else if !self.runtime_registry.custom_syscalls().is_empty() {
            self.refresh_runtime_environments();
        }

        #[cfg(feature = "precompiles")]
        if self.runtime_registry.loads_standard_precompiles() {
            self.clear_precompile_accounts();
            self.load_precompiles();
        }

        if self.default_programs_loaded {
            self.load_default_programs();
        }

        if self.spl_programs_loaded {
            self.load_spl_programs();
        }

        self.accounts.rebuild_program_cache().map_err(HPSVMError::from)
    }

    /// **Advanced reconfiguration.** Replaces the runtime compute budget and invalidates any
    /// previously transacted but uncommitted outcomes.
    pub const fn set_compute_budget(&mut self, compute_budget: ComputeBudget) {
        self.runtime_env.compute_budget = Some(compute_budget);
        self.invalidate_execution_outcomes();
    }

    /// **Advanced reconfiguration.** Replaces only the runtime compute unit limit.
    pub fn set_compute_unit_limit(&mut self, compute_unit_limit: u64) {
        let mut compute_budget = self.runtime_env.compute_budget.unwrap_or_else(|| {
            ComputeBudget::new_with_defaults(
                self.cfg.feature_set.is_active(&raise_cpi_nesting_limit_to_8::ID),
            )
        });
        compute_budget.compute_unit_limit = compute_unit_limit;
        self.set_compute_budget(compute_budget);
    }

    /// **Advanced reconfiguration.** Enables or disables signature verification for future
    /// transactions and invalidates any previously transacted but uncommitted outcomes.
    pub const fn set_sigverify(&mut self, sigverify: bool) {
        self.cfg.sigverify = sigverify;
        self.invalidate_execution_outcomes();
    }

    /// **Advanced reconfiguration.** Enables or disables blockhash checking for future
    /// transactions and invalidates any previously transacted but uncommitted outcomes.
    pub const fn set_blockhash_check(&mut self, check: bool) {
        self.cfg.blockhash_check = check;
        self.invalidate_execution_outcomes();
    }

    /// **Advanced reconfiguration.** Enables or disables execution diagnostics for future
    /// transactions and invalidates any previously transacted but uncommitted outcomes.
    ///
    /// When diagnostics are disabled, `send_transaction` / `transact` returns an empty
    /// [`ExecutionDiagnostics`](types::ExecutionDiagnostics) instead of computing pre/post
    /// account diffs and token balances.
    pub const fn set_compute_diagnostics(&mut self, enabled: bool) {
        self.cfg.compute_diagnostics = enabled;
        self.invalidate_execution_outcomes();
    }

    pub(crate) fn set_sysvars(&mut self) {
        self.sysvars_loaded = true;
        self.set_sysvar_internal(&Clock::default());
        self.set_sysvar_internal(&EpochRewards::default());
        self.set_sysvar_internal(&EpochSchedule::default());
        #[expect(deprecated)]
        let fees = Fees::default();
        self.set_sysvar_internal(&fees);
        self.set_sysvar_internal(&LastRestartSlot::default());
        let latest_blockhash = self.block_env.latest_blockhash;
        #[expect(deprecated)]
        self.set_sysvar_internal(&RecentBlockhashes::from_iter([IterItem(
            0,
            &latest_blockhash,
            fees.fee_calculator.lamports_per_signature,
        )]));

        // Rent account differs based off feature gating
        #[expect(deprecated)]
        {
            let mut rent_account = Rent::default();
            if self
                .cfg
                .feature_set
                .is_active(&agave_feature_set::deprecate_rent_exemption_threshold::id())
            {
                rent_account.exemption_threshold = 1.0f64.to_le_bytes();
                rent_account.lamports_per_byte = solana_rent::DEFAULT_LAMPORTS_PER_BYTE;
            }
            self.set_sysvar_internal(&rent_account);
        }
        self.set_sysvar_internal(&SlotHashes::new(&[SlotHash::new(
            self.accounts.current_slot(),
            latest_blockhash,
        )]));
        self.set_sysvar_internal(&SlotHistory::default());
        // ponytail: StakeHistory doesn't impl the wincode StateMutWincode helper,
        // so set the account directly via serde using the standard Solana API.
        {
            let account = AccountSharedData::new_data(
                1,
                &StakeHistory::default(),
                &solana_sdk_ids::sysvar::id(),
            )
            .expect("StakeHistory account creation");
            self.accounts
                .add_account(StakeHistory::id(), account)
                .expect("add StakeHistory account");
        }
        self.invalidate_execution_outcomes();
    }

    /// **Advanced reconfiguration.** Replaces the active feature set, rebuilds any materialized
    /// feature-dependent state, and invalidates previously transacted but uncommitted outcomes.
    pub fn set_feature_set(&mut self, feature_set: FeatureSet) -> Result<(), HPSVMError> {
        let previous_feature_set = self.cfg.feature_set.clone();
        let previous_accounts = self.accounts.clone();
        let previous_reserved_account_keys = self.reserved_account_keys.clone();
        let previous_state_version = self.state_version;

        self.cfg.feature_set = feature_set;
        self.reserved_account_keys =
            Self::reserved_account_keys_for_feature_set(&self.cfg.feature_set);
        if let Err(error) = self.reconfigure_materialized_feature_state(&previous_feature_set) {
            self.cfg.feature_set = previous_feature_set;
            self.reserved_account_keys = previous_reserved_account_keys;
            self.accounts = previous_accounts;
            self.state_version = previous_state_version;
            return Err(error);
        }

        self.invalidate_execution_outcomes();
        Ok(())
    }

    pub(crate) fn materialize_feature_accounts(&mut self) {
        self.feature_accounts_loaded = true;
        for (feature_id, activation_slot) in self.cfg.feature_set.active() {
            let feature_account = Feature { activated_at: Some(*activation_slot) };
            let lamports = self.minimum_balance_for_rent_exemption(Feature::size_of());
            let account = feature_gate::create_account(&feature_account, lamports);
            self.accounts.add_account_no_checks(*feature_id, account);
        }
    }

    pub(crate) fn set_feature_accounts(&mut self) {
        self.materialize_feature_accounts();
        self.invalidate_execution_outcomes();
    }

    pub(crate) fn reserved_account_keys_for_feature_set(
        feature_set: &FeatureSet,
    ) -> ReservedAccountKeys {
        let mut reserved_account_keys = ReservedAccountKeys::default();
        reserved_account_keys.update_active_set(feature_set);
        reserved_account_keys
    }

    pub(crate) fn load_builtins(&mut self) {
        self.builtins_loaded = true;
        self.refresh_runtime_environments();
        for builtint in BUILTINS {
            if builtint.enable_feature_id.is_none_or(|x| self.cfg.feature_set.is_active(&x)) {
                let loaded_program = ProgramCacheEntry::new_builtin(builtint.register_fn);
                self.accounts
                    .replenish_program_cache(builtint.program_id, Arc::new(loaded_program));
                self.accounts.add_builtin_account(
                    builtint.program_id,
                    crate::utils::create_loadable_account_for_test(builtint.name),
                );
            }
        }
    }

    pub(crate) fn set_builtins(&mut self) {
        self.load_builtins();
        self.invalidate_execution_outcomes();
    }

    pub fn set_lamports(&mut self, lamports: u64) {
        self.accounts.add_account_no_checks(
            Keypair::try_from(self.airdrop_kp.as_slice())
                .expect("airdrop keypair should be valid")
                .pubkey(),
            AccountSharedData::new(lamports, 0, &system_program::id()),
        );
        self.invalidate_execution_outcomes();
    }

    pub(crate) fn load_default_programs(&mut self) {
        self.default_programs_loaded = true;
        load_default_programs(self);
    }

    pub(crate) fn load_spl_programs(&mut self) {
        self.spl_programs_loaded = true;
        load_spl_programs(self);
    }

    pub(crate) fn set_default_programs(&mut self) {
        self.load_default_programs();
        self.invalidate_execution_outcomes();
    }

    pub(crate) fn set_spl_programs(&mut self) {
        self.load_spl_programs();
        self.invalidate_execution_outcomes();
    }

    /// **Advanced reconfiguration.** Changes the transaction history capacity. Set this to 0 to
    /// disable history and allow duplicate transactions.
    pub fn set_transaction_history(&mut self, capacity: usize) {
        self.history.set_capacity(capacity);
        self.invalidate_execution_outcomes();
    }

    /// **Advanced reconfiguration.** Installs a read-through account source for future lookups and
    /// invalidates previously transacted but uncommitted outcomes.
    pub fn set_account_source(&mut self, source: impl AccountSource + 'static) {
        self.accounts.set_account_source(Arc::new(source));
        self.invalidate_execution_outcomes();
    }

    /// **Advanced reconfiguration.** Adjusts the log truncation limit for future execution and
    /// invalidates previously transacted but uncommitted outcomes.
    pub const fn set_log_bytes_limit(&mut self, limit: Option<usize>) {
        self.runtime_env.log_bytes_limit = limit;
        self.invalidate_execution_outcomes();
    }

    #[cfg(feature = "precompiles")]
    pub(crate) fn load_precompiles(&mut self) {
        load_precompiles(self);
    }

    #[cfg(feature = "precompiles")]
    pub(crate) fn set_precompiles(&mut self) {
        self.runtime_registry.enable_standard_precompiles();
        self.load_precompiles();
        self.invalidate_execution_outcomes();
    }

    /// Returns minimum balance required to make an account with specified data length rent exempt.
    pub fn minimum_balance_for_rent_exemption(&self, data_len: usize) -> u64 {
        self.accounts.minimum_balance_for_rent_exemption(data_len)
    }

    /// Returns all information associated with the account of the provided pubkey.
    #[cfg_attr(feature = "hotpath", hotpath::measure)]
    pub fn get_account(&self, address: &Address) -> Option<Account> {
        self.accounts.get_account(address).map(Into::into)
    }

    /// Returns account data while preserving failures from a configured external source.
    pub fn try_get_account(
        &self,
        address: &Address,
    ) -> Result<Option<Account>, AccountSourceError> {
        self.accounts.try_get_account(address).map(|account| account.map(Into::into))
    }

    /// **⚠️ ADVANCED USE ONLY ⚠️**
    ///
    /// Sets all information associated with the account of the provided pubkey.
    ///
    /// This writes directly into the in-memory test state. It does not execute
    /// the owning program or replay the full transaction pipeline, so it is best
    /// used for fixtures, snapshots, and explicit state surgery between
    /// transactions. Prefer [`airdrop`](HPSVM::airdrop) or
    /// [`send_transaction`](HPSVM::send_transaction) when you want a
    /// protocol-consistent state transition.
    #[cfg_attr(feature = "hotpath", hotpath::measure)]
    pub fn set_account(&mut self, address: Address, data: Account) -> Result<(), HPSVMError> {
        self.accounts.add_account(address, data.into())?;
        self.sync_block_env_slot();
        self.invalidate_execution_outcomes();
        Ok(())
    }

    /// **⚠️ ADVANCED USE ONLY ⚠️**
    ///
    /// Returns a read-only view of the internal accounts database.
    ///
    /// This provides read-only access to the accounts database for advanced inspection.
    /// Use [`get_account`](HPSVM::get_account) for normal account retrieval.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hpsvm::HPSVM;
    ///
    /// let svm = HPSVM::new();
    ///
    /// // Read-only access to accounts data
    /// let accounts = svm.accounts();
    /// // ... inspect internal state if needed
    /// ```
    pub const fn accounts(&self) -> AccountsView<'_> {
        AccountsView::new(&self.accounts)
    }

    /// Gets the balance of the provided account pubkey.
    pub fn get_balance(&self, address: &Address) -> Option<u64> {
        self.accounts.get_account(address).map(|account| account.lamports())
    }

    /// Gets the latest blockhash.
    pub const fn latest_blockhash(&self) -> Hash {
        self.block_env.latest_blockhash
    }

    /// Gets the current block environment.
    pub const fn block_env(&self) -> BlockEnv {
        self.block_env
    }

    /// **⚠️ ADVANCED USE ONLY ⚠️**
    ///
    /// Sets the sysvar in the test environment.
    ///
    /// This is a direct override intended for tests that need to manipulate
    /// runtime context. It bypasses transaction execution.
    ///
    /// Returns an error if serialization fails or if the sysvar account update
    /// is rejected by the internal accounts database.
    pub fn set_sysvar<T>(&mut self, sysvar: &T) -> Result<(), HPSVMError>
    where
        T: Default + Serialize + SysvarId,
    {
        self.try_set_sysvar(sysvar)?;
        self.sync_block_env_slot();
        self.invalidate_execution_outcomes();
        Ok(())
    }

    pub(crate) fn try_set_sysvar<T>(&mut self, sysvar: &T) -> Result<(), HPSVMError>
    where
        T: Default + Serialize + SysvarId,
    {
        // The buffer is sized to the sysvar's canonical on-chain length, then the value
        // is encoded into it with the same bincode codec the read path decodes.
        let mut account = AccountSharedData::new(
            1,
            sysvar_account_size::<T>(&T::id()),
            &solana_sdk_ids::sysvar::id(),
        );
        account.serialize_data(sysvar).map_err(|error| HPSVMError::SysvarSerialization {
            sysvar: std::any::type_name::<T>(),
            reason: error.to_string(),
        })?;
        self.accounts.add_account(T::id(), account)
    }

    pub(crate) fn set_sysvar_internal<T>(&mut self, sysvar: &T)
    where
        T: Default + Serialize + SysvarId,
    {
        self.try_set_sysvar(sysvar)
            .expect("internal sysvar setup should never fail for supported sysvars");
        self.sync_block_env_slot();
    }

    /// Gets a sysvar from the test environment.
    pub fn get_sysvar<T>(&self) -> T
    where
        T: Sysvar + SysvarId + DeserializeOwned,
    {
        self.try_get_sysvar().expect("sysvar account should exist and deserialize")
    }

    pub(crate) fn try_get_sysvar<T>(&self) -> Result<T, HPSVMError>
    where
        T: Sysvar + SysvarId + DeserializeOwned,
    {
        let account = self
            .accounts
            .get_account_ref(&T::id())
            .ok_or(HPSVMError::MissingRuntimeComponent { component: "sysvars" })?;

        account.deserialize_data().map_err(|error| HPSVMError::SysvarSerialization {
            sysvar: std::any::type_name::<T>(),
            reason: error.to_string(),
        })
    }

    pub(crate) fn require_sysvars_loaded(&self) -> Result<(), HPSVMError> {
        if self.sysvars_loaded {
            Ok(())
        } else {
            Err(HPSVMError::MissingRuntimeComponent { component: "sysvars" })
        }
    }

    /// Gets a transaction from the transaction history.
    pub fn get_transaction(&self, signature: &Signature) -> Option<&TransactionResult> {
        self.history.get_transaction(signature)
    }

    /// Returns the pubkey of the internal airdrop account.
    pub fn airdrop_pubkey(&self) -> Address {
        Keypair::try_from(self.airdrop_kp.as_slice())
            .expect("airdrop keypair should be valid")
            .pubkey()
    }

    /// Airdrops lamports by submitting an internal system transfer transaction.
    ///
    /// Unlike [`set_account`](HPSVM::set_account), this goes through the normal
    /// execution pipeline instead of mutating balances directly.
    #[cfg_attr(feature = "hotpath", hotpath::measure)]
    pub fn airdrop(&mut self, address: &Address, lamports: u64) -> TransactionResult {
        let payer =
            Keypair::try_from(self.airdrop_kp.as_slice()).expect("airdrop keypair should be valid");
        let tx = VersionedTransaction::try_new(
            VersionedMessage::Legacy(Message::new_with_blockhash(
                &[solana_system_interface::instruction::transfer(
                    &payer.pubkey(),
                    address,
                    lamports,
                )],
                Some(&payer.pubkey()),
                &self.block_env.latest_blockhash,
            )),
            &[payer],
        )
        .expect("failed to create airdrop transaction");

        self.with_temporary_account_source(Arc::new(EmptyAccountSource), |svm| {
            svm.with_transaction_origin(TransactionOrigin::InternalAirdrop, |svm| {
                svm.send_transaction(tx)
            })
        })
    }

    /// Adds a builtin program to the test environment.
    pub fn add_builtin(&mut self, program_id: Address, entrypoint: BuiltinFunctionRegisterer) {
        let builtin = ProgramCacheEntry::new_builtin(entrypoint);

        self.accounts.replenish_program_cache(program_id, Arc::new(builtin));

        let mut account = AccountSharedData::new(1, 1, &bpf_loader::id());
        account.set_executable(true);
        self.accounts.add_account_no_checks(program_id, account);
        self.invalidate_execution_outcomes();
    }

    /// Adds an SBF program to the test environment from the file specified.
    #[cfg_attr(feature = "hotpath", hotpath::measure)]
    pub fn add_program_from_file(
        &mut self,
        program_id: impl Into<Address>,
        path: impl AsRef<Path>,
    ) -> Result<(), HPSVMError> {
        let bytes = std::fs::read(path)?;
        self.add_program(program_id, &bytes)?;
        Ok(())
    }

    #[cfg_attr(feature = "hotpath", hotpath::measure)]
    pub(crate) fn add_program_internal<const PREVERIFIED: bool, const CACHED: bool>(
        &mut self,
        program_id: impl Into<Address>,
        program_bytes: &[u8],
        loader_id: &Address,
    ) -> Result<(), HPSVMError> {
        let program_id = program_id.into();
        let current_slot = self.accounts.current_slot();

        if bpf_loader_upgradeable::check_id(loader_id) {
            let (programdata_address, _bump) =
                Address::find_program_address(&[program_id.as_ref()], loader_id);

            let programdata_metadata_len = UpgradeableLoaderState::size_of_programdata_metadata();
            let programdata_len = programdata_metadata_len + program_bytes.len();
            let programdata_lamports = self.minimum_balance_for_rent_exemption(programdata_len);
            let mut programdata_account =
                AccountSharedData::new(programdata_lamports, programdata_len, loader_id);
            programdata_account
                .set_state(&UpgradeableLoaderState::ProgramData {
                    slot: current_slot,
                    upgrade_authority_address: None,
                })
                .expect("UpgradeableLoaderState::ProgramData serialization should never fail");
            programdata_account.data_as_mut_slice()[programdata_metadata_len..]
                .copy_from_slice(program_bytes);

            let program_len = UpgradeableLoaderState::size_of_program();
            let program_lamports = self.minimum_balance_for_rent_exemption(program_len);
            let mut program_account =
                AccountSharedData::new(program_lamports, program_len, loader_id);
            program_account.set_executable(true);
            program_account
                .set_state(&UpgradeableLoaderState::Program { programdata_address })
                .expect("UpgradeableLoaderState::Program serialization should never fail");

            self.accounts.add_account_no_checks(programdata_address, programdata_account);
            self.accounts.add_account_no_checks(program_id, program_account);
        } else if bpf_loader::check_id(loader_id) || bpf_loader_deprecated::check_id(loader_id) {
            let program_len = program_bytes.len();
            let lamports = self.minimum_balance_for_rent_exemption(program_len);
            let mut account = AccountSharedData::new(lamports, program_len, loader_id);
            account.set_executable(true);
            account.set_data_from_slice(program_bytes);

            self.accounts.add_account_no_checks(program_id, account);
        } else {
            return Err(HPSVMError::InvalidLoader { program_id, loader_id: *loader_id });
        };

        // Resolve (and optionally memoize) the verified executable. Passing the
        // environment by reference avoids the per-load `ProgramRuntimeEnvironment`
        // clone the previous implementation performed.
        let env = self.accounts.runtime_environments().get_env_for_execution();
        let loaded_program_arc =
            self.resolve_program_entry::<CACHED>(loader_id, env, current_slot, program_bytes)?;

        self.accounts.replenish_program_cache(program_id, loaded_program_arc);

        Ok(())
    }

    /// Adds an SBF program to the test environment.
    ///
    /// Uses `BPFLoaderUpgradeable` by default for the loader.
    #[cfg_attr(feature = "hotpath", hotpath::measure)]
    pub fn add_program(
        &mut self,
        program_id: impl Into<Address>,
        program_bytes: &[u8],
    ) -> Result<(), HPSVMError> {
        self.add_program_internal::<false, false>(
            program_id,
            program_bytes,
            &bpf_loader_upgradeable::id(),
        )?;
        self.invalidate_execution_outcomes();
        Ok(())
    }

    /// Adds an SBF program with a specific loader to match mainnet CU behavior.
    ///
    /// Use `bpf_loader::id()` for BPFLoader2, `bpf_loader_deprecated::id()` for BPFLoader1,
    /// or `bpf_loader_upgradeable::id()` for the upgradeable loader.
    pub fn add_program_with_loader(
        &mut self,
        program_id: impl Into<Address>,
        program_bytes: &[u8],
        loader_id: Address,
    ) -> Result<(), HPSVMError> {
        self.add_program_internal::<false, false>(program_id, program_bytes, &loader_id)?;
        self.invalidate_execution_outcomes();
        Ok(())
    }

    /// Adds an SBF program that is known-good and already verified.
    ///
    /// This is the only caller that opts into the default-program executable
    /// cache (`CACHED = true`). It is invoked exclusively with `include_bytes!`
    /// statics, so the ELF slice pointer used as part of the cache key is
    /// stable for the lifetime of the process.
    pub(crate) fn add_program_preverified(
        &mut self,
        program_id: impl Into<Address>,
        program_bytes: &[u8],
        loader_id: &Address,
    ) -> Result<(), HPSVMError> {
        self.add_program_internal::<true, true>(program_id, program_bytes, loader_id)
    }

    /// Submits a signed transaction and commits its post-state to this VM instance.
    ///
    /// This updates accounts, transaction history, and other in-memory runtime
    /// state, so it intentionally requires `&mut self`. `hpsvm` is optimized for
    /// fast, in-process testing of a single mutable environment rather than
    /// Sealevel-style concurrent scheduling within one instance.
    #[cfg_attr(feature = "hotpath", hotpath::measure)]
    pub fn send_transaction(&mut self, tx: impl Into<VersionedTransaction>) -> TransactionResult {
        let log_collector = Rc::new(RefCell::new(LogCollector {
            bytes_limit: self.runtime_env.log_bytes_limit,
            ..Default::default()
        }));
        let execution = if self.cfg.sigverify {
            self.execute_transaction(tx.into(), log_collector.clone())
        } else {
            self.execute_transaction_no_verify(tx.into(), log_collector.clone())
        };
        let outcome = execution_into_outcome(self, execution, log_collector, "send_transaction");
        self.commit_transaction(outcome)
    }

    /// Executes a single instruction case against a cloned VM without mutating this instance.
    pub fn process_instruction_case(
        &self,
        case: &InstructionCase,
    ) -> Result<ExecutionOutcome, HPSVMError> {
        let mut working = self.clone();
        working.set_sigverify(false);

        for (address, account) in &case.pre_accounts {
            working.set_account(*address, account.clone())?;
        }

        let fee_payer = fee_payer_for_instruction_case(case);
        if working.get_account(&fee_payer).is_none() {
            working.set_account(
                fee_payer,
                Account {
                    lamports: 1_000_000_000,
                    owner: system_program::id(),
                    ..Default::default()
                },
            )?;
        }

        let message = Message::new_with_blockhash(
            &[case.instruction()],
            Some(&fee_payer),
            &working.latest_blockhash(),
        );
        let signatures =
            vec![Signature::default(); usize::from(message.header.num_required_signatures)];
        let tx = VersionedTransaction { signatures, message: VersionedMessage::Legacy(message) };

        working.try_transact(tx)
    }

    /// Processes one instruction as a synthetic transaction and commits its post-state.
    ///
    /// This is a convenience for instruction-level harnesses that do not need to
    /// construct and sign a full transaction. Signature verification is bypassed
    /// for this synthetic transaction only; blockhash and runtime behavior still
    /// use this VM's current environment.
    pub fn process_instruction(&mut self, instruction: Instruction) -> TransactionResult {
        self.process_instruction_chain([instruction])
    }

    /// Processes one instruction after first writing the provided account states.
    ///
    /// Explicit accounts are written to the VM before execution, then the
    /// instruction post-state is committed just like [`HPSVM::process_instruction`].
    pub fn process_instruction_with_accounts(
        &mut self,
        instruction: Instruction,
        pre_accounts: impl IntoIterator<Item = (Address, Account)>,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        self.process_instruction_chain_with_accounts([instruction], pre_accounts)
    }

    /// Processes multiple instructions atomically and commits their post-state.
    pub fn process_instruction_chain(
        &mut self,
        instructions: impl IntoIterator<Item = Instruction>,
    ) -> TransactionResult {
        let outcome = self.transact_instruction_chain_no_verify(instructions.into_iter().collect());
        self.commit_transaction(outcome)
    }

    /// Processes multiple instructions atomically after first writing explicit account states.
    pub fn process_instruction_chain_with_accounts(
        &mut self,
        instructions: impl IntoIterator<Item = Instruction>,
        pre_accounts: impl IntoIterator<Item = (Address, Account)>,
    ) -> TransactionResult {
        for (address, account) in pre_accounts {
            if self.set_account(address, account).is_err() {
                return Err(FailedTransactionMetadata {
                    err: TransactionError::InstructionError(
                        0,
                        solana_transaction::InstructionError::InvalidAccountData,
                    ),
                    meta: TransactionMetadata::default(),
                });
            }
        }
        self.process_instruction_chain(instructions)
    }

    /// Simulates one instruction without committing post-state.
    pub fn simulate_instruction(
        &self,
        instruction: Instruction,
    ) -> Result<SimulatedTransactionInfo, FailedTransactionMetadata> {
        self.simulate_instruction_chain([instruction])
    }

    /// Simulates one instruction with explicit temporary account states.
    pub fn simulate_instruction_with_accounts(
        &self,
        instruction: Instruction,
        pre_accounts: impl IntoIterator<Item = (Address, Account)>,
    ) -> Result<SimulatedTransactionInfo, FailedTransactionMetadata> {
        self.simulate_instruction_chain_with_accounts([instruction], pre_accounts)
    }

    /// Simulates multiple instructions atomically without committing post-state.
    pub fn simulate_instruction_chain(
        &self,
        instructions: impl IntoIterator<Item = Instruction>,
    ) -> Result<SimulatedTransactionInfo, FailedTransactionMetadata> {
        let mut working = self.clone();
        let ExecutionOutcome { meta, post_accounts, status, .. } =
            working.transact_instruction_chain_no_verify(instructions.into_iter().collect());
        if let Err(tx_err) = status {
            Err(FailedTransactionMetadata { err: tx_err, meta })
        } else {
            Ok(SimulatedTransactionInfo { meta, post_accounts })
        }
    }

    /// Simulates multiple instructions atomically with explicit temporary account states.
    pub fn simulate_instruction_chain_with_accounts(
        &self,
        instructions: impl IntoIterator<Item = Instruction>,
        pre_accounts: impl IntoIterator<Item = (Address, Account)>,
    ) -> Result<SimulatedTransactionInfo, FailedTransactionMetadata> {
        let mut working = self.clone();
        for (address, account) in pre_accounts {
            if working.set_account(address, account).is_err() {
                return Err(FailedTransactionMetadata {
                    err: TransactionError::InstructionError(
                        0,
                        solana_transaction::InstructionError::InvalidAccountData,
                    ),
                    meta: TransactionMetadata::default(),
                });
            }
        }
        let ExecutionOutcome { meta, post_accounts, status, .. } =
            working.transact_instruction_chain_no_verify(instructions.into_iter().collect());
        if let Err(tx_err) = status {
            Err(FailedTransactionMetadata { err: tx_err, meta })
        } else {
            Ok(SimulatedTransactionInfo { meta, post_accounts })
        }
    }

    /// Executes a signed transaction without committing its post-state.
    ///
    /// The returned [`ExecutionOutcome`] is bound to this VM instance and its
    /// current state version. Commit it back to the same [`HPSVM`] before any
    /// intervening state or config mutation. Otherwise
    /// [`HPSVM::commit_transaction`] returns `ResanitizationNeeded`.
    pub fn try_transact(
        &self,
        tx: impl Into<VersionedTransaction>,
    ) -> Result<ExecutionOutcome, HPSVMError> {
        let log_collector = Rc::new(RefCell::new(LogCollector {
            bytes_limit: self.runtime_env.log_bytes_limit,
            ..Default::default()
        }));
        let mut execution = if self.cfg.sigverify {
            self.execute_transaction_readonly(tx.into(), log_collector.clone())
        } else {
            self.execute_transaction_no_verify_readonly(tx.into(), log_collector.clone())
        };

        if let Some(error) = execution.fatal_error.take() {
            return Err(error);
        }

        Ok(execution_into_outcome(self, execution, log_collector, "try_transact"))
    }

    #[must_use = "call HPSVM::commit_transaction to apply the returned execution outcome"]
    pub fn transact(&self, tx: impl Into<VersionedTransaction>) -> ExecutionOutcome {
        self.transact_inner(tx.into())
    }

    /// Commits a previously transacted execution outcome to this VM instance.
    ///
    /// Outcomes are valid only for the VM instance and state version that
    /// produced them. If this VM mutated after [`HPSVM::transact`] created the
    /// outcome, or if the outcome came from a different VM instance, this
    /// returns `ResanitizationNeeded`.
    #[cfg_attr(feature = "hotpath", hotpath::measure)]
    pub fn commit_transaction(&mut self, outcome: ExecutionOutcome) -> TransactionResult {
        commit_execution_outcome(self, outcome)
    }

    /// Plans a conflict-aware transaction batch without committing any state.
    ///
    /// The returned stages contain indexes into the original input order. Stages
    /// are built greedily from account read/write conflicts and form the basis
    /// for higher-level batch schedulers.
    pub fn plan_transaction_batch<T>(
        &self,
        txs: impl IntoIterator<Item = T>,
    ) -> Result<TransactionBatchPlan, TransactionBatchError>
    where
        T: Into<VersionedTransaction>,
    {
        let transactions = txs.into_iter().map(Into::into).collect::<Vec<_>>();
        plan_transaction_batch(self, &transactions)
    }

    /// Submits a batch of transactions and returns a conflict-aware schedule.
    ///
    /// Execution results are returned in the original input order. Transactions
    /// in the same conflict-free stage are executed against cloned snapshots in
    /// parallel, then their disjoint account deltas are merged back into this VM
    /// before the next stage begins.
    pub fn send_transaction_batch<T>(
        &mut self,
        txs: impl IntoIterator<Item = T>,
    ) -> Result<TransactionBatchExecutionResult, TransactionBatchError>
    where
        T: Into<VersionedTransaction>,
    {
        let transactions = txs.into_iter().map(Into::into).collect::<Vec<_>>();
        send_transaction_batch(self, transactions)
    }

    /// Simulates a transaction without committing post-state.
    #[cfg_attr(feature = "hotpath", hotpath::measure)]
    pub fn simulate_transaction(
        &self,
        tx: impl Into<VersionedTransaction>,
    ) -> Result<SimulatedTransactionInfo, FailedTransactionMetadata> {
        let ExecutionOutcome { meta, post_accounts, status, .. } = self.transact(tx);
        if let Err(tx_err) = status {
            Err(FailedTransactionMetadata { err: tx_err, meta })
        } else {
            Ok(SimulatedTransactionInfo { meta, post_accounts })
        }
    }

    /// Expires the current blockhash.
    #[cfg_attr(feature = "hotpath", hotpath::measure)]
    pub fn expire_blockhash(&mut self) {
        self.block_env.latest_blockhash =
            create_blockhash(&self.block_env.latest_blockhash.to_bytes());
        #[expect(deprecated)]
        self.set_sysvar_internal(&RecentBlockhashes::from_iter([IterItem(
            0,
            &self.block_env.latest_blockhash,
            self.cfg.fee_structure.lamports_per_signature,
        )]));
        self.invalidate_execution_outcomes();
    }

    /// Warps the clock to the specified slot.
    pub fn warp_to_slot(&mut self, slot: u64) {
        let mut clock = self.get_sysvar::<Clock>();
        clock.slot = slot;
        self.set_sysvar_internal(&clock);
        self.invalidate_execution_outcomes();
    }

    /// Gets the current compute budget.
    pub const fn get_compute_budget(&self) -> Option<ComputeBudget> {
        self.runtime_env.compute_budget
    }

    /// Returns whether signature verification is enabled.
    pub const fn get_sigverify(&self) -> bool {
        self.cfg.sigverify
    }

    #[cfg(feature = "internal-test")]
    pub fn get_feature_set(&self) -> Arc<FeatureSet> {
        self.cfg.feature_set.clone().into()
    }

    pub(crate) fn check_transaction_age(
        &self,
        tx: &SanitizedTransaction,
    ) -> Result<(), ExecutionResult> {
        self.check_transaction_age_inner(tx)
            .map_err(|e| ExecutionResult { tx_result: Err(e), ..Default::default() })
    }

    pub(crate) fn check_transaction_age_inner(
        &self,
        tx: &SanitizedTransaction,
    ) -> solana_transaction_error::TransactionResult<()> {
        let recent_blockhash = tx.message().recent_blockhash();
        if recent_blockhash == &self.block_env.latest_blockhash ||
            self.check_transaction_for_nonce(
                tx,
                &DurableNonce::from_blockhash(&self.block_env.latest_blockhash),
            )
        {
            Ok(())
        } else {
            tracing::error!(
                "Blockhash {} not found. Expected blockhash {}",
                recent_blockhash,
                self.block_env.latest_blockhash
            );
            Err(TransactionError::BlockhashNotFound)
        }
    }

    pub(crate) fn check_message_for_nonce(&self, message: &SanitizedMessage) -> bool {
        SVMMessage::get_durable_nonce(message)
            .and_then(|nonce_address| self.accounts.get_account(nonce_address))
            .and_then(|nonce_account| {
                verify_nonce_account(&nonce_account, message.recent_blockhash())
            })
            .is_some_and(|nonce_data| {
                SVMStaticMessage::get_ix_signers(message, NONCED_TX_MARKER_IX_INDEX as usize)
                    .any(|signer| signer == &nonce_data.authority)
            })
    }

    pub(crate) fn check_transaction_for_nonce(
        &self,
        tx: &SanitizedTransaction,
        next_durable_nonce: &DurableNonce,
    ) -> bool {
        let nonce_is_advanceable = tx.message().recent_blockhash() != next_durable_nonce.as_hash();
        nonce_is_advanceable && self.check_message_for_nonce(tx.message())
    }

    #[cfg(feature = "invocation-inspect-callback")]
    pub fn set_invocation_inspect_callback<C: InvocationInspectCallback + 'static>(
        &mut self,
        callback: C,
    ) {
        self.invocation_inspect_callback = Arc::new(callback);
        self.invalidate_execution_outcomes();
    }

    /// **Advanced reconfiguration.** Registers a custom syscall in both program runtime
    /// environments (v1 and v2).
    ///
    /// This can be called on a freshly constructed [`HPSVM::new()`] instance or on an existing
    /// environment after programs have already been loaded. The runtime environments are refreshed
    /// and cached programs are rebuilt so subsequent executions see the new syscall.
    ///
    /// Returns an error if runtime refresh, syscall registration, or program cache
    /// rebuilding fails.
    pub fn register_custom_syscall(
        &mut self,
        name: &str,
        syscall: BuiltinFunctionRegisterer,
    ) -> Result<(), HPSVMError> {
        self.runtime_registry.register_custom_syscall(CustomSyscallRegistration {
            name: name.to_owned(),
            function: syscall,
        });

        self.try_refresh_runtime_environments()?;
        self.accounts.rebuild_program_cache().map_err(HPSVMError::from)?;
        self.invalidate_execution_outcomes();

        Ok(())
    }
}

impl HPSVM {
    #[cfg_attr(feature = "hotpath", hotpath::measure)]
    pub(crate) fn transact_inner(&self, tx: VersionedTransaction) -> ExecutionOutcome {
        let log_collector = Rc::new(RefCell::new(LogCollector {
            bytes_limit: self.runtime_env.log_bytes_limit,
            ..Default::default()
        }));
        let execution = if self.cfg.sigverify {
            self.execute_transaction_readonly(tx, log_collector.clone())
        } else {
            self.execute_transaction_no_verify_readonly(tx, log_collector.clone())
        };
        execution_into_outcome(self, execution, log_collector, "transact")
    }

    pub(crate) fn transact_instruction_chain_no_verify(
        &mut self,
        instructions: Vec<Instruction>,
    ) -> ExecutionOutcome {
        let tx = self.instruction_chain_transaction(&instructions);
        let log_collector = Rc::new(RefCell::new(LogCollector {
            bytes_limit: self.runtime_env.log_bytes_limit,
            ..Default::default()
        }));
        let sigverify = self.cfg.sigverify;
        self.cfg.sigverify = false;
        let execution = self.execute_transaction_no_verify(tx, log_collector.clone());
        self.cfg.sigverify = sigverify;
        execution_into_outcome(self, execution, log_collector, "process_instruction_chain")
    }

    pub(crate) fn instruction_chain_transaction(
        &self,
        instructions: &[Instruction],
    ) -> VersionedTransaction {
        let fee_payer = fee_payer_for_instructions(instructions, self.airdrop_pubkey());
        let message = Message::new_with_blockhash(
            instructions,
            Some(&fee_payer),
            &self.block_env.latest_blockhash,
        );
        let signatures =
            vec![Signature::default(); usize::from(message.header.num_required_signatures)];
        VersionedTransaction { signatures, message: VersionedMessage::Legacy(message) }
    }
}

fn execution_into_outcome(
    vm: &HPSVM,
    execution: ExecutionResult,
    log_collector: Rc<RefCell<LogCollector>>,
    method_name: &str,
) -> ExecutionOutcome {
    let ExecutionResult {
        post_accounts,
        tx_result,
        signature,
        compute_units_consumed,
        inner_instructions,
        return_data,
        execution_trace,
        included,
        fee,
        fee_payer,
        account_source_failures,
        fatal_error: _,
    } = execution;
    let Ok(logs) = Rc::try_unwrap(log_collector).map(|collector| collector.into_inner().messages)
    else {
        unreachable!("Log collector should not be used after {method_name} returns")
    };

    ExecutionOutcome {
        meta: TransactionMetadata {
            signature,
            logs,
            inner_instructions,
            compute_units_consumed,
            return_data,
            fee,
            diagnostics: if vm.cfg.compute_diagnostics {
                hotpath_block!("hpsvm::execution_into_outcome::diagnostics", {
                    execution_diagnostics(
                        vm,
                        &post_accounts,
                        execution_trace,
                        account_source_failures,
                    )
                })
            } else {
                // Diagnostics disabled: carry over the cheap trace and any
                // account-source failures, but skip the expensive pre/post
                // balance, account-diff, and token-balance computation.
                ExecutionDiagnostics {
                    execution_trace,
                    account_source_failures,
                    ..Default::default()
                }
            },
        },
        post_accounts,
        status: tx_result,
        included,
        origin_vm_instance_id: vm.instance_id,
        origin_state_version: vm.state_version,
        fee_payer,
    }
}

fn execution_diagnostics(
    vm: &HPSVM,
    post_accounts: &[(Address, AccountSharedData)],
    execution_trace: ExecutionTrace,
    account_source_failures: Vec<AccountSourceFailure>,
) -> ExecutionDiagnostics {
    // ponytail: single pass over post_accounts — load pre-state once, compute
    // balances and diffs without redundant clones.
    let mut pre_balances = Vec::with_capacity(post_accounts.len());
    let mut post_balances = Vec::with_capacity(post_accounts.len());
    let mut account_diffs = Vec::new();
    let mut pre_accounts = Vec::with_capacity(post_accounts.len());

    for (address, post) in post_accounts {
        let pre = vm.accounts.get_account(address).unwrap_or_default();
        pre_balances.push(pre.lamports());
        post_balances.push(post.lamports());
        if accounts_differ(&pre, post) {
            account_diffs.push(AccountDiff {
                address: *address,
                pre: public_account_from_shared(&pre),
                post: public_account_from_shared(post),
            });
        }
        pre_accounts.push((*address, pre));
    }

    ExecutionDiagnostics {
        pre_balances,
        post_balances,
        account_diffs,
        account_source_failures,
        pre_token_balances: hotpath_block!("hpsvm::diagnostics::pre_token_balances", {
            token_balances(&pre_accounts, &vm.accounts)
        }),
        post_token_balances: hotpath_block!("hpsvm::diagnostics::post_token_balances", {
            token_balances(post_accounts, &vm.accounts)
        }),
        execution_trace,
    }
}

fn fee_payer_for_instruction_case(case: &InstructionCase) -> Address {
    case.accounts
        .iter()
        .find(|account| account.is_signer)
        .or_else(|| case.accounts.iter().find(|account| account.is_writable))
        .or_else(|| case.accounts.first())
        .map(|account| account.pubkey)
        .unwrap_or_else(Address::new_unique)
}

#[cfg(test)]
mod tests {
    use solana_instruction::{Instruction, account_meta::AccountMeta};
    use solana_message::{Message, VersionedMessage};
    use solana_signer::Signer;
    use solana_system_interface::{instruction::transfer, program as system_program};
    use solana_transaction::{InstructionError, Transaction};

    use super::*;

    #[test]
    pub(crate) fn sysvar_accounts_are_demoted_to_readonly() {
        let payer = Keypair::new();
        let svm = HPSVM::new();
        let rent_key = solana_sdk_ids::sysvar::rent::id();
        let ix = Instruction {
            program_id: solana_sdk_ids::system_program::id(),
            accounts: vec![AccountMeta { pubkey: rent_key, is_signer: false, is_writable: true }],
            data: vec![],
        };
        let message = Message::new(&[ix], Some(&payer.pubkey()));
        let tx =
            VersionedTransaction::try_new(VersionedMessage::Legacy(message), &[&payer]).unwrap();
        let sanitized = svm.sanitize_transaction_no_verify(tx).unwrap();

        assert!(!sanitized.message().is_writable(1));
    }

    #[test]
    pub(crate) fn with_transaction_origin_restores_previous_origin_after_panic() {
        let mut svm = HPSVM::new();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            svm.with_transaction_origin(TransactionOrigin::InternalAirdrop, |_| {
                panic!("boom");
            });
        }));

        assert!(result.is_err());
        assert_eq!(svm.inspection_origin, TransactionOrigin::User);
    }

    #[test]
    pub(crate) fn set_feature_set_rolls_back_failed_reconfiguration() {
        let mut svm = HPSVM::new();
        let payer = Keypair::new();
        let recipient = Address::new_unique();
        let original_feature_set = svm.cfg.feature_set.clone();
        let original_reserved_account_keys = svm.reserved_account_keys.clone();

        svm.airdrop(&payer.pubkey(), 10_000).unwrap();
        let tx = Transaction::new(
            &[&payer],
            Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
            svm.latest_blockhash(),
        );
        let outcome = svm.transact(tx);

        let poisoned_program = Address::new_unique();
        let mut invalid_program = AccountSharedData::new(1, 0, &system_program::id());
        invalid_program.set_executable(true);
        svm.accounts.add_account_no_checks(poisoned_program, invalid_program);

        let err = svm
            .set_feature_set(FeatureSet::all_enabled())
            .expect_err("invalid cached program should abort feature-set reconfiguration");

        assert!(matches!(err, HPSVMError::Instruction(InstructionError::IncorrectProgramId)));
        assert_eq!(svm.cfg.feature_set, original_feature_set);
        assert_eq!(svm.reserved_account_keys.active, original_reserved_account_keys.active);

        let result = svm.commit_transaction(outcome);

        assert!(result.is_ok());
        assert_eq!(svm.get_balance(&recipient), Some(64));
    }
}
