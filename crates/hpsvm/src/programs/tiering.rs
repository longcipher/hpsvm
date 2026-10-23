//! Uncompiled load path and tier promoter (evm2 backend thread analogue).
//!
//! * [`load_program_uncompiled`] mirrors agave's `ProgramCacheEntry::load` verbatim minus the
//!   `jit_compile` call, yielding an interpreter-first entry that is structurally identical modulo
//!   the compiled program.
//! * [`TierPromoter`] sweeps agave's per-entry `ProgramStatistics` after each transaction and
//!   promotes hot entries via `Executable::jit_compile` interior mutability — the next invocation
//!   observes the compiled program with zero coordination.
//! * Promotion failures leave the entry interpreted and never fail or alter any transaction (evm2's
//!   `JitFailed` fallback, already built into `ExecutionMode::PreferJit`).

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use solana_address::Address;
use solana_transaction::InstructionError;

use crate::programs::jit_policy::JitPolicy;

/// Loads a verified-but-uncompiled program entry.
///
/// Mirrors `ProgramCacheEntry::load` (ELF parse + `RequisiteVerifier`) without
/// the unconditional `jit_compile` on x86_64, so the entry starts
/// interpreter-first. All fields match agave's construction; only the
/// compiled program is absent.
pub(crate) fn load_program_uncompiled(
    loader_key: &Address,
    env: &solana_program_runtime::loaded_programs::ProgramRuntimeEnvironment,
    elf_bytes: &[u8],
) -> Result<solana_program_runtime::program_cache_entry::ProgramCacheEntry, InstructionError> {
    use solana_program_runtime::{
        program_cache_entry::{ProgramCacheEntry, ProgramCacheEntryOwner, ProgramCacheEntryType},
        solana_sbpf::{elf::Executable, verifier::RequisiteVerifier},
    };

    use crate::accounts_db::IMMEDIATE_DEPLOYMENT_SLOT;

    // `ProgramRuntimeEnvironment` derefs to `Arc<BuiltinProgram>`; deref
    // coercion supplies the `&Arc` that `Arc::clone` expects.
    let loader: Arc<
        solana_program_runtime::solana_sbpf::program::BuiltinProgram<
            solana_program_runtime::invoke_context::InvokeContext<'static, 'static>,
        >,
    > = Arc::clone(env);
    let executable =
        Executable::load(elf_bytes, loader).map_err(|_| InstructionError::InvalidAccountData)?;
    executable.verify::<RequisiteVerifier>().map_err(|_| InstructionError::InvalidAccountData)?;
    let owner = ProgramCacheEntryOwner::try_from(loader_key)
        .map_err(|()| InstructionError::IncorrectProgramId)?;
    Ok(ProgramCacheEntry {
        program: ProgramCacheEntryType::Loaded(executable),
        account_owner: owner,
        deployment_slot: IMMEDIATE_DEPLOYMENT_SLOT,
        stats: Arc::new(solana_program_runtime::program_metrics::ProgramStatistics::default()),
        latest_access_slot: std::sync::atomic::AtomicU64::new(0),
    })
}

/// Whether an entry currently has a compiled program.
///
/// Off x86_64 there is no compiled program by construction.
#[must_use]
pub(crate) fn has_compiled_program(
    entry: &solana_program_runtime::program_cache_entry::ProgramCacheEntry,
) -> bool {
    #[cfg(all(not(target_os = "windows"), target_arch = "x86_64"))]
    {
        use solana_program_runtime::program_cache_entry::ProgramCacheEntryType;
        match &entry.program {
            ProgramCacheEntryType::Loaded(executable) => {
                executable.get_compiled_program().is_some()
            }
            _ => false,
        }
    }
    #[cfg(not(all(not(target_os = "windows"), target_arch = "x86_64")))]
    {
        let _ = entry;
        false
    }
}

/// Promotion outcome counters, all atomics (no locks on the transaction path).
#[derive(Debug, Default)]
pub(crate) struct PromotionStats {
    /// Promotion jobs scheduled (including synchronous threshold-0 compiles).
    pub(crate) attempted: AtomicU64,
    /// Promotions that published a compiled program.
    pub(crate) succeeded: AtomicU64,
    /// Promotions that failed (entry stays interpreted).
    pub(crate) failed: AtomicU64,
    /// Total microseconds spent inside `jit_compile` (all outcomes).
    pub(crate) compile_us: AtomicU64,
    /// Jobs dropped because the pool was saturated.
    pub(crate) dropped: AtomicU64,
}

