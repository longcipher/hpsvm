//! Runtime-environment construction.
//!
//! `Hpsvm::new()` (and the `with_program_test_defaults` builder path)
//! repeatedly rebuilds an identical default runtime environment and
//! re-verifies the same built-in / SPL ELF executables on every
//! construction. For test suites that spin up many VMs this dominates
//! wall-clock time (>80% of `core_interfaces`).
//!
//! When the configuration matches the exact default (`FeatureSet::all_enabled`,
//! default compute budget, no register tracing, no custom syscalls) the
//! runtime environment is immutable and identical across VMs, so a single
//! `Arc<ProgramRuntimeEnvironments>` is shared process-wide.
//!
//! Verified executables are memoized in the process-wide
//! [`ProgramEntryStore`](crate::programs::entry_store) (slice-pointer fast
//! path for `include_bytes!` statics plus the content-keyed resident store),
//! letting every VM after the first skip ELF parsing, verification, and JIT
//! compilation for already-seen programs.
//!
//! Safety: the shared environment is never mutated after creation. Custom
//! syscall registration and feature-set changes rebuild a fresh environment
//! and reload programs from account data, leaving the shared cache untouched.
//! The executable caches are keyed by `Arc` pointer identity of the
//! environment, so only VMs sharing the exact same environment can observe a
//! cached entry. The ELF slice pointer in the fast-path key is stable because
//! the only `CACHED` producer is `add_program_preverified`, invoked
//! exclusively with `include_bytes!` statics.

use std::sync::{Arc, OnceLock};

use agave_feature_set::FeatureSet;
use solana_address::Address;
use solana_compute_budget::compute_budget::ComputeBudget;
use solana_program_runtime::{
    invoke_context::InvokeContext,
    loaded_programs::{ProgramRuntimeEnvironment, ProgramRuntimeEnvironments},
    program_cache_entry::ProgramCacheEntry,
    program_metrics::LoadProgramMetrics,
    solana_sbpf::program::BuiltinProgram,
};
use solana_syscalls::create_program_runtime_environment;

use crate::{FeatureGate, HPSVMError, Svm, SvmTypesHost, TxFeatureFlags};

/// Shared default runtime environment, built once per process.
pub(crate) static DEFAULT_RUNTIME_ENV: OnceLock<Arc<ProgramRuntimeEnvironments>> = OnceLock::new();

/// Returns the shared default environment pointer iff `envs` *is* the shared
/// default environment (pointer identity), so only VMs running under it can
/// reuse memoized executables.
pub(crate) fn shared_default_env_ptr(envs: &Arc<ProgramRuntimeEnvironments>) -> Option<usize> {
    let shared = DEFAULT_RUNTIME_ENV.get()?;
    Arc::ptr_eq(envs, shared).then_some(Arc::as_ptr(shared) as usize)
}

