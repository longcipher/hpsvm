use std::{marker::PhantomData, sync::Arc};

use agave_feature_set::FeatureSet;
use solana_compute_budget::compute_budget::ComputeBudget;
use solana_native_token::LAMPORTS_PER_SOL;
use solana_program_runtime::invoke_context::BuiltinFunctionRegisterer;

use crate::{
    AccountSource, BaseSvmTypes, CustomSyscallRegistration, Hpsvm, Inspector, Svm,
    error::HPSVMError, precompile_registry::PrecompileProvider,
};

mod private {
    pub trait Sealed {}
}

/// Typestate marker used while the builder still allows changing the feature set.
#[doc(hidden)]
pub struct FeatureConfigOpen;

/// Typestate marker used once feature-dependent state has been selected.
#[doc(hidden)]
pub struct FeatureConfigSealed;

impl private::Sealed for FeatureConfigOpen {}
impl private::Sealed for FeatureConfigSealed {}

/// Internal marker trait for the builder typestate.
pub trait FeatureConfigState: private::Sealed {}

impl FeatureConfigState for FeatureConfigOpen {}
impl FeatureConfigState for FeatureConfigSealed {}

enum LogBytesLimitPlan {
    Inherit,
    Explicit(Option<usize>),
}

struct BuildPlan {
    feature_set: Option<FeatureSet>,
    compute_budget: Option<ComputeBudget>,
    sigverify: Option<bool>,
    blockhash_check: Option<bool>,
    compute_diagnostics: Option<bool>,
    lamports: Option<u64>,
    include_sysvars: bool,
    include_feature_accounts: bool,
    include_builtins: bool,
    include_spl_programs: bool,
    include_default_programs: bool,
    #[cfg(feature = "precompiles")]
    include_precompiles: bool,
    precompile_provider: Option<Arc<dyn PrecompileProvider<BaseSvmTypes>>>,
    transaction_history: Option<usize>,
    account_source: Option<Arc<dyn AccountSource>>,
    log_bytes_limit: LogBytesLimitPlan,
    inspector: Option<Box<dyn Inspector<BaseSvmTypes>>>,
    custom_syscalls: Vec<CustomSyscallRegistration>,
    enable_register_tracing: bool,
}

impl BuildPlan {
    fn new() -> Self {
        Self {
            feature_set: None,
            compute_budget: None,
            sigverify: None,
            blockhash_check: None,
            compute_diagnostics: None,
            lamports: None,
            include_sysvars: false,
            include_feature_accounts: false,
            include_builtins: false,
            include_spl_programs: false,
            include_default_programs: false,
            #[cfg(feature = "precompiles")]
            include_precompiles: false,
            precompile_provider: None,
            transaction_history: None,
            account_source: None,
            log_bytes_limit: LogBytesLimitPlan::Inherit,
            inspector: None,
            custom_syscalls: Vec::new(),
            enable_register_tracing: Hpsvm::default_register_tracing_enabled(),
        }
    }

    fn apply_program_test_defaults(&mut self) {
        self.lamports.get_or_insert(1_000_000u64.wrapping_mul(LAMPORTS_PER_SOL));
        self.include_sysvars = true;
        self.include_feature_accounts = true;
        self.include_builtins = true;
        self.include_default_programs = true;
        #[cfg(feature = "precompiles")]
        {
            self.include_precompiles = true;
        }
        self.sigverify.get_or_insert(true);
        self.blockhash_check.get_or_insert(true);
    }
}

/// Typed builder for [`Hpsvm`].
///
/// The builder deliberately keeps feature selection open until the first
/// feature-dependent surface is requested. Once the build plan starts
/// materializing builtins, feature accounts, default programs, or precompiles,
/// the builder moves into [`FeatureConfigSealed`]. That removes
/// [`HpsvmBuilder::with_feature_set`] from the API surface and turns a runtime
/// ordering concern into a compile-time guarantee.
///
/// ```compile_fail
/// use agave_feature_set::FeatureSet;
/// use hpsvm::Hpsvm;
///
/// let _ = Hpsvm::builder()
///     .with_default_programs()
///     .with_feature_set(FeatureSet::default());
/// ```
///
/// ```compile_fail
/// use hpsvm::Hpsvm;
///
/// let _ = Hpsvm::new().with_sigverify(false);
/// ```
#[must_use = "builders do nothing unless you call build()"]
pub struct HpsvmBuilder<State = FeatureConfigOpen> {
    plan: BuildPlan,
    state: PhantomData<State>,
}

impl Default for HpsvmBuilder<FeatureConfigOpen> {
    fn default() -> Self {
        Self::new()
    }
}

impl HpsvmBuilder<FeatureConfigOpen> {
    /// Start a new builder in the feature-configurable state.
    pub fn new() -> Self {
        Self { plan: BuildPlan::new(), state: PhantomData }
    }

    /// Select the feature set before any feature-dependent state gets materialized.
    pub fn with_feature_set(mut self, feature_set: FeatureSet) -> Self {
        self.plan.feature_set = Some(feature_set);
        self
    }