impl PromotionStats {
    /// Snapshots the counters.
    pub(crate) fn snapshot(&self) -> PromotionStatsSnapshot {
        let ord = Ordering::Relaxed;
        PromotionStatsSnapshot {
            attempted: self.attempted.load(ord),
            succeeded: self.succeeded.load(ord),
            failed: self.failed.load(ord),
            compile_us: self.compile_us.load(ord),
            dropped: self.dropped.load(ord),
        }
    }
}

/// Owned snapshot of [`PromotionStats`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PromotionStatsSnapshot {
    /// Jobs scheduled.
    pub(crate) attempted: u64,
    /// Jobs that published a compiled program.
    pub(crate) succeeded: u64,
    /// Jobs that failed.
    pub(crate) failed: u64,
    /// Total `jit_compile` microseconds.
    pub(crate) compile_us: u64,
    /// Jobs dropped on saturation.
    pub(crate) dropped: u64,
}

/// Background promoter driving the interpreter-first tier.
///
/// Enabled only under `JitPolicy::Lazy` on x86_64. The pool is sized
/// `clamp(cpus/2, 1, 4)` (evm2's worker rule) with drop-and-count saturation:
/// when the pool is saturated the job is dropped and counted, never fatal.
pub(crate) struct TierPromoter {
    enabled: bool,
    threshold: usize,
    pool: Option<rayon::ThreadPool>,
    pending: Arc<AtomicU64>,
    max_pending: u64,
    stats: Arc<PromotionStats>,
    #[cfg(test)]
    inject_failure: Arc<AtomicU64>,
}

impl Drop for TierPromoter {
    /// Drop the background pool without blocking the caller.
    ///
    /// `rayon::ThreadPool`'s own `Drop` waits for every in-flight promotion
    /// to finish, which would stall VM teardown / `set_jit_policy` while a
    /// promotion is still running. Move the pool onto a detached thread so
    /// teardown returns immediately; the thread exits once the pool (and its
    /// pending tasks) are done.
    fn drop(&mut self) {
        if let Some(pool) = self.pool.take() {
            std::thread::spawn(move || drop(pool));
        }
    }
}

impl std::fmt::Debug for TierPromoter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TierPromoter")
            .field("enabled", &self.enabled)
            .field("threshold", &self.threshold)
            .field("stats", &self.stats.snapshot())
            .finish_non_exhaustive()
    }
}

impl TierPromoter {
    /// Builds a promoter for `policy`.
    ///
    /// Only `Lazy` on x86_64 enables the pool; every other combination
    /// compiles to a no-op that never schedules work.
    pub(crate) fn new(policy: JitPolicy) -> Self {
        let effective = policy.effective();
        let (enabled, threshold) = match effective {
            JitPolicy::Lazy { hot_threshold } => (true, hot_threshold),
            JitPolicy::Eager | JitPolicy::Off => (false, usize::MAX),
        };
        // Off-target builds have no JIT; force the no-op even for `Lazy`.
        let enabled = enabled && cfg!(target_arch = "x86_64");
        let pool = if enabled {
            let cpus = std::thread::available_parallelism().map_or(4, |n| n.get());
            let threads = cpus.div_ceil(2).clamp(1, 4);
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .thread_name(|i| format!("hpsvm-tier-promoter-{i}"))
                .build()
                .ok()
        } else {
            None
        };
        let enabled = enabled && pool.is_some();
        Self {
            enabled,
            threshold,
            pool,
            pending: Arc::new(AtomicU64::new(0)),
            max_pending: 64,
            stats: Arc::new(PromotionStats::default()),
            #[cfg(test)]
            inject_failure: Arc::new(AtomicU64::new(0)),
        }
    }