impl<T: SvmTypesHost> Svm<'_, T> {
    pub(crate) fn try_refresh_runtime_environments(&mut self) -> Result<(), HPSVMError> {
        #[cfg(feature = "register-tracing")]
        let enable_register_tracing = self.extensions.enable_register_tracing;
        #[cfg(not(feature = "register-tracing"))]
        let enable_register_tracing = false;

        // The exact default configuration produces a runtime environment that
        // is identical across VMs. Reuse the process-wide shared `Arc` so the
        // first default VM pays the build cost once and every subsequent
        // `Hpsvm::new()` skips `create_program_runtime_environment` entirely,
        // and so default-program executables can be memoized (see
        // `resolve_program_entry`).
        let is_default_config = !enable_register_tracing &&
            self.version.compute_budget.is_none() &&
            self.extensions.runtime_registry.custom_syscalls().is_empty() &&
            self.version.feature_set.as_ref() == &FeatureSet::all_enabled();

        if is_default_config {
            let shared = DEFAULT_RUNTIME_ENV.get();
            if let Some(env) = shared {
                self.state.accounts.set_runtime_environments_arc(Arc::clone(env));
                return Ok(());
            }
        }

        // One flag load per environment refresh; the refresh runs at setup or
        // reconfiguration time, never per transaction.
        let flags = TxFeatureFlags::load(&self.version.feature_set);
        let compute_budget = self.version.compute_budget.unwrap_or_else(|| {
            ComputeBudget::new_with_defaults(flags.is_enabled(FeatureGate::RaiseCpiNestingLimitTo8))
        });
        let program_runtime = create_program_runtime_environment(
            &self.version.feature_set.runtime_features(),
            &compute_budget.to_budget(),
            false,
            enable_register_tracing,
        )
        .map_err(|error| HPSVMError::RuntimeEnvironment {
            version: "v1",
            reason: error.to_string(),
        })?;

        let mut current_runtime = program_runtime;
        for syscall in self.extensions.runtime_registry.custom_syscalls() {
            // ponytail: BuiltinProgram is not Clone, reconstruct like mollusk does
            let config = current_runtime.get_config().clone();
            let mut loader: BuiltinProgram<InvokeContext<'static, 'static>> =
                BuiltinProgram::new_loader(config);
            for (_key, (name, value)) in current_runtime.get_function_registry().iter() {
                let name = std::str::from_utf8(name).expect("syscall names are valid UTF-8");
                loader.register_function(name, value).map_err(|error| {
                    HPSVMError::CustomSyscallRegistration {
                        name: name.to_owned(),
                        runtime: "runtime",
                        reason: error.to_string(),
                    }
                })?;
            }
            (syscall.function)(&mut loader, &syscall.name).map_err(|error| {
                HPSVMError::CustomSyscallRegistration {
                    name: syscall.name.clone(),
                    runtime: "runtime",
                    reason: error.to_string(),
                }
            })?;
            current_runtime = ProgramRuntimeEnvironment::from(loader);
        }
        let program_runtime = current_runtime;

        let env =
            Arc::new(ProgramRuntimeEnvironments::new(program_runtime.clone(), program_runtime));
        if is_default_config {
            // First default VM: publish so later VMs reuse it. A concurrent
            // builder may have raced; either arc is equivalent, so ignore.
            let _ = DEFAULT_RUNTIME_ENV.set(Arc::clone(&env));
        }
        self.state.accounts.set_runtime_environments_arc(env);

        Ok(())
    }

    pub(crate) fn refresh_runtime_environments(&mut self) {
        self.try_refresh_runtime_environments()
            .expect("runtime environment refresh should never fail for internal configuration");
    }

    /// Resolve the [`ProgramCacheEntry`] for a program being loaded.
    ///
    /// Lookup order:
    /// 1. Slice-pointer fast path when eligible (`CACHED`, slot 0, shared default environment) —
    ///    stable for `include_bytes!` statics.
    /// 2. Content-keyed resident store keyed by `(env, ELF hash, loader, slot bucket)` — safe for
    ///    every environment because the key includes the environment `Arc` pointer identity.
    /// 3. Fresh load: [`load_program_uncompiled`](crate::programs::load_program_uncompiled) under
    ///    `Lazy`/`Off` on x86_64 (interpreter-first tier), otherwise agave's
    ///    `ProgramCacheEntry::load` (today's eager path).
    ///
    /// On a hit the expensive ELF parse + verify + JIT is skipped entirely.
    /// Misses populate the resident store (and the fast-path cache when
    /// eligible) so the next VM reuses the entry.
    pub(crate) fn resolve_program_entry<const CACHED: bool>(
        &self,
        loader_id: &Address,
        env: &ProgramRuntimeEnvironment,
        current_slot: u64,
        program_bytes: &[u8],
    ) -> Result<Arc<ProgramCacheEntry>, HPSVMError> {
        use crate::{
            accounts_db::IMMEDIATE_DEPLOYMENT_SLOT,
            programs::{content_key, default_program_cache, program_entry_store},
        };

        let envs_arc = self.state.accounts.runtime_environments_arc();
        let env_ptr = Arc::as_ptr(envs_arc) as usize;

        // Fast path: stable slice identity for default statics.
        let fast_key = if CACHED && current_slot == 0 {
            shared_default_env_ptr(envs_arc)
                .map(|ptr| (ptr, program_bytes.as_ptr() as usize, program_bytes.len(), *loader_id))
        } else {
            None
        };
        if let Some(key) = fast_key &&
            let Some(entry) = default_program_cache().read().get(&key).cloned()
        {
            return Ok(entry);
        }

        // Resident content store: safe for every environment.
        let store_key = content_key(env_ptr, program_bytes, loader_id, IMMEDIATE_DEPLOYMENT_SLOT);
        if let Some(entry) = program_entry_store().get(&store_key) {
            // Backfill the fast path so default VMs keep hitting it first.
            // Return the fast-path winner to preserve pointer identity under
            // races (both caches hold the same logical entry).
            if let Some(key) = fast_key {
                let winner = default_program_cache()
                    .write()
                    .entry(key)
                    .or_insert_with(|| Arc::clone(&entry))
                    .clone();
                return Ok(winner);
            }
            return Ok(entry);
        }

        let loaded =
            if self.version.jit_policy.requires_uncompiled_load() && cfg!(target_arch = "x86_64") {
                crate::programs::load_program_uncompiled(loader_id, env, program_bytes)
                    .map_err(HPSVMError::from)?
            } else {
                ProgramCacheEntry::load(
                    loader_id,
                    env.clone(),
                    crate::accounts_db::IMMEDIATE_DEPLOYMENT_SLOT,
                    program_bytes,
                    &mut LoadProgramMetrics::default(),
                )
                .map_err(HPSVMError::from)?
            };
        let arc = Arc::new(loaded);

        // First-writer wins: return the resident entry so concurrent loaders
        // of the same bytes share one `Arc` (pointer identity).
        let resident = program_entry_store().insert(store_key, Arc::clone(&arc));
        if let Some(key) = fast_key {
            // Fast path mirrors the store winner, not our own load.
            let winner = Arc::clone(&resident);
            let cached = default_program_cache().write().entry(key).or_insert(winner).clone();
            return Ok(cached);
        }

        Ok(resident)
    }
}