    /// Queue feature accounts and seal the feature selection window.
    pub fn with_feature_accounts(mut self) -> HpsvmBuilder<FeatureConfigSealed> {
        self.plan.include_feature_accounts = true;
        self.seal()
    }

    /// Queue builtin programs and seal the feature selection window.
    pub fn with_builtins(mut self) -> HpsvmBuilder<FeatureConfigSealed> {
        self.plan.include_builtins = true;
        self.seal()
    }

    /// Queue the standard default programs and seal the feature selection window.
    pub fn with_default_programs(mut self) -> HpsvmBuilder<FeatureConfigSealed> {
        self.plan.include_default_programs = true;
        self.seal()
    }

    /// Queue only the SPL Token, Token-2022, and Associated Token programs.
    pub fn with_spl_programs(mut self) -> HpsvmBuilder<FeatureConfigSealed> {
        self.plan.include_spl_programs = true;
        self.seal()
    }

    /// Queue the standard precompiles and seal the feature selection window.
    #[cfg(feature = "precompiles")]
    pub fn with_precompiles(mut self) -> HpsvmBuilder<FeatureConfigSealed> {
        self.plan.include_precompiles = true;
        self.seal()
    }

    /// Install a custom precompile provider and seal the feature selection window.
    ///
    /// Replaces the default native dispatch set; account installation for the
    /// provider's ids happens at build time against the selected feature set.
    pub fn with_precompile_provider(
        mut self,
        provider: Arc<dyn PrecompileProvider<BaseSvmTypes>>,
    ) -> HpsvmBuilder<FeatureConfigSealed> {
        self.plan.precompile_provider = Some(provider);
        self.seal()
    }

    /// Queue the same materialized runtime surfaces used by [`Hpsvm::new()`].
    ///
    /// If the caller has not chosen a feature set yet, this helper opts into the
    /// fully-enabled feature set so the resulting VM matches the legacy test
    /// defaults. When a feature set was already chosen explicitly, it is kept.
    pub fn with_program_test_defaults(mut self) -> HpsvmBuilder<FeatureConfigSealed> {
        self.plan.feature_set.get_or_insert_with(FeatureSet::all_enabled);
        self.plan.apply_program_test_defaults();
        self.seal()
    }

    fn seal(self) -> HpsvmBuilder<FeatureConfigSealed> {
        HpsvmBuilder { plan: self.plan, state: PhantomData }
    }
}

impl HpsvmBuilder<FeatureConfigSealed> {
    /// Queue feature accounts after the feature set has been locked in.
    pub fn with_feature_accounts(mut self) -> Self {
        self.plan.include_feature_accounts = true;
        self
    }

    /// Queue builtin programs after the feature set has been locked in.
    pub fn with_builtins(mut self) -> Self {
        self.plan.include_builtins = true;
        self
    }

    /// Queue the standard default programs after the feature set has been locked in.
    pub fn with_default_programs(mut self) -> Self {
        self.plan.include_default_programs = true;
        self
    }

    /// Queue only the SPL Token, Token-2022, and Associated Token programs.
    pub fn with_spl_programs(mut self) -> Self {
        self.plan.include_spl_programs = true;
        self
    }

    /// Queue the standard precompiles after the feature set has been locked in.
    #[cfg(feature = "precompiles")]
    pub fn with_precompiles(mut self) -> Self {
        self.plan.include_precompiles = true;
        self
    }

    /// Install a custom precompile provider after the feature set has been locked in.
    ///
    /// Replaces the default native dispatch set; account installation for the
    /// provider's ids happens at build time against the selected feature set.
    pub fn with_precompile_provider(
        mut self,
        provider: Arc<dyn PrecompileProvider<BaseSvmTypes>>,
    ) -> Self {
        self.plan.precompile_provider = Some(provider);
        self
    }

    /// Fill in the standard program-test runtime surfaces without reopening feature selection.
    pub fn with_program_test_defaults(mut self) -> Self {
        self.plan.apply_program_test_defaults();
        self
    }
}