    /// A disabled promoter (no pool, no work).
    pub(crate) fn disabled() -> Self {
        Self {
            enabled: false,
            threshold: usize::MAX,
            pool: None,
            pending: Arc::new(AtomicU64::new(0)),
            max_pending: 64,
            stats: Arc::new(PromotionStats::default()),
            #[cfg(test)]
            inject_failure: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Whether this promoter ever schedules work.
    pub(crate) const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Returns the shared stats handle.
    pub(crate) fn stats_handle(&self) -> Arc<PromotionStats> {
        Arc::clone(&self.stats)
    }

    /// Forces the next promotion to fail (tests only).
    #[cfg(test)]
    pub(crate) fn set_inject_failure(&self, fail: bool) {
        self.inject_failure.store(u64::from(fail), Ordering::Relaxed);
    }

    /// Whether failure injection is armed (tests only).
    #[cfg(test)]
    fn injects_failure(&self) -> bool {
        self.inject_failure.load(Ordering::Relaxed) != 0
    }

    /// Post-transaction sweep over the committed entries.
    ///
    /// Reads only atomics; promotion itself runs on the pool (or
    /// synchronously for `hot_threshold == 0` so tests are deterministic).
    /// Never fails, never alters transaction results.
    pub(crate) fn observe(&self, accounts: &crate::accounts_db::Accounts) {
        if !self.enabled {
            return;
        }
        let entries = accounts.program_entries_snapshot();
        // `threshold == 0` is the deterministic test mode: promote any
        // compiled-absent `Loaded` entry synchronously on this thread.
        if self.threshold == 0 {
            for entry in entries {
                if has_compiled_program(&entry) {
                    continue;
                }
                if !is_loaded(&entry) {
                    continue;
                }
                self.promote_blocking(entry);
            }
            return;
        }
        for entry in entries {
            if has_compiled_program(&entry) {
                continue;
            }
            if !is_loaded(&entry) {
                continue;
            }
            let interpreted = entry.stats.interpreted_invocations.load(Ordering::Relaxed);
            if interpreted < self.threshold as u64 {
                continue;
            }
            self.promote(entry);
        }
    }

    /// Prewarm path: schedule promotion for an explicit entry.
    ///
    /// Used by `prewarm_programs` under `Lazy`. Respects the compiled
    /// pre-check; races may double-promote (the second compile overwrites
    /// the first), which is tolerated like evm2's resident-map race.
    pub(crate) fn promote(
        &self,
        entry: Arc<solana_program_runtime::program_cache_entry::ProgramCacheEntry>,
    ) {
        if !self.enabled {
            return;
        }
        if has_compiled_program(&entry) {
            return;
        }
        let pending = self.pending.fetch_add(1, Ordering::Relaxed);
        if pending >= self.max_pending {
            self.pending.fetch_sub(1, Ordering::Relaxed);
            self.stats.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let stats = Arc::clone(&self.stats);
        let pending = Arc::clone(&self.pending);
        #[cfg(test)]
        let inject = self.injects_failure();
        #[cfg(not(test))]
        let inject = false;
        let Some(pool) = self.pool.as_ref() else {
            self.pending.fetch_sub(1, Ordering::Relaxed);
            return;
        };
        pool.spawn(move || {
            let _guard = PendingGuard { pending };
            stats.attempted.fetch_add(1, Ordering::Relaxed);
            if inject {
                stats.failed.fetch_add(1, Ordering::Relaxed);
                return;
            }
            let start = Instant::now();
            let outcome = compile_entry(&entry);
            let elapsed = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
            stats.compile_us.fetch_add(elapsed, Ordering::Relaxed);
            if outcome {
                // Mirror agave's load path so per-entry `ProgramStatistics`
                // stay truthful for promoted entries.
                entry.stats.jit_compiled(elapsed);
                stats.succeeded.fetch_add(1, Ordering::Relaxed);
            } else {
                stats.failed.fetch_add(1, Ordering::Relaxed);
            }
        });
    }

    /// Synchronous promotion for `hot_threshold == 0` and tests.
    pub(crate) fn promote_blocking(
        &self,
        entry: Arc<solana_program_runtime::program_cache_entry::ProgramCacheEntry>,
    ) {
        if !self.enabled {
            return;
        }
        if has_compiled_program(&entry) {
            return;
        }
        self.stats.attempted.fetch_add(1, Ordering::Relaxed);
        #[cfg(test)]
        if self.injects_failure() {
            self.stats.failed.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let start = Instant::now();
        let outcome = compile_entry(&entry);
        let elapsed = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
        self.stats.compile_us.fetch_add(elapsed, Ordering::Relaxed);
        if outcome {
            // Mirror agave's load path so per-entry `ProgramStatistics`
            // stay truthful for promoted entries.
            entry.stats.jit_compiled(elapsed);
            self.stats.succeeded.fetch_add(1, Ordering::Relaxed);
        } else {
            self.stats.failed.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Whether the entry is a `Loaded` executable (the only promotable kind).
fn is_loaded(entry: &solana_program_runtime::program_cache_entry::ProgramCacheEntry) -> bool {
    use solana_program_runtime::program_cache_entry::ProgramCacheEntryType;
    matches!(entry.program, ProgramCacheEntryType::Loaded(_))
}

/// Runs `jit_compile` on a `Loaded` entry.
///
/// Returns `true` when a compiled program is present afterwards. Failures
/// leave the entry interpreted. Off x86_64 this is always `false`.
fn compile_entry(entry: &solana_program_runtime::program_cache_entry::ProgramCacheEntry) -> bool {
    #[cfg(all(not(target_os = "windows"), target_arch = "x86_64"))]
    {
        use solana_program_runtime::program_cache_entry::ProgramCacheEntryType;
        match &entry.program {
            ProgramCacheEntryType::Loaded(executable) => {
                if executable.get_compiled_program().is_some() {
                    return true;
                }
                if executable.jit_compile().is_err() {
                    return false;
                }
                executable.get_compiled_program().is_some()
            }
            _ => false,
        }
    }
    #[cfg(not(all(not(target_os = "windows"), target_arch = "x86_64")))]
    {
        let _ = entry;
        false
    }
}

/// Decrements the pending count when a pool job finishes.
struct PendingGuard {
    pending: Arc<AtomicU64>,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.pending.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn disabled_promoter_never_enables() {
        let promoter = TierPromoter::disabled();
        assert!(!promoter.is_enabled());
    }

    #[test]
    fn eager_and_off_are_disabled() {
        assert!(!TierPromoter::new(JitPolicy::Eager).is_enabled());
        assert!(!TierPromoter::new(JitPolicy::Off).is_enabled());
    }

    #[test]
    fn lazy_enabled_only_on_x86_64() {
        let promoter = TierPromoter::new(JitPolicy::Lazy { hot_threshold: 8 });
        assert_eq!(promoter.is_enabled(), cfg!(target_arch = "x86_64"));
    }

    #[test]
    fn is_loaded_distinguishes_loaded_from_tombstones() {
        use solana_program_runtime::program_cache_entry::{
            ProgramCacheEntry, ProgramCacheEntryOwner, ProgramCacheEntryType,
        };

        // Loaded via the uncompiled path must promote.
        let loaded = {
            use solana_program_runtime::{
                loaded_programs::ProgramRuntimeEnvironment, solana_sbpf::program::BuiltinProgram,
            };
            let env =
                ProgramRuntimeEnvironment::from(BuiltinProgram::new_loader(Default::default()));
            let elf = include_bytes!("../../elf/spl_memo-3.0.0.so");
            let loader = solana_sdk_ids::bpf_loader::id();
            load_program_uncompiled(&loader, &env, elf).expect("load should succeed")
        };
        assert!(is_loaded(&loaded));

        // Tombstones and builtins must never promote.
        let closed = ProgramCacheEntry {
            program: ProgramCacheEntryType::Closed,
            account_owner: ProgramCacheEntryOwner::LoaderV3,
            deployment_slot: 0,
            stats: std::sync::Arc::new(
                solana_program_runtime::program_metrics::ProgramStatistics::default(),
            ),
            latest_access_slot: std::sync::atomic::AtomicU64::new(0),
        };
        assert!(!is_loaded(&closed));
    }

    #[test]
    fn pending_guard_releases_on_drop() {
        let pending = std::sync::Arc::new(AtomicU64::new(1));
        {
            let _guard = PendingGuard { pending: std::sync::Arc::clone(&pending) };
            assert_eq!(pending.load(Ordering::Relaxed), 1);
        }
        assert_eq!(pending.load(Ordering::Relaxed), 0);
    }

    proptest! {
        /// Promotion observation is idempotent on an empty cache and never
        /// promotes under `Eager`/`Off`.
        #[test]
        fn observe_is_idempotent_and_never_promotes_under_eager_or_off(
            threshold in any::<usize>(),
        ) {
            use crate::accounts_db::Accounts;

            for policy in [
                JitPolicy::Eager,
                JitPolicy::Off,
                JitPolicy::Lazy { hot_threshold: threshold },
            ] {
                let promoter = TierPromoter::new(policy);
                let accounts = Accounts::default();
                let before = promoter.stats_handle().snapshot();
                promoter.observe(&accounts);
                promoter.observe(&accounts);
                let after = promoter.stats_handle().snapshot();
                if matches!(policy.effective(), JitPolicy::Eager | JitPolicy::Off) {
                    prop_assert!(!promoter.is_enabled());
                    prop_assert_eq!(before.attempted, after.attempted);
                    prop_assert_eq!(after.attempted, 0);
                } else {
                    // Empty cache: nothing to promote even when enabled.
                    prop_assert_eq!(after.attempted, 0);
                }
            }
        }
    }
}
