//! Core [`Svm`] behaviour: transaction execution entry points, account and
//! sysvar state management, program loading, and the public transaction API.

use std::{cell::RefCell, marker::PhantomData, path::Path, rc::Rc, sync::Arc};

use agave_feature_set::{FeatureSet, raise_cpi_nesting_limit_to_8};
use agave_reserved_account_keys::ReservedAccountKeys;
use parking_lot::Mutex;
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
use solana_native_token::LAMPORTS_PER_SOL;
use solana_nonce::{NONCED_TX_MARKER_IX_INDEX, state::DurableNonce};
use solana_nonce_account::verify_nonce_account;
use solana_program_runtime::{
    invoke_context::BuiltinFunctionRegisterer, program_cache_entry::ProgramCacheEntry,
};
use solana_rent::Rent;
use solana_sdk_ids::{
    bpf_loader, bpf_loader_deprecated, bpf_loader_upgradeable, native_loader, system_program,
};
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
use crate::precompile_registry::NativePrecompiles;
#[cfg(not(feature = "precompiles"))]
use crate::precompile_registry::NoPrecompiles;
#[cfg(feature = "precompiles")]
use crate::precompiles::load_precompiles;
#[cfg(feature = "register-tracing")]
use crate::register_tracing::DefaultRegisterTracingCallback;
use crate::{
    AccountSource, AccountSourceError, AccountsView, BlockEnv, ClusterInfo, ClusterSpecId,
    CustomSyscallRegistration, Hpsvm, HpsvmBuilder, InspectMode, Inspector, RuntimeLimits, Svm,
    SvmTypesHost, TransactionOrigin, Version,
    account_source::EmptyAccountSource,
    batch::{
        TransactionBatchError, TransactionBatchExecutionResult, TransactionBatchPlan,
        plan_transaction_batch, send_transaction_batch,
    },
    commit::detached_into_result_and_delta,
    error::HPSVMError,
    executed::{DetachedTx, ExecutedTx, StagedFailure, staging_touches_caches},
    helpers::*,
    history::TransactionHistory,
    instruction::InstructionCase,
    precompile_registry::PrecompileProvider,
    programs::{DEFAULT_PROGRAM_IDS, SPL_PROGRAM_IDS, load_default_programs, load_spl_programs},
    runtime_registry::RuntimeExtensionRegistry,
    tx_registry::{TxHandler, TxRegistryError, TxTag, tag_of},
    types::{
        AccountDiff, AccountSourceFailure, ExecutionDiagnostics, ExecutionResult, ExecutionTrace,
        FailedTransactionMetadata, SimulatedTransactionInfo, TransactionMetadata,
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

impl Hpsvm {
    /// Creates the basic test environment.
    #[cfg_attr(feature = "hotpath", hotpath::measure)]
    pub fn new() -> Self {
        Self::builder()
            .with_program_test_defaults()
            .build()
            .expect("standard Svm construction should remain infallible")
    }

    /// Create a typed builder for explicit, compile-time-checked environment assembly.
    pub fn builder() -> HpsvmBuilder {
        HpsvmBuilder::new()
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
            .expect("standard debuggable Svm construction should remain infallible")
    }
}

impl<'a, T: SvmTypesHost> Svm<'a, T> {
    pub(crate) fn default_register_tracing_enabled() -> bool {
        // Allow users to virtually get register tracing data without doing any
        // changes to their code provided `SBF_TRACE_DIR` is set.
        #[cfg(feature = "register-tracing")]
        {
            std::env::var("SBF_TRACE_DIR").is_ok()
        }
        #[cfg(not(feature = "register-tracing"))]
        {
            false
        }
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
            version: Version {
                feature_set: Arc::new(feature_set),
                sigverify: false,
                blockhash_check: false,
                fee_structure: FeeStructure::default(),
                compute_diagnostics: true,
                compute_budget: None,
                rent: Rent::default(),
                limits: RuntimeLimits::default(),
                cluster: ClusterInfo {
                    spec_id: ClusterSpecId::MainnetBeta,
                    name: ClusterSpecId::MainnetBeta.name(),
                },
            },
            feature_accounts_loaded: false,
            inspector: Mutex::new(None),
            inspection_origin: TransactionOrigin::User,
            runtime_registry: RuntimeExtensionRegistry::default(),
            registry: crate::tx_registry::TxRegistry::with_defaults(),
            block_env: BlockEnv {
                latest_blockhash,
                slot: 0,
                // `Default::default()` is `()` for the base family, so the
                // base path keeps constructing exactly the old value.
                ext: T::BlockEnvExt::default(),
            },
            history: TransactionHistory::new(),
            sysvars_loaded: false,
            #[cfg(feature = "precompiles")]
            precompiles: Arc::new(NativePrecompiles::new()),
            #[cfg(not(feature = "precompiles"))]
            precompiles: Arc::new(NoPrecompiles),
            _type_family: PhantomData,
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

    /// Creates a fully-initialized test environment for any type family.
    ///
    /// Mirrors [`Hpsvm::new`] (program-test defaults: every feature on, a
    /// funded airdrop account, sysvars, builtins, default programs, and —
    /// with the `precompiles` feature — the native precompile set) for an
    /// arbitrary `T`, so a custom family stands up in one call instead of
    /// replaying the builder steps (the builder only builds `BaseSvmTypes`).
    /// Signature verification and blockhash checks stay on, exactly like
    /// `Hpsvm::new`.
    pub fn new_with_defaults() -> Self {
        let mut svm = Self::new_inner(false);
        svm.set_feature_set(FeatureSet::all_enabled())
            .expect("enabling every feature should succeed on a fresh VM");
        svm.set_builtins();
        svm.set_lamports(1_000_000 * LAMPORTS_PER_SOL);
        svm.set_sysvars();
        svm.set_feature_accounts();
        svm.set_default_programs();
        #[cfg(feature = "precompiles")]
        svm.set_precompiles();
        svm.set_sigverify(true);
        svm.set_blockhash_check(true);
        svm
    }

    /// Installs an execution inspector that observes transaction activity.
    ///
    /// Builder-style sugar over [`set_inspector`](Svm::set_inspector): moves
    /// `inspector` into the VM after firing its `initialize` hook.
    pub fn with_inspector<I: Inspector<T> + 'a>(mut self, inspector: I) -> Self {
        self.set_inspector(inspector);
        self
    }

    /// Installs `inspector`, firing its `initialize` hook first.
    ///
    /// Replaces any previously installed inspector (use
    /// [`clear_inspector`](Svm::clear_inspector) first to recover it).
    pub fn set_inspector(&mut self, inspector: impl Inspector<T> + 'a) {
        self.set_boxed_inspector(Box::new(inspector));
    }

    /// Installs an already-boxed inspector, firing its `initialize` hook
    /// first. Used by the builder, which holds the plan-time inspector boxed.
    pub(crate) fn set_boxed_inspector(&mut self, mut inspector: Box<dyn Inspector<T> + 'a>) {
        inspector.initialize(self);
        *self.inspector.lock() = Some(inspector);
    }

    /// Removes the installed inspector and returns it, if any.
    ///
    /// State the inspector accumulated stays observable through the returned
    /// box; downcast it back with `downcast_ref` to read the concrete type.
    pub fn clear_inspector(&mut self) -> Option<Box<dyn Inspector<T> + 'a>> {
        self.inspector.lock().take()
    }

    /// Returns whether the installed inspector accepts the current
    /// [`TransactionOrigin`].
    ///
    /// Consulted once per execution, at the [`InspectMode`] selection point
    /// (`execute_readonly`), which monomorphizes the whole execution below it
    /// over the answer — so an origin-observing inspector still sees exactly
    /// one consultation per execution, and no hook dispatcher consults again.
    #[inline]
    pub(crate) fn inspector_live(&self) -> bool {
        let origin = self.inspection_origin;
        self.inspector.lock().as_ref().is_some_and(|inspector| inspector.accepts_origin(origin))
    }

    /// Runs `op` against the installed inspector, if any.
    ///
    /// Takes the box out, runs the hook with the lock released, and puts the
    /// box back — restoring it even when the hook panics. A hook that
    /// re-enters execution therefore observes no inspector rather than
    /// deadlocking on the lock.
    fn fire_inspector(&self, op: impl FnOnce(&mut (dyn Inspector<T> + '_), &Self)) {
        let Some(mut inspector) = self.inspector.lock().take() else { return };
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| op(&mut *inspector, self)));
        *self.inspector.lock() = Some(inspector);
        if let Err(payload) = outcome {
            std::panic::resume_unwind(payload);
        }
    }

    pub(crate) fn on_transaction_start<const MODE: InspectMode>(&self, tx: &SanitizedTransaction) {
        match MODE {
            InspectMode::DynInspector => {
                self.fire_inspector(|inspector, svm| inspector.transaction_start(svm, tx));
            }
            InspectMode::NoInspector => {}
        }
    }

    pub(crate) fn on_instruction_start<const MODE: InspectMode>(
        &self,
        index: usize,
        program_id: &Address,
    ) {
        match MODE {
            InspectMode::DynInspector => {
                self.fire_inspector(|inspector, _| inspector.instruction_start(index, program_id));
            }
            InspectMode::NoInspector => {}
        }
    }

    pub(crate) fn on_instruction_end<const MODE: InspectMode>(
        &self,
        index: usize,
        program_id: &Address,
    ) {
        match MODE {
            InspectMode::DynInspector => {
                self.fire_inspector(|inspector, _| inspector.instruction_end(index, program_id));
            }
            InspectMode::NoInspector => {}
        }
    }

    pub(crate) fn on_log<const MODE: InspectMode>(&self, message: &str) {
        match MODE {
            InspectMode::DynInspector => {
                self.fire_inspector(|inspector, _| inspector.log(message));
            }
            InspectMode::NoInspector => {}
        }
    }

    pub(crate) fn on_account_change<const MODE: InspectMode>(
        &self,
        change: crate::state_change_sink::AccountChange<'_>,
    ) {
        match MODE {
            InspectMode::DynInspector => {
                self.fire_inspector(|inspector, _| inspector.account_change(change));
            }
            InspectMode::NoInspector => {}
        }
    }

    pub(crate) fn on_transaction_end<const MODE: InspectMode>(
        &self,
        result: &solana_transaction_error::TransactionResult<()>,
    ) {
        match MODE {
            InspectMode::DynInspector => {
                self.fire_inspector(|inspector, svm| inspector.transaction_end(svm, result));
            }
            InspectMode::NoInspector => {}
        }
    }

    pub(crate) fn with_transaction_origin<O>(
        &mut self,
        origin: TransactionOrigin,
        op: impl FnOnce(&mut Self) -> O,
    ) -> O {
        let previous_origin = self.inspection_origin;
        self.inspection_origin = origin;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| op(self)));
        self.inspection_origin = previous_origin;
        match result {
            Ok(result) => result,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    pub(crate) fn with_temporary_account_source<O>(
        &mut self,
        source: Arc<dyn AccountSource>,
        op: impl FnOnce(&mut Self) -> O,
    ) -> O {
        let previous_source = self.accounts.replace_account_source(source);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| op(self)));
        self.accounts.set_account_source(previous_source);
        match result {
            Ok(result) => result,
            Err(payload) => std::panic::resume_unwind(payload),
        }
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
        for program_id in self.precompiles.program_ids() {
            self.accounts.remove_account(&program_id);
        }
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

    /// **Advanced reconfiguration.** Replaces the runtime compute budget.
    pub const fn set_compute_budget(&mut self, compute_budget: ComputeBudget) {
        self.version.compute_budget = Some(compute_budget);
    }

    /// **Advanced reconfiguration.** Replaces only the runtime compute unit limit.
    pub fn set_compute_unit_limit(&mut self, compute_unit_limit: u64) {
        let mut compute_budget = self.version.compute_budget.unwrap_or_else(|| {
            // Setup-time one-shot query, not on the per-transaction path:
            // keep the direct `is_active` instead of a `TxFeatureFlags` load.
            ComputeBudget::new_with_defaults(
                self.version.feature_set.is_active(&raise_cpi_nesting_limit_to_8::ID),
            )
        });
        compute_budget.compute_unit_limit = compute_unit_limit;
        self.set_compute_budget(compute_budget);
    }

    /// **Advanced reconfiguration.** Enables or disables signature verification for future
    /// transactions.
    pub const fn set_sigverify(&mut self, sigverify: bool) {
        self.version.sigverify = sigverify;
    }

    /// **Advanced reconfiguration.** Enables or disables blockhash checking for future
    /// transactions.
    pub const fn set_blockhash_check(&mut self, check: bool) {
        self.version.blockhash_check = check;
    }

    /// **Advanced reconfiguration.** Enables or disables execution diagnostics for future
    /// transactions.
    ///
    /// When diagnostics are disabled, `send_transaction` / `transact` returns an empty
    /// [`ExecutionDiagnostics`] instead of computing pre/post
    /// account diffs and token balances.
    pub const fn set_compute_diagnostics(&mut self, enabled: bool) {
        self.version.compute_diagnostics = enabled;
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

        // Rent account differs based off feature gating.
        // Setup-time one-shot query (sysvar initialization, not per
        // transaction): keep the direct `is_active` instead of a
        // `TxFeatureFlags` load.
        #[expect(deprecated)]
        {
            let mut rent_account = Rent::default();
            if self
                .version
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
    }

    /// **Advanced reconfiguration.** Replaces the active feature set and rebuilds any
    /// materialized feature-dependent state.
    pub fn set_feature_set(&mut self, feature_set: FeatureSet) -> Result<(), HPSVMError> {
        let previous_feature_set = Arc::clone(&self.version.feature_set);
        let previous_accounts = self.accounts.clone();
        let previous_reserved_account_keys = self.reserved_account_keys.clone();

        self.version.feature_set = Arc::new(feature_set);
        self.reserved_account_keys =
            Self::reserved_account_keys_for_feature_set(&self.version.feature_set);
        if let Err(error) = self.reconfigure_materialized_feature_state(&previous_feature_set) {
            self.version.feature_set = previous_feature_set;
            self.reserved_account_keys = previous_reserved_account_keys;
            self.accounts = previous_accounts;
            // The program cache is shared through an `Arc`, so the failed
            // reconfigure may have mutated it through the backup's handle.
            // The cache is derived from the overlay, so rebuild it from the
            // restored accounts to recover the pre-call contents.
            if let Err(rebuild_error) = self.accounts.rebuild_program_cache() {
                tracing::error!(
                    ?rebuild_error,
                    "failed to restore program cache after set_feature_set rollback"
                );
            }
            return Err(error);
        }

        Ok(())
    }

    pub(crate) fn materialize_feature_accounts(&mut self) {
        self.feature_accounts_loaded = true;
        for (feature_id, activation_slot) in self.version.feature_set.active() {
            let feature_account = Feature { activated_at: Some(*activation_slot) };
            let lamports = self.minimum_balance_for_rent_exemption(Feature::size_of());
            let account = feature_gate::create_account(&feature_account, lamports);
            self.accounts.add_account_no_checks(*feature_id, account);
        }
    }

    pub(crate) fn set_feature_accounts(&mut self) {
        self.materialize_feature_accounts();
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
            // Dynamic id: each builtin carries its own `enable_feature_id`,
            // which cannot be enumerated into the fixed `TxFeatureFlags` set.
            // Setup-time only, so the direct query stays.
            if builtint.enable_feature_id.is_none_or(|x| self.version.feature_set.is_active(&x)) {
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
    }

    pub fn set_lamports(&mut self, lamports: u64) {
        self.accounts.add_account_no_checks(
            Keypair::try_from(self.airdrop_kp.as_slice())
                .expect("airdrop keypair should be valid")
                .pubkey(),
            AccountSharedData::new(lamports, 0, &system_program::id()),
        );
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
    }

    pub(crate) fn set_spl_programs(&mut self) {
        self.load_spl_programs();
    }

    /// **Advanced reconfiguration.** Changes the transaction history capacity. Set this to 0 to
    /// disable history and allow duplicate transactions.
    pub fn set_transaction_history(&mut self, capacity: usize) {
        self.history.set_capacity(capacity);
    }

    /// **Advanced reconfiguration.** Installs a read-through account source for future lookups.
    pub fn set_account_source(&mut self, source: impl AccountSource + 'static) {
        self.accounts.set_account_source(Arc::new(source));
    }

    /// **Advanced reconfiguration.** Adjusts the log truncation limit for future execution.
    pub const fn set_log_bytes_limit(&mut self, limit: Option<usize>) {
        self.version.limits.log_bytes_limit = limit;
    }

    #[cfg(feature = "precompiles")]
    pub(crate) fn load_precompiles(&mut self) {
        load_precompiles(self);
    }

    #[cfg(feature = "precompiles")]
    pub(crate) fn set_precompiles(&mut self) {
        self.runtime_registry.enable_standard_precompiles();
        self.load_precompiles();
    }

    /// Installs a custom precompile provider, replacing the default dispatch set.
    ///
    /// [`is_precompile`](solana_svm_callback::InvokeContextCallback::is_precompile)
    /// and dispatch consult the new provider from this call on. When the
    /// `precompiles` feature is on, accounts for the provider's enabled ids
    /// are installed too, so entries under brand-new program ids work out of
    /// the box; previously installed precompile accounts are left in place.
    /// With the feature off the swap is inert: nothing dispatches as a
    /// precompile either way, exactly as before.
    pub fn set_precompile_provider(&mut self, provider: Arc<dyn PrecompileProvider<T>>) {
        self.precompiles = provider;
        #[cfg(feature = "precompiles")]
        self.load_precompiles();
    }

    /// Returns the active precompile provider.
    pub fn precompile_provider(&self) -> Arc<dyn PrecompileProvider<T>> {
        Arc::clone(&self.precompiles)
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
    /// transactions. Prefer [`airdrop`](Hpsvm::airdrop) or
    /// [`send_transaction`](Hpsvm::send_transaction) when you want a
    /// protocol-consistent state transition.
    #[cfg_attr(feature = "hotpath", hotpath::measure)]
    pub fn set_account(&mut self, address: Address, data: Account) -> Result<(), HPSVMError> {
        self.accounts.add_account(address, data.into())?;
        self.sync_block_env_slot();
        Ok(())
    }

    /// **⚠️ ADVANCED USE ONLY ⚠️**
    ///
    /// Returns a read-only view of the internal accounts database.
    ///
    /// This provides read-only access to the accounts database for advanced inspection.
    /// Use [`get_account`](Hpsvm::get_account) for normal account retrieval.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hpsvm::Hpsvm;
    ///
    /// let svm = Hpsvm::new();
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
    pub const fn block_env(&self) -> BlockEnv<T::BlockEnvExt> {
        self.block_env
    }

    /// **Advanced reconfiguration.** Replaces the block-environment extension slot.
    ///
    /// The core `latest_blockhash`/`slot` are untouched; only the
    /// family-selected `T::BlockEnvExt` changes. Custom families use this to
    /// publish chain-specific block data, which then reads back through
    /// [`block_env`](Svm::block_env) and [`Host::block_env`](crate::Host::block_env).
    pub fn set_block_env_ext(&mut self, ext: T::BlockEnvExt) {
        self.block_env.ext = ext;
    }

    /// Returns the execution-shaping protocol configuration this VM runs under.
    ///
    /// Custom environments start from [`Version::base`] and install overrides
    /// (notably via [`Version::with_feature`]) through
    /// [`set_feature_set`](Svm::set_feature_set); this accessor makes the
    /// installed configuration observable.
    #[inline]
    pub fn version(&self) -> &Version {
        &self.version
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
    pub fn set_sysvar<S>(&mut self, sysvar: &S) -> Result<(), HPSVMError>
    where
        S: Default + Serialize + SysvarId,
    {
        self.try_set_sysvar(sysvar)?;
        self.sync_block_env_slot();
        Ok(())
    }

    pub(crate) fn try_set_sysvar<S>(&mut self, sysvar: &S) -> Result<(), HPSVMError>
    where
        S: Default + Serialize + SysvarId,
    {
        // The buffer is sized to the sysvar's canonical on-chain length, then the value
        // is encoded into it with the same bincode codec the read path decodes.
        let mut account = AccountSharedData::new(
            1,
            sysvar_account_size::<S>(&S::id()),
            &solana_sdk_ids::sysvar::id(),
        );
        account.serialize_data(sysvar).map_err(|error| HPSVMError::SysvarSerialization {
            sysvar: std::any::type_name::<S>(),
            reason: error.to_string(),
        })?;
        self.accounts.add_account(S::id(), account)
    }

    pub(crate) fn set_sysvar_internal<S>(&mut self, sysvar: &S)
    where
        S: Default + Serialize + SysvarId,
    {
        self.try_set_sysvar(sysvar)
            .expect("internal sysvar setup should never fail for supported sysvars");
        self.sync_block_env_slot();
    }

    /// Gets a sysvar from the test environment.
    pub fn get_sysvar<S>(&self) -> S
    where
        S: Sysvar + SysvarId + DeserializeOwned,
    {
        self.try_get_sysvar().expect("sysvar account should exist and deserialize")
    }

    pub(crate) fn try_get_sysvar<S>(&self) -> Result<S, HPSVMError>
    where
        S: Sysvar + SysvarId + DeserializeOwned,
    {
        let account = self
            .accounts
            .get_account_ref(&S::id())
            .ok_or(HPSVMError::MissingRuntimeComponent { component: "sysvars" })?;

        account.deserialize_data().map_err(|error| HPSVMError::SysvarSerialization {
            sysvar: std::any::type_name::<S>(),
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
    /// Unlike [`set_account`](Hpsvm::set_account), this goes through the normal
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
    ///
    /// The installed account is owned by the native loader (exactly like the
    /// accounts installed for Agave's own builtins), so the runtime dispatches
    /// invocations of `program_id` to `entrypoint` through the builtin path.
    pub fn add_builtin(&mut self, program_id: Address, entrypoint: BuiltinFunctionRegisterer) {
        let builtin = ProgramCacheEntry::new_builtin(entrypoint);

        self.accounts.replenish_program_cache(program_id, Arc::new(builtin));

        let mut account = AccountSharedData::new(1, 1, &native_loader::id());
        account.set_executable(true);
        self.accounts.add_account_no_checks(program_id, account);
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
        match Self::transact_core(self, tx.into()) {
            Ok(staged) => staged.commit(),
            // Fatal errors (e.g. account-source failures) historically surfaced
            // as an `Err` result rather than propagating, so return the
            // prebuilt result instead of the error.
            Err(failure) => failure.into_result(),
        }
    }

    /// Executes a single instruction case against a cloned VM without mutating this instance.
    pub fn process_instruction_case(
        &self,
        case: &InstructionCase,
    ) -> Result<DetachedTx, HPSVMError> {
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
    /// instruction post-state is committed just like [`Hpsvm::process_instruction`].
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
        let tx = self.instruction_chain_transaction(&instructions.into_iter().collect::<Vec<_>>());
        let sigverify = self.version.sigverify;
        self.version.sigverify = false;
        let outcome = Self::transact_core(self, tx);
        // The borrow held by `outcome` ends when it is resolved below, so the
        // sigverify flag can only be restored afterwards.
        let result = match outcome {
            Ok(staged) => staged.commit(),
            Err(failure) => failure.into_result(),
        };
        self.version.sigverify = sigverify;
        result
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
        // A throwaway clone: disable sigverify permanently instead of the
        // save/restore dance the committing path performs on live state.
        working.version.sigverify = false;
        let tx =
            working.instruction_chain_transaction(&instructions.into_iter().collect::<Vec<_>>());
        let detached = working.transact_detached(tx);
        if let Err(tx_err) = detached.status {
            Err(FailedTransactionMetadata { err: tx_err, meta: detached.meta })
        } else {
            Ok(SimulatedTransactionInfo {
                meta: detached.meta,
                post_accounts: detached.post_accounts,
            })
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
        working.version.sigverify = false;
        let tx =
            working.instruction_chain_transaction(&instructions.into_iter().collect::<Vec<_>>());
        let detached = working.transact_detached(tx);
        if let Err(tx_err) = detached.status {
            Err(FailedTransactionMetadata { err: tx_err, meta: detached.meta })
        } else {
            Ok(SimulatedTransactionInfo {
                meta: detached.meta,
                post_accounts: detached.post_accounts,
            })
        }
    }

    /// Executes a signed transaction without staging or committing any state.
    ///
    /// Returns the owned readonly outcome. Fatal errors (e.g. account-source
    /// failures) propagate as [`HPSVMError`]; every other execution result —
    /// including pre-execution rejections — arrives as an `Ok` detached
    /// outcome whose [`status`](DetachedTx::status) carries the failure.
    pub fn try_transact(
        &self,
        tx: impl Into<VersionedTransaction>,
    ) -> Result<DetachedTx, HPSVMError> {
        let mut detached = self.dispatch_detached(tx.into()).map_err(HPSVMError::from)?;
        if let Some(error) = detached.take_fatal() {
            return Err(error);
        }
        Ok(detached)
    }

    /// Executes a signed transaction and stages its post-state in the live overlay.
    ///
    /// The returned [`ExecutedTx`] borrows `self`: while it is alive no other
    /// code can observe or mutate the VM, which replaces the old
    /// `origin_vm_instance_id` / `origin_state_version` provenance check.
    /// Resolve it with [`commit`](ExecutedTx::commit) to keep the staged
    /// post-state or [`discard`](ExecutedTx::discard) to roll it back.
    /// Fatal errors (e.g. account-source failures) stage nothing and propagate
    /// as [`HPSVMError`].
    #[cfg_attr(feature = "hotpath", hotpath::measure)]
    pub fn transact<'svm>(
        &'svm mut self,
        tx: impl Into<VersionedTransaction>,
    ) -> Result<ExecutedTx<'svm, 'a, T>, HPSVMError> {
        Self::transact_core(self, tx.into()).map_err(StagedFailure::into_error)
    }

    /// Executes a signed transaction without staging or committing any state.
    ///
    /// Infallible readonly counterpart to [`transact`](Svm::transact): fatal
    /// errors surface as an `Err` execution status inside the returned
    /// [`DetachedTx`], exactly as the legacy `transact` reported them.
    pub fn transact_detached(&self, tx: impl Into<VersionedTransaction>) -> DetachedTx {
        let tx = tx.into();
        let signature = tx.signatures.first().copied().unwrap_or_default();
        match self.dispatch_detached(tx) {
            Ok(detached) => detached,
            Err(error) => DetachedTx::for_dispatch_failure(&error, signature),
        }
    }

    /// Registers a transaction handler for `tag`, replacing the previous entry.
    ///
    /// This is how tests and forks intercept execution: registering for
    /// [`TxTag::Legacy`] reroutes every legacy
    /// transaction through `handle`, while unmentioned tags keep their
    /// current handlers.
    pub fn register_tx_handler(
        &mut self,
        tag: TxTag,
        handle: impl TxHandler<T, VersionedTransaction, DetachedTx> + 'static,
    ) {
        self.registry.register(tag, handle);
    }

    /// Routes one envelope through the handler registry without staging.
    ///
    /// The tag lookup borrows only the registry while the host borrows the
    /// whole VM; both are shared borrows, so this serves `&self` and `&mut
    /// self` entry points alike.
    pub(crate) fn dispatch_detached(
        &self,
        tx: VersionedTransaction,
    ) -> Result<DetachedTx, TxRegistryError> {
        let tag = tag_of(&tx);
        self.registry.dispatch(tag, tx, self)
    }

    /// Runs the current readonly execution path and converts it to the owned
    /// detached outcome, preserving the fatal-error and inspector-liveness
    /// transports for the entry points.
    ///
    /// This is what [`DefaultExecutionHandler`](crate::DefaultExecutionHandler)
    /// invokes, so the default registry entries behave exactly as the
    /// pre-registry hardcoded path did.
    pub(crate) fn default_execute_detached(&self, tx: VersionedTransaction) -> DetachedTx {
        let (execution, log_collector) = self.execute_readonly(tx);
        execution_into_detached(self, execution, log_collector, "tx_registry::default_execute")
    }

    /// Shared staging core behind `transact`, `send_transaction`, and
    /// `process_instruction_chain`.
    ///
    /// Runs the existing readonly execution, then stages `post_accounts` into
    /// the live overlay (journaled) and splits the history entry off for
    /// commit time. Diagnostics are built before staging so the pre-balances
    /// still observe the pre-transaction overlay.
    fn transact_core<'svm, 'host>(
        svm: &'svm mut Svm<'host, T>,
        tx: VersionedTransaction,
    ) -> Result<ExecutedTx<'svm, 'host, T>, StagedFailure> {
        let signature = tx.signatures.first().copied().unwrap_or_default();
        let mut detached = match svm.dispatch_detached(tx) {
            Ok(detached) => detached,
            Err(error) => {
                // No handler ran, so there is no execution data to stage:
                // report the dispatch failure with its legacy-identical `Err`
                // result and no history entry.
                let detached = DetachedTx::for_dispatch_failure(&error, signature);
                let hpsvm_error = HPSVMError::from(error);
                let (result, _) = detached_into_result_and_delta(detached, false);
                return Err(StagedFailure::new(hpsvm_error, result));
            }
        };
        if let Some(error) = detached.take_fatal() {
            // Nothing was staged (the readonly path never mutates the
            // overlay), so report the fatal error with its legacy-identical
            // `Err` result and no history entry.
            let (result, _) = detached_into_result_and_delta(detached, false);
            return Err(StagedFailure::new(error, result));
        }

        let checkpoint = svm.accounts.checkpoint();
        let inspector_live = detached.inspector_live;
        let cache_dirty = staging_touches_caches(&detached.post_accounts, &svm.accounts);
        let history_enabled = svm.history.is_enabled();
        let (result, delta) = crate::hotpath_block!(
            "hpsvm::commit::detached_into_result_and_delta",
            detached_into_result_and_delta(detached, history_enabled)
        );
        let (post_accounts, history_entry) = delta.into_parts();
        // Capture the per-address before/after deltas now, while the overlay
        // still holds the pre-staging state in `get_account` reads (post
        // accounts are applied by sync below). Ownership moves into the
        // staged transaction so commit_with streams without re-reading state.
        let changes: Vec<crate::state_change_sink::AccountChangeRecord> = post_accounts
            .iter()
            .map(|(address, after)| crate::state_change_sink::AccountChangeRecord {
                address: *address,
                before: svm.accounts.get_account(address),
                // A zero-lamport post-state is removed from the overlay by
                // `add_account`, so report it as "removed" (None) to match the
                // committed after-state rather than the intermediate zero entry.
                after: if after.lamports() == 0 { None } else { Some(after.clone()) },
            })
            .collect();
        if let Err(error) = crate::hotpath_block!(
            "hpsvm::commit::sync_staged_accounts",
            svm.accounts.sync_accounts(post_accounts)
        ) {
            // Practically unreachable: execution-produced accounts always pass
            // sync validation. Roll back rather than commit partial state.
            // (The legacy path panicked here via `expect`.)
            svm.accounts.rollback(checkpoint);
            return Err(StagedFailure::new(error, result));
        }
        Ok(ExecutedTx::new(
            svm,
            checkpoint,
            result,
            history_entry,
            cache_dirty,
            changes,
            inspector_live,
        ))
    }

    /// Runs the readonly execution for one transaction, honouring the current
    /// sigverify setting. Never mutates the overlay.
    ///
    /// This is the single `InspectMode` selection point for every execution
    /// path (`transact_core`, `try_transact`, `transact_detached`, and through
    /// them `send_transaction`, `simulate_*`, and the batch stages): one
    /// `inspector_live()` mutex lock decides which monomorphization runs, and
    /// everything below it is hook-branch-free when no inspector is live.
    pub(crate) fn execute_readonly(
        &self,
        tx: VersionedTransaction,
    ) -> (ExecutionResult, Rc<RefCell<LogCollector>>) {
        let log_collector = Rc::new(RefCell::new(LogCollector {
            bytes_limit: self.version.limits.log_bytes_limit,
            ..Default::default()
        }));
        if self.inspector_live() {
            self.execute_readonly_impl::<{ InspectMode::DynInspector }>(tx, log_collector)
        } else {
            self.execute_readonly_impl::<{ InspectMode::NoInspector }>(tx, log_collector)
        }
    }

    fn execute_readonly_impl<const MODE: InspectMode>(
        &self,
        tx: VersionedTransaction,
        log_collector: Rc<RefCell<LogCollector>>,
    ) -> (ExecutionResult, Rc<RefCell<LogCollector>>) {
        let execution = if self.version.sigverify {
            self.execute_transaction_readonly::<MODE>(tx, log_collector.clone())
        } else {
            self.execute_transaction_no_verify_readonly::<MODE>(tx, log_collector.clone())
        };
        (execution, log_collector)
    }

    /// Plans a conflict-aware transaction batch without committing any state.
    ///
    /// The returned stages contain indexes into the original input order. Stages
    /// are built greedily from account read/write conflicts and form the basis
    /// for higher-level batch schedulers.
    pub fn plan_transaction_batch<V>(
        &self,
        txs: impl IntoIterator<Item = V>,
    ) -> Result<TransactionBatchPlan, TransactionBatchError>
    where
        V: Into<VersionedTransaction>,
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
    pub fn send_transaction_batch<V>(
        &mut self,
        txs: impl IntoIterator<Item = V>,
    ) -> Result<TransactionBatchExecutionResult, TransactionBatchError>
    where
        V: Into<VersionedTransaction>,
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
        let detached = self.transact_detached(tx);
        if let Err(tx_err) = detached.status {
            Err(FailedTransactionMetadata { err: tx_err, meta: detached.meta })
        } else {
            Ok(SimulatedTransactionInfo {
                meta: detached.meta,
                post_accounts: detached.post_accounts,
            })
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
            self.version.fee_structure.lamports_per_signature,
        )]));
    }

    /// Warps the clock to the specified slot.
    pub fn warp_to_slot(&mut self, slot: u64) {
        let mut clock = self.get_sysvar::<Clock>();
        clock.slot = slot;
        self.set_sysvar_internal(&clock);
    }

    /// Gets the current compute budget.
    pub const fn get_compute_budget(&self) -> Option<ComputeBudget> {
        self.version.compute_budget
    }

    /// Returns whether signature verification is enabled.
    pub const fn get_sigverify(&self) -> bool {
        self.version.sigverify
    }

    #[cfg(feature = "internal-test")]
    pub fn get_feature_set(&self) -> Arc<FeatureSet> {
        Arc::clone(&self.version.feature_set)
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
    pub fn set_invocation_inspect_callback<C: InvocationInspectCallback<T> + 'static>(
        &mut self,
        callback: C,
    ) {
        self.invocation_inspect_callback = Arc::new(callback);
    }

    /// **Advanced reconfiguration.** Registers a custom syscall in both program runtime
    /// environments (v1 and v2).
    ///
    /// This can be called on a freshly constructed [`Hpsvm::new()`] instance or on an existing
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

        Ok(())
    }
}

impl<T: SvmTypesHost> Svm<'_, T> {
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

fn execution_into_detached<T: SvmTypesHost>(
    vm: &Svm<'_, T>,
    execution: ExecutionResult,
    log_collector: Rc<RefCell<LogCollector>>,
    method_name: &str,
) -> DetachedTx {
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
        fatal_error,
        inspector_live,
    } = execution;
    let Ok(logs) = Rc::try_unwrap(log_collector).map(|collector| collector.into_inner().messages)
    else {
        unreachable!("Log collector should not be used after {method_name} returns")
    };

    DetachedTx {
        meta: TransactionMetadata {
            signature,
            logs,
            inner_instructions,
            compute_units_consumed,
            return_data,
            fee,
            diagnostics: if vm.version.compute_diagnostics {
                hotpath_block!("hpsvm::execution_into_detached::diagnostics", {
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
        fee_payer,
        fatal_error,
        inspector_live,
    }
}

fn execution_diagnostics<T: SvmTypesHost>(
    vm: &Svm<'_, T>,
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
        let svm = Hpsvm::new();
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
        let mut svm = Hpsvm::new();

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
        let mut svm = Hpsvm::new();
        let payer = Keypair::new();
        let recipient = Address::new_unique();
        let original_feature_set = Arc::clone(&svm.version.feature_set);
        let original_reserved_account_keys = svm.reserved_account_keys.clone();

        svm.airdrop(&payer.pubkey(), 10_000).unwrap();

        let poisoned_program = Address::new_unique();
        let mut invalid_program = AccountSharedData::new(1, 0, &system_program::id());
        invalid_program.set_executable(true);
        svm.accounts.add_account_no_checks(poisoned_program, invalid_program);

        let err = svm
            .set_feature_set(FeatureSet::all_enabled())
            .expect_err("invalid cached program should abort feature-set reconfiguration");

        assert!(matches!(err, HPSVMError::Instruction(InstructionError::IncorrectProgramId)));
        assert_eq!(svm.version.feature_set, original_feature_set);
        assert_eq!(svm.reserved_account_keys.active, original_reserved_account_keys.active);

        // The staged execution API borrows the VM, so the transact necessarily
        // happens after the failed reconfiguration rolled accounts back.
        let tx = Transaction::new(
            &[&payer],
            Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
            svm.latest_blockhash(),
        );
        let result = svm.transact(tx).expect("transact should stage").commit();

        assert!(result.is_ok());
        assert_eq!(svm.get_balance(&recipient), Some(64));
    }
}