impl<State: FeatureConfigState> HpsvmBuilder<State> {
    /// Install an execution inspector before the VM is built.
    pub fn with_inspector<I: Inspector<BaseSvmTypes> + 'static>(mut self, inspector: I) -> Self {
        self.plan.inspector = Some(Box::new(inspector));
        self
    }

    /// Set the compute budget that will be baked into the runtime environments.
    pub fn with_compute_budget(mut self, compute_budget: ComputeBudget) -> Self {
        self.plan.compute_budget = Some(compute_budget);
        self
    }

    /// Enable or disable signature verification.
    pub fn with_sigverify(mut self, sigverify: bool) -> Self {
        self.plan.sigverify = Some(sigverify);
        self
    }

    /// Enable or disable blockhash checking.
    pub fn with_blockhash_check(mut self, blockhash_check: bool) -> Self {
        self.plan.blockhash_check = Some(blockhash_check);
        self
    }

    /// Enable or disable execution diagnostics (pre/post diffs, token balances).
    ///
    /// Defaults to `true`. When disabled, transaction outcomes return an empty
    /// [`ExecutionDiagnostics`](crate::types::ExecutionDiagnostics), skipping
    /// expensive balance and diff computation.
    pub fn with_compute_diagnostics(mut self, enabled: bool) -> Self {
        self.plan.compute_diagnostics = Some(enabled);
        self
    }

    /// Change the initial lamports in the airdrop account.
    pub fn with_lamports(mut self, lamports: u64) -> Self {
        self.plan.lamports = Some(lamports);
        self
    }

    /// Include the default sysvars.
    pub fn with_sysvars(mut self) -> Self {
        self.plan.include_sysvars = true;
        self
    }

    /// Change the transaction history capacity.
    pub fn with_transaction_history(mut self, capacity: usize) -> Self {
        self.plan.transaction_history = Some(capacity);
        self
    }

    /// Install a read-through account source used when local state misses an account.
    pub fn with_account_source(mut self, source: impl AccountSource + 'static) -> Self {
        self.plan.account_source = Some(Arc::new(source));
        self
    }

    /// Override the log byte limit. Use `None` to disable truncation entirely.
    pub fn with_log_bytes_limit(mut self, limit: Option<usize>) -> Self {
        self.plan.log_bytes_limit = LogBytesLimitPlan::Explicit(limit);
        self
    }

    /// Configure register tracing before any programs get materialized.
    pub fn with_register_tracing(mut self, enable_register_tracing: bool) -> Self {
        self.plan.enable_register_tracing = enable_register_tracing;
        self
    }

    /// Queue a custom syscall for both runtime environments.
    ///
    /// The builder stores the registration and applies it once, during `build()`,
    /// so callers do not have to reason about whether builtins or cached programs
    /// have already been loaded.
    pub fn with_custom_syscall(mut self, name: &str, syscall: BuiltinFunctionRegisterer) -> Self {
        self.plan
            .custom_syscalls
            .push(CustomSyscallRegistration { name: name.to_owned(), function: syscall });
        self
    }

    /// Materialize the build plan into a runnable [`Hpsvm`].
    pub fn build(self) -> Result<Hpsvm, HPSVMError> {
        let BuildPlan {
            feature_set,
            compute_budget,
            sigverify,
            blockhash_check,
            compute_diagnostics,
            lamports,
            include_sysvars,
            include_feature_accounts,
            include_builtins,
            include_spl_programs,
            include_default_programs,
            #[cfg(feature = "precompiles")]
            include_precompiles,
            precompile_provider,
            transaction_history,
            account_source,
            log_bytes_limit,
            inspector,
            custom_syscalls,
            enable_register_tracing,
        } = self.plan;

        #[cfg(feature = "precompiles")]
        let needs_precompiles = include_precompiles;
        #[cfg(not(feature = "precompiles"))]
        let needs_precompiles = false;

        let needs_sysvars = include_builtins ||
            include_default_programs ||
            include_spl_programs ||
            needs_precompiles ||
            sigverify == Some(true) ||
            blockhash_check == Some(true);
        if needs_sysvars && !include_sysvars {
            return Err(HPSVMError::MissingRuntimeComponent { component: "sysvars" });
        }

        let mut svm = Svm::<BaseSvmTypes>::new_inner(enable_register_tracing);

        if let Some(feature_set) = feature_set {
            svm.set_feature_set(feature_set)?;
        }
        if let Some(compute_budget) = compute_budget {
            svm.set_compute_budget(compute_budget);
        }
        if let Some(transaction_history) = transaction_history {
            svm.set_transaction_history(transaction_history);
        }
        if let Some(account_source) = account_source {
            svm.accounts.set_account_source(account_source);
        }
        if let LogBytesLimitPlan::Explicit(limit) = log_bytes_limit {
            svm.set_log_bytes_limit(limit);
        }

        let needs_early_runtime_refresh = !custom_syscalls.is_empty() && !include_builtins;
        for registration in custom_syscalls {
            svm.runtime_registry.register_custom_syscall(registration);
        }
        if needs_early_runtime_refresh {
            svm.try_refresh_runtime_environments()?;
            svm.accounts.rebuild_program_cache().map_err(HPSVMError::from)?;
        }

        if include_builtins {
            svm.set_builtins();
        }
        if let Some(lamports) = lamports {
            svm.set_lamports(lamports);
        }
        if include_sysvars {
            svm.set_sysvars();
        }
        if include_feature_accounts {
            svm.set_feature_accounts();
        }
        if include_default_programs {
            svm.set_default_programs();
        } else if include_spl_programs {
            svm.set_spl_programs();
        }
        #[cfg(feature = "precompiles")]
        if include_precompiles {
            svm.set_precompiles();
        }
        if let Some(provider) = precompile_provider {
            svm.set_precompile_provider(provider);
        }
        if let Some(sigverify) = sigverify {
            svm.set_sigverify(sigverify);
        }
        if let Some(blockhash_check) = blockhash_check {
            svm.set_blockhash_check(blockhash_check);
        }
        if let Some(compute_diagnostics) = compute_diagnostics {
            svm.set_compute_diagnostics(compute_diagnostics);
        }

        if let Some(inspector) = inspector {
            svm.set_boxed_inspector(inspector);
        }

        Ok(svm)
    }
}
