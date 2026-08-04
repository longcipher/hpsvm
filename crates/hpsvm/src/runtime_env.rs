//! Runtime-environment construction and the process-wide default-program
//! executable cache.
//!
//! `HPSVM::new()` (and the `with_program_test_defaults` builder path)
//! repeatedly rebuilds an identical default runtime environment and
//! re-verifies the same built-in / SPL ELF executables on every
//! construction. For test suites that spin up many VMs this dominates
//! wall-clock time (>80% of `core_interfaces`).
//!
//! When the configuration matches the exact default (`FeatureSet::all_enabled`,
//! default compute budget, no register tracing, no custom syscalls) the
//! runtime environment is immutable and identical across VMs, so a single
//! `Arc<ProgramRuntimeEnvironments>` is shared process-wide. Verified default
//! executables (`Arc<ProgramCacheEntry>`) are then memoized keyed by the
//! shared environment pointer plus the ELF slice identity, letting every VM
//! after the first skip ELF parsing, verification, and JIT compilation for
//! the default programs.
//!
//! Safety: the shared environment is never mutated after creation. Custom
//! syscall registration and feature-set changes rebuild a fresh environment
//! and reload programs from account data, leaving the shared cache untouched.
//! The executable cache is keyed by `Arc` pointer identity of the
//! environment, so only VMs sharing the exact same environment can observe a
//! cached entry. The ELF slice pointer in the key is stable because the only
//! producer is `add_program_preverified`, invoked exclusively with
//! `include_bytes!` statics.

use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use agave_feature_set::{FeatureSet, raise_cpi_nesting_limit_to_8};
use parking_lot::RwLock;
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

use crate::{HPSVM, HPSVMError};

/// Shared default runtime environment, built once per process.
pub(crate) static DEFAULT_RUNTIME_ENV: OnceLock<Arc<ProgramRuntimeEnvironments>> = OnceLock::new();

/// Cache key for a memoized default-program executable:
/// `(env_ptr, elf_ptr, elf_len, loader_id)`.
type DefaultProgramKey = (usize, usize, usize, Address);

/// Memoized verified executables for default programs.
pub(crate) static DEFAULT_PROGRAM_CACHE: OnceLock<
    RwLock<HashMap<DefaultProgramKey, Arc<ProgramCacheEntry>>>,
> = OnceLock::new();

pub(crate) fn default_program_cache()
-> &'static RwLock<HashMap<DefaultProgramKey, Arc<ProgramCacheEntry>>> {
    DEFAULT_PROGRAM_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Returns the shared default environment pointer iff `envs` *is* the shared
/// default environment (pointer identity), so only VMs running under it can
/// reuse memoized executables.
fn shared_default_env_ptr(envs: &Arc<ProgramRuntimeEnvironments>) -> Option<usize> {
    let shared = DEFAULT_RUNTIME_ENV.get()?;
    Arc::ptr_eq(envs, shared).then_some(Arc::as_ptr(shared) as usize)
}

impl HPSVM {
    pub(crate) fn try_refresh_runtime_environments(&mut self) -> Result<(), HPSVMError> {
        #[cfg(feature = "register-tracing")]
        let enable_register_tracing = self.enable_register_tracing;
        #[cfg(not(feature = "register-tracing"))]
        let enable_register_tracing = false;

        // The exact default configuration produces a runtime environment that
        // is identical across VMs. Reuse the process-wide shared `Arc` so the
        // first default VM pays the build cost once and every subsequent
        // `HPSVM::new()` skips `create_program_runtime_environment` entirely,
        // and so default-program executables can be memoized (see
        // `resolve_program_entry`).
        let is_default_config = !enable_register_tracing &&
            self.runtime_env.compute_budget.is_none() &&
            self.runtime_registry.custom_syscalls().is_empty() &&
            self.cfg.feature_set == FeatureSet::all_enabled();

        if is_default_config {
            let shared = DEFAULT_RUNTIME_ENV.get();
            if let Some(env) = shared {
                self.accounts.set_runtime_environments_arc(Arc::clone(env));
                return Ok(());
            }
        }

        let compute_budget = self.runtime_env.compute_budget.unwrap_or_else(|| {
            ComputeBudget::new_with_defaults(
                self.cfg.feature_set.is_active(&raise_cpi_nesting_limit_to_8::ID),
            )
        });
        let program_runtime = create_program_runtime_environment(
            &self.cfg.feature_set.runtime_features(),
            &compute_budget.to_budget(),
            false,
            enable_register_tracing,
        )
        .map_err(|error| HPSVMError::RuntimeEnvironment {
            version: "v1",
            reason: error.to_string(),
        })?;

        let mut current_runtime = program_runtime;
        for syscall in self.runtime_registry.custom_syscalls() {
            // ponytail: BuiltinProgram is not Clone, reconstruct like mollusk does
            let config = current_runtime.get_config().clone();
            let mut loader: BuiltinProgram<InvokeContext<'static, 'static>> =
                BuiltinProgram::new_loader(config);
            for (_key, (name, value)) in current_runtime.get_function_registry().iter() {
                let name = std::str::from_utf8(name).unwrap();
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
        self.accounts.set_runtime_environments_arc(env);

        Ok(())
    }

    pub(crate) fn refresh_runtime_environments(&mut self) {
        self.try_refresh_runtime_environments()
            .expect("runtime environment refresh should never fail for internal configuration");
    }

    /// Resolve the [`ProgramCacheEntry`] for a program being loaded, consulting
    /// the process-wide default-program cache when eligible.
    ///
    /// Cache eligibility requires all of:
    /// - the caller opted in via `CACHED` (only [`HPSVM::add_program_preverified`], invoked
    ///   exclusively with `include_bytes!` statics);
    /// - the VM is running under the shared default runtime environment (pointer identity), so the
    ///   memoized executable is valid here;
    /// - loading at slot 0, so the entry's baked-in slot fields match.
    ///
    /// On a cache hit the expensive `ProgramCacheEntry::new` (ELF parse +
    /// verify + JIT) is skipped entirely. Misses populate the cache so the next
    /// default VM reuses the entry.
    pub(crate) fn resolve_program_entry<const CACHED: bool>(
        &self,
        loader_id: &Address,
        env: &ProgramRuntimeEnvironment,
        current_slot: u64,
        program_bytes: &[u8],
        program_size: usize,
    ) -> Result<Arc<ProgramCacheEntry>, HPSVMError> {
        let cache_key = if CACHED && current_slot == 0 {
            shared_default_env_ptr(self.accounts.runtime_environments_arc()).map(|env_ptr| {
                (env_ptr, program_bytes.as_ptr() as usize, program_bytes.len(), *loader_id)
            })
        } else {
            None
        };

        if let Some(key) = cache_key {
            let cached = default_program_cache().read().get(&key).cloned();
            if let Some(entry) = cached {
                return Ok(entry);
            }
        }

        let mut loaded_program = ProgramCacheEntry::new(
            loader_id,
            env.clone(),
            current_slot,
            current_slot,
            program_bytes,
            program_size,
            &mut LoadProgramMetrics::default(),
        )
        .map_err(HPSVMError::from)?;
        loaded_program.effective_slot = current_slot;
        let arc = Arc::new(loaded_program);

        if let Some(key) = cache_key {
            default_program_cache().write().entry(key).or_insert_with(|| Arc::clone(&arc));
        }

        Ok(arc)
    }
}
