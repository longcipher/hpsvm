//! Compiled-program tiering policy (`JitPolicy`).
//!
//! The policy decides **whether, when, and on which thread** a program
//! becomes native code. The compiler itself is agave's `solana-sbpf` JIT;
//! this enum only drives the existing tiering seam
//! (`ExecutionMode::PreferJit` + `Executable::jit_compile`).
//!
//! * [`JitPolicy::Eager`] — compile every loaded program at load time on x86_64. Today's behavior
//!   via agave's `ProgramCacheEntry::load`.
//! * [`JitPolicy::Lazy`] — interpret until a program crosses `hot_threshold` interpreted
//!   invocations, then promote it on a background pool.
//! * [`JitPolicy::Off`] — never compile; force the interpreter even on x86_64. Diagnostics and the
//!   fixture equivalence gate.
//!
//! The policy lives on [`Version`](crate::Version) as runtime data (not a
//! cargo feature) so a workload can toggle it per VM without a rebuild.
//! On non-x86_64 targets every variant collapses to `Off` because sbpf
//! exposes no JIT there.

/// Compiled-program tiering policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JitPolicy {
    /// Compile every loaded program to native code (x86_64 only).
    ///
    /// Today's behavior via agave's `ProgramCacheEntry::load`.
    Eager,
    /// Interpret until a program crosses `hot_threshold` invocations,
    /// then promote it on a background pool.
    ///
    /// `hot_threshold: 0` means "compile synchronously at first use",
    /// the deterministic mode the tests use.
    Lazy {
        /// Interpreted-invocation count at which promotion is scheduled.
        hot_threshold: usize,
    },
    /// Never compile; force the interpreter even on x86_64.
    ///
    /// Diagnostics and the fixture equivalence gate. Applies to the load
    /// path (uncompiled entry on a store miss) and disables promotion;
    /// an already-compiled resident entry shared via the process-wide store
    /// is still re-linked as-is (see the store's sharing note).
    Off,
}

impl JitPolicy {
    /// Default policy: [`JitPolicy::Eager`].
    pub const DEFAULT: Self = Self::Eager;

    /// Whether the load path must bypass `jit_compile`.
    ///
    /// Only meaningful on x86_64; off x86_64 nothing ever compiles.
    #[must_use]
    pub const fn requires_uncompiled_load(self) -> bool {
        !matches!(self, Self::Eager)
    }

    /// The policy as observed on this target.
    ///
    /// Everything collapses to [`JitPolicy::Off`] off x86_64 because
    /// `solana-sbpf` gates its JIT on `target_arch = "x86_64"`.
    #[must_use]
    pub const fn effective(self) -> Self {
        if cfg!(target_arch = "x86_64") { self } else { Self::Off }
    }

    /// Whether background promotion is active under this policy.
    #[must_use]
    pub const fn is_lazy(self) -> bool {
        matches!(self.effective(), Self::Lazy { .. })
    }

    /// The hot threshold when [`JitPolicy::Lazy`], else `None`.
    #[must_use]
    pub const fn hot_threshold(self) -> Option<usize> {
        match self.effective() {
            Self::Lazy { hot_threshold } => Some(hot_threshold),
            Self::Eager | Self::Off => None,
        }
    }
}

impl Default for JitPolicy {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_eager() {
        assert_eq!(JitPolicy::default(), JitPolicy::Eager);
        assert_eq!(JitPolicy::DEFAULT, JitPolicy::Eager);
    }

    #[test]
    fn requires_uncompiled_load_per_variant() {
        assert!(!JitPolicy::Eager.requires_uncompiled_load());
        assert!(JitPolicy::Lazy { hot_threshold: 8 }.requires_uncompiled_load());
        assert!(JitPolicy::Lazy { hot_threshold: 0 }.requires_uncompiled_load());
        assert!(JitPolicy::Off.requires_uncompiled_load());
    }

    #[test]
    fn effective_collapses_off_x86_64() {
        if cfg!(target_arch = "x86_64") {
            assert_eq!(JitPolicy::Eager.effective(), JitPolicy::Eager);
            assert_eq!(
                JitPolicy::Lazy { hot_threshold: 8 }.effective(),
                JitPolicy::Lazy { hot_threshold: 8 }
            );
            assert_eq!(JitPolicy::Off.effective(), JitPolicy::Off);
        } else {
            assert_eq!(JitPolicy::Eager.effective(), JitPolicy::Off);
            assert_eq!(JitPolicy::Lazy { hot_threshold: 8 }.effective(), JitPolicy::Off);
            assert_eq!(JitPolicy::Off.effective(), JitPolicy::Off);
        }
    }

    #[test]
    fn lazy_threshold_accessor() {
        assert_eq!(JitPolicy::Lazy { hot_threshold: 8 }.hot_threshold(), {
            if cfg!(target_arch = "x86_64") { Some(8) } else { None }
        });
        assert_eq!(JitPolicy::Eager.hot_threshold(), None);
        assert_eq!(JitPolicy::Off.hot_threshold(), None);
        assert!(JitPolicy::Lazy { hot_threshold: 8 }.is_lazy() == cfg!(target_arch = "x86_64"));
        assert!(!JitPolicy::Eager.is_lazy());
        assert!(!JitPolicy::Off.is_lazy());
    }
}
