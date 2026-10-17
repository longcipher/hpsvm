//! Tier stats snapshot (evm2 `RuntimeStats` analogue).
//!
//! [`ProgramTierStats`] is assembled from atomics only — agave's per-entry
//! `ProgramStatistics` plus hpsvm-side promotion counters — so reporting
//! never takes a lock on the transaction path beyond the short snapshot.
//! The snapshot feeds the existing hotpath/tracing surface.

use std::sync::atomic::Ordering;

use crate::programs::{jit_policy::JitPolicy, tiering::PromotionStats};

/// Observable compiled-tier execution state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgramTierStats {
    /// Policy the VM was configured with.
    pub policy: JitPolicy,
    /// Policy as observed on this target (`Off` off x86_64).
    pub effective_policy: JitPolicy,
    /// Resident entries in the process-wide content store.
    pub resident_entries: usize,
    /// Entries in the committed program cache.
    pub cache_entries: usize,
    /// Total JIT compilations recorded by agave (`ProgramStatistics`).
    pub compilations: u64,
    /// Total JIT compile time in microseconds (agave side).
    pub total_compile_time_us: u64,
    /// Total interpreted invocations (agave side).
    pub interpreted_invocations: u64,
    /// Total JIT invocations (agave side).
    pub jit_invocations: u64,
    /// Promotion jobs scheduled (hpsvm side).
    pub promotions_attempted: u64,
    /// Promotions that published a compiled program.
    pub promotions_succeeded: u64,
    /// Promotions that failed (entry stays interpreted).
    pub promotions_failed: u64,
    /// Jobs dropped because the pool was saturated.
    pub promotions_dropped: u64,
    /// Total `jit_compile` microseconds spent by the promoter.
    pub promotion_compile_us: u64,
}

impl ProgramTierStats {
    /// Assembles a snapshot from the committed entries, the resident store
    /// and the promoter counters. All reads are atomic.
    pub(crate) fn collect(
        policy: JitPolicy,
        accounts: &crate::accounts_db::Accounts,
        resident_entries: usize,
        promotion: &PromotionStats,
    ) -> Self {
        let entries = accounts.program_entries_snapshot();
        let ord = Ordering::Relaxed;
        let mut compilations = 0u64;
        let mut total_compile_time_us = 0u64;
        let mut interpreted_invocations = 0u64;
        let mut jit_invocations = 0u64;
        for entry in &entries {
            compilations = compilations.saturating_add(entry.stats.compilations.load(ord));
            total_compile_time_us = total_compile_time_us
                .saturating_add(entry.stats.total_compilation_time_us.load(ord));
            interpreted_invocations = interpreted_invocations
                .saturating_add(entry.stats.interpreted_invocations.load(ord));
            jit_invocations = jit_invocations.saturating_add(entry.stats.jit_invocations.load(ord));
        }
        let snapshot = promotion.snapshot();
        Self {
            policy,
            effective_policy: policy.effective(),
            resident_entries,
            cache_entries: entries.len(),
            compilations,
            total_compile_time_us,
            interpreted_invocations,
            jit_invocations,
            promotions_attempted: snapshot.attempted,
            promotions_succeeded: snapshot.succeeded,
            promotions_failed: snapshot.failed,
            promotions_dropped: snapshot.dropped,
            promotion_compile_us: snapshot.compile_us,
        }
    }

    /// Zero snapshot used off x86_64 and for disabled promoters.
    pub(crate) const fn zero(policy: JitPolicy) -> Self {
        Self {
            policy,
            effective_policy: JitPolicy::Off,
            resident_entries: 0,
            cache_entries: 0,
            compilations: 0,
            total_compile_time_us: 0,
            interpreted_invocations: 0,
            jit_invocations: 0,
            promotions_attempted: 0,
            promotions_succeeded: 0,
            promotions_failed: 0,
            promotions_dropped: 0,
            promotion_compile_us: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_snapshot_reports_off_effective() {
        let stats = ProgramTierStats::zero(JitPolicy::Eager);
        assert_eq!(stats.effective_policy, JitPolicy::Off);
        assert_eq!(stats.compilations, 0);
        assert_eq!(stats.promotions_attempted, 0);
    }
}
