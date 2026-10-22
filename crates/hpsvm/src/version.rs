//! Execution-shaping protocol configuration: one value replacing `SvmCfg` + `RuntimeEnv`.
//!
//! [`Version`] carries every knob that shapes execution independently from block
//! state: the feature set, fee structure, compute budget, rent parameters,
//! numeric runtime limits, validation flags and the cluster profile it was
//! derived from. Values are data, not branches: a custom environment starts
//! from [`Version::base`] and overrides fields (notably via
//! [`Version::with_feature`]) instead of forking control flow.
//!
//! # Why `base` is not `const`
//!
//! The design sketch asks for `pub const fn base(spec_id: ClusterSpecId) ->
//! &'static Self`. That is infeasible: [`FeatureSet`][agave_feature_set::FeatureSet]
//! contains `AHashMap`/`AHashSet`, which cannot be const-constructed, so no
//! `Version` holding a feature set can ever be built in a `const` context.
//! Pragmatically, [`Version::base`] is a plain `pub fn` backed by
//! [`std::sync::OnceLock`] statics (the same pattern `runtime_env.rs` already
//! uses for the shared program runtime environment). The `&'static` guarantee
//! is kept — callers can bind the result to a `&'static Version` — only the
//! `const`-evaluability is dropped.
//!
//! # Current defaults
//!
//! All four profiles reproduce the exact defaults the VM used before this
//! module existed and differ only in [`ClusterInfo`]:
//!
//! * feature set: [`FeatureSet::all_enabled`][agave_feature_set::FeatureSet::all_enabled] (the same
//!   set `Hpsvm::builder().with_program_test_defaults()` installs);
//! * fee structure: [`FeeStructure::default`] (`lamports_per_signature == 5_000`);
//! * rent: [`Rent::default`];
//! * compute budget: `None` (derived per transaction);
//! * log-bytes limit: `Some(10_000)`;
//! * `sigverify` / `blockhash_check`: `true` (the `Hpsvm::new()` final state);
//! * `compute_diagnostics`: `true` (the spec's backward-compatibility default).
//! * JIT policy: [`JitPolicy::Eager`](crate::JitPolicy::Eager) (today's behavior).
//!
//! Protocol differences between clusters arrive later via [`Version::with_feature`]
//! overrides, not via the base table.

use std::sync::{Arc, OnceLock};

use agave_feature_set::{
    FeatureSet, deprecate_rent_exemption_threshold, increase_tx_account_lock_limit,
    raise_cpi_nesting_limit_to_8, replace_spl_token_with_p_token,
};
use solana_address::Address;
use solana_compute_budget::compute_budget::ComputeBudget;
use solana_fee_structure::FeeStructure;
use solana_rent::Rent;

/// Built-in cluster profiles, each indexing one [`Version::base`] entry.
///
/// `#[repr(u32)]` keeps the discriminant stable so it can cross FFI and
/// diagnostic boundaries; [`BaseSvmTypes`][crate::BaseSvmTypes] uses this type
/// directly as its `SpecId`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum ClusterSpecId {
    /// Solana mainnet-beta profile.
    MainnetBeta = 0,
    /// Solana testnet profile.
    Testnet = 1,
    /// Solana devnet profile.
    Devnet = 2,
    /// Next (future) cluster profile.
    Next = 3,
}

impl ClusterSpecId {
    /// All built-in profiles in discriminant order.
    pub const ALL: [Self; 4] = [Self::MainnetBeta, Self::Testnet, Self::Devnet, Self::Next];

    /// Short human-readable name of the profile.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::MainnetBeta => "mainnet-beta",
            Self::Testnet => "testnet",
            Self::Devnet => "devnet",
            Self::Next => "next",
        }
    }
}

/// Numeric runtime limits shaping execution.
///
/// Migrated from `RuntimeEnv`: the log-truncation ceiling lives here while the
/// (optional) compute-budget override stays a top-level [`Version`] field so
/// the `None`-means-derive-per-transaction semantics are preserved exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeLimits {
    /// Truncation ceiling for captured program logs, in bytes. `None` disables
    /// truncation. Default: `Some(10_000)`.
    pub log_bytes_limit: Option<usize>,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self { log_bytes_limit: Some(10_000) }
    }
}

/// Identity of the cluster profile a [`Version`] was derived from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClusterInfo {
    /// Which built-in profile this version tracks.
    pub spec_id: ClusterSpecId,
    /// Short human-readable name of the profile.
    pub name: &'static str,
}

use crate::programs::JitPolicy;

/// All execution-shaping configuration in one value.
///
/// `Clone` (not `Copy`): the feature set is reference-counted via `Arc`.
#[derive(Debug, Clone)]
pub struct Version {
    /// Active protocol feature gates.
    pub feature_set: Arc<FeatureSet>,
    /// Fee structure used for fee calculation.
    pub fee_structure: FeeStructure,
    /// Compute-budget override. `None` derives the budget per transaction from
    /// the compute-budget instructions and feature gates.
    pub compute_budget: Option<ComputeBudget>,
    /// Rent parameters used for rent-exemption checks and sysvar materialization.
    pub rent: Rent,
    /// Numeric runtime limits.
    pub limits: RuntimeLimits,
    /// Cluster profile this version was derived from.
    pub cluster: ClusterInfo,
    /// Whether transactions go through signature verification.
    ///
    /// Carried over from `SvmCfg`.
    pub sigverify: bool,
    /// Whether the recent-blockhash / nonce check runs.
    ///
    /// Carried over from `SvmCfg`.
    pub blockhash_check: bool,
    /// Whether transaction outcomes carry pre/post diffs and token balances.
    ///
    /// Carried over from `SvmCfg`.
    ///
    /// Defaults to `true` for backward compatibility (the performance spec's
    /// `2026-06-17-02` Approach pins the default). Computing the diagnostics
    /// reloads the pre-state of every touched account and scans the account
    /// set for token balances, so turning it off is the lever for callers
    /// that do not read them.
    pub compute_diagnostics: bool,
    /// Compiled-program tiering policy.
    ///
    /// Defaults to [`JitPolicy::Eager`] (today's behavior). Override per VM
    /// via [`Version::with_jit_policy`] or the builder's `with_jit_policy`.
    pub jit_policy: JitPolicy,
}

/// Process-wide base table, built once. See the module docs for why this is a
/// `OnceLock` instead of `const` data.
static BASE_VERSIONS: OnceLock<[Version; 4]> = OnceLock::new();

fn build_base_table() -> [Version; 4] {
    ClusterSpecId::ALL.map(|spec_id| Version {
        feature_set: Arc::new(FeatureSet::all_enabled()),
        fee_structure: FeeStructure::default(),
        compute_budget: None,
        rent: Rent::default(),
        limits: RuntimeLimits::default(),
        cluster: ClusterInfo { spec_id, name: spec_id.name() },
        sigverify: true,
        blockhash_check: true,
        compute_diagnostics: true,
        jit_policy: JitPolicy::DEFAULT,
    })
}

impl Version {
    /// Returns the base version for a built-in cluster profile.
    ///
    /// All profiles share the current defaults and differ only in
    /// [`ClusterInfo`]; protocol differences arrive via [`Version::with_feature`]
    /// overrides. The same `&Version` pointer is returned on every call.
    ///
    /// (Deviates from the design sketch, which asks for `const fn`: a `const`
    /// context cannot build the `AHashMap` inside `FeatureSet`. This is a
    /// plain `fn` returning `&'static Self` from `OnceLock` statics.)
    #[must_use]
    pub fn base(spec_id: ClusterSpecId) -> &'static Self {
        &BASE_VERSIONS.get_or_init(build_base_table)[spec_id as usize]
    }

    /// Returns a copy of this version with the JIT tiering policy replaced.
    ///
    /// The default is [`JitPolicy::Eager`]; `Lazy` opts into
    /// interpreter-first tiering and `Off` forces the interpreter even on
    /// x86_64 (diagnostics / equivalence gate).
    #[must_use]
    pub const fn with_jit_policy(mut self, policy: JitPolicy) -> Self {
        self.jit_policy = policy;
        self
    }

    /// Returns a copy of this version with one feature gate flipped.
    ///
    /// The key type is [`Address`][solana_address::Address], which is the real
    /// `FeatureSet` key: `solana_pubkey::Pubkey` is a re-export alias of it, so
    /// `agave_feature_set::...::ID` values pass directly. Enabling activates
    /// the gate at slot 0; disabling removes it. Every other gate is untouched.
    #[must_use]
    pub fn with_feature(mut self, id: &Address, enabled: bool) -> Self {
        let mut feature_set = (*self.feature_set).clone();
        if enabled {
            feature_set.activate(id, 0);
        } else {
            feature_set.deactivate(id);
        }
        self.feature_set = Arc::new(feature_set);
        self
    }
}

/// Static feature gates resolved once per transaction into [`TxFeatureFlags`].
///
/// Only gates with a fixed id consulted on the execution hot path are listed.
/// Dynamic ids (per-builtin `enable_feature_id`, per-precompile `feature`, the
/// precompile callback's `feature_id` parameter) cannot be enumerated up front
/// and keep their direct `is_active` queries; see the call-site comments.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FeatureGate {
    /// `raise_cpi_nesting_limit_to_8`: deeper CPI nesting and compute budget.
    RaiseCpiNestingLimitTo8,
    /// `deprecate_rent_exemption_threshold`: rent sysvar materialization.
    DeprecateRentExemptionThreshold,
    /// `increase_tx_account_lock_limit`: wider per-transaction account locks.
    IncreaseTxAccountLockLimit,
    /// `replace_spl_token_with_p_token`: p-token program selection.
    ReplaceSplTokenWithPToken,
}

impl FeatureGate {
    /// Every hoisted gate, in declaration order.
    pub const ALL: &'static [Self] = &[
        Self::RaiseCpiNestingLimitTo8,
        Self::DeprecateRentExemptionThreshold,
        Self::IncreaseTxAccountLockLimit,
        Self::ReplaceSplTokenWithPToken,
    ];

    /// The agave feature id backing this gate.
    #[must_use]
    pub const fn id(self) -> Address {
        match self {
            Self::RaiseCpiNestingLimitTo8 => raise_cpi_nesting_limit_to_8::ID,
            Self::DeprecateRentExemptionThreshold => deprecate_rent_exemption_threshold::ID,
            Self::IncreaseTxAccountLockLimit => increase_tx_account_lock_limit::ID,
            Self::ReplaceSplTokenWithPToken => replace_spl_token_with_p_token::ID,
        }
    }
}

/// Feature-gate decisions for one transaction, resolved once via [`TxFeatureFlags::load`].
///
/// Call sites on the per-transaction path read these fields instead of calling
/// `FeatureSet::is_active`, so the number of feature-set consultations per
/// transaction is constant. The struct is `Copy` so entries can thread it (or
/// a reference to it) down the execution path without allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxFeatureFlags {
    /// Cached `raise_cpi_nesting_limit_to_8` decision.
    pub raise_cpi_nesting_limit_to_8: bool,
    /// Cached `deprecate_rent_exemption_threshold` decision.
    pub deprecate_rent_exemption_threshold: bool,
    /// Cached `increase_tx_account_lock_limit` decision.
    pub increase_tx_account_lock_limit: bool,
    /// Cached `replace_spl_token_with_p_token` decision.
    pub replace_spl_token_with_p_token: bool,
}

#[cfg(test)]
std::thread_local! {
    // Thread-local (not process-wide) so parallel test threads executing
    // transactions cannot pollute each other's load counts.
    static TX_FEATURE_FLAGS_LOADS: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
}

impl TxFeatureFlags {
    /// Resolves every hoisted gate against `feature_set` in one pass.
    #[must_use]
    pub fn load(feature_set: &FeatureSet) -> Self {
        #[cfg(test)]
        TX_FEATURE_FLAGS_LOADS.with(|count| count.set(count.get() + 1));
        Self {
            raise_cpi_nesting_limit_to_8: feature_set.is_active(&raise_cpi_nesting_limit_to_8::ID),
            deprecate_rent_exemption_threshold: feature_set
                .is_active(&deprecate_rent_exemption_threshold::ID),
            increase_tx_account_lock_limit: feature_set
                .is_active(&increase_tx_account_lock_limit::ID),
            replace_spl_token_with_p_token: feature_set
                .is_active(&replace_spl_token_with_p_token::ID),
        }
    }

    /// Returns the cached decision for `gate`.
    ///
    /// The `match` is exhaustive over [`FeatureGate`] so adding a gate without
    /// a backing field fails to compile.
    #[must_use]
    pub const fn is_enabled(&self, gate: FeatureGate) -> bool {
        match gate {
            FeatureGate::RaiseCpiNestingLimitTo8 => self.raise_cpi_nesting_limit_to_8,
            FeatureGate::DeprecateRentExemptionThreshold => self.deprecate_rent_exemption_threshold,
            FeatureGate::IncreaseTxAccountLockLimit => self.increase_tx_account_lock_limit,
            FeatureGate::ReplaceSplTokenWithPToken => self.replace_spl_token_with_p_token,
        }
    }

    /// Number of [`TxFeatureFlags::load`] calls on this thread (tests only).
    #[cfg(test)]
    pub(crate) fn load_count() -> usize {
        TX_FEATURE_FLAGS_LOADS.with(|count| count.get())
    }

    /// Resets the per-thread load counter (tests only).
    #[cfg(test)]
    pub(crate) fn reset_load_count() {
        TX_FEATURE_FLAGS_LOADS.with(|count| count.set(0));
    }
}

#[cfg(test)]
mod tests {
    use std::ptr;

    use agave_feature_set::{FeatureSet, raise_cpi_nesting_limit_to_8};
    use solana_fee_structure::{FeeBin, FeeStructure};
    use solana_rent::{DEFAULT_LAMPORTS_PER_BYTE, Rent};

    use super::*;
    use crate::SvmTypesHost;

    fn takes_static(_: &'static Version) {}

    #[test]
    fn cluster_spec_id_is_a_stable_u32() {
        assert_eq!(size_of::<ClusterSpecId>(), 4);
        assert_eq!(ClusterSpecId::MainnetBeta as u32, 0);
        assert_eq!(ClusterSpecId::Testnet as u32, 1);
        assert_eq!(ClusterSpecId::Devnet as u32, 2);
        assert_eq!(ClusterSpecId::Next as u32, 3);
        assert_eq!(ClusterSpecId::ALL.len(), 4);
    }

    #[test]
    fn cluster_names_are_distinct_and_match_spec() {
        for spec_id in ClusterSpecId::ALL {
            assert_eq!(Version::base(spec_id).cluster.spec_id, spec_id);
            assert_eq!(Version::base(spec_id).cluster.name, spec_id.name());
        }
        let names: Vec<&str> = ClusterSpecId::ALL.iter().map(|id| id.name()).collect();
        let mut deduped = names.clone();
        deduped.sort_unstable();
        deduped.dedup();
        assert_eq!(deduped.len(), names.len(), "cluster names must be distinct");
    }

    #[test]
    fn base_reproduces_exact_fee_values_for_every_profile() {
        let expected = FeeStructure {
            lamports_per_signature: 5_000,
            lamports_per_write_lock: 0,
            compute_fee_bins: vec![FeeBin { limit: 1_400_000, fee: 0 }],
        };
        assert_eq!(FeeStructure::default(), expected);
        for spec_id in ClusterSpecId::ALL {
            let version = Version::base(spec_id);
            takes_static(version);
            assert_eq!(version.fee_structure, expected);
            assert_eq!(version.fee_structure.lamports_per_signature, 5_000);
        }
    }

    #[test]
    fn base_reproduces_exact_rent_values_for_every_profile() {
        let expected = Rent::default();
        assert_eq!(expected.lamports_per_byte, DEFAULT_LAMPORTS_PER_BYTE);
        assert_eq!(expected.lamports_per_byte, 6_960);
        for data_len in [0usize, 82, 165, 355] {
            let expected_balance = expected.minimum_balance(data_len);
            for spec_id in ClusterSpecId::ALL {
                let version = Version::base(spec_id);
                assert_eq!(version.rent, expected);
                assert_eq!(version.rent.minimum_balance(data_len), expected_balance);
            }
        }
    }

    #[test]
    fn base_enables_every_feature_for_every_profile() {
        let all_enabled = FeatureSet::all_enabled();
        assert!(!all_enabled.active().is_empty());
        for spec_id in ClusterSpecId::ALL {
            let version = Version::base(spec_id);
            assert_eq!(version.feature_set.as_ref(), &all_enabled);
        }
    }

    #[test]
    fn base_carries_current_runtime_defaults_for_every_profile() {
        for spec_id in ClusterSpecId::ALL {
            let version = Version::base(spec_id);
            assert_eq!(version.compute_budget, None);
            assert_eq!(version.limits.log_bytes_limit, Some(10_000));
            assert!(version.sigverify);
            assert!(version.blockhash_check);
            assert!(version.compute_diagnostics);
            assert_eq!(version.jit_policy, crate::JitPolicy::Eager);
        }
    }

    #[test]
    fn base_returns_the_same_pointer_on_repeat_calls() {
        for spec_id in ClusterSpecId::ALL {
            assert!(ptr::eq(Version::base(spec_id), Version::base(spec_id)));
        }
    }

    #[test]
    fn with_feature_flips_exactly_one_gate_in_both_directions() {
        let probe = raise_cpi_nesting_limit_to_8::ID;
        let control = agave_feature_set::increase_tx_account_lock_limit::id();
        assert!(Version::base(ClusterSpecId::MainnetBeta).feature_set.is_active(&probe));
        assert!(Version::base(ClusterSpecId::MainnetBeta).feature_set.is_active(&control));

        let disabled =
            Version::base(ClusterSpecId::MainnetBeta).clone().with_feature(&probe, false);
        assert!(!disabled.feature_set.is_active(&probe));
        assert!(disabled.feature_set.is_active(&control));
        // The base table itself must be untouched by the clone-and-flip.
        assert!(Version::base(ClusterSpecId::MainnetBeta).feature_set.is_active(&probe));

        let re_enabled = disabled.with_feature(&probe, true);
        assert!(re_enabled.feature_set.is_active(&probe));
        assert!(re_enabled.feature_set.is_active(&control));
        assert_eq!(re_enabled.feature_set.as_ref(), &FeatureSet::all_enabled());
    }

    #[test]
    fn with_feature_accepts_agave_feature_ids_directly() {
        // `solana_pubkey::Pubkey` is an alias of `solana_address::Address`, so
        // the real `FeatureSet` key type flows through with no conversion.
        let id: Address = agave_feature_set::deprecate_rent_exemption_threshold::id();
        let version = Version::base(ClusterSpecId::Testnet).clone().with_feature(&id, false);
        assert!(!version.feature_set.is_active(&id));
    }

    #[test]
    fn cluster_spec_id_satisfies_the_deferred_spec_id_bound() {
        fn takes_spec_id<T: SvmTypesHost<SpecId = ClusterSpecId>>() {}
        takes_spec_id::<crate::BaseSvmTypes>();
        fn into_spec_id<T: Into<ClusterSpecId>>(id: T) -> ClusterSpecId {
            id.into()
        }
        assert_eq!(into_spec_id(ClusterSpecId::Devnet), ClusterSpecId::Devnet);
    }

    // `FeatureGate` and `TxFeatureFlags` did not exist yet, so the
    // so this module must fail to compile with unresolved imports until the
    // hoisted flag container landed without them.
    use super::{FeatureGate, TxFeatureFlags};

    #[test]
    fn feature_gate_all_covers_every_hoisted_flag() {
        assert_eq!(FeatureGate::ALL.len(), 4, "one entry per TxFeatureFlags field");
        // `is_enabled` matches exhaustively, so adding a gate without a field
        // fails to compile; this asserts the table and the struct stay in step.
        for gate in FeatureGate::ALL {
            let _ = *gate;
        }
    }

    #[test]
    fn tx_feature_flags_match_feature_set_for_every_gate() {
        for gate in FeatureGate::ALL {
            let all_on = FeatureSet::all_enabled();
            assert_eq!(
                TxFeatureFlags::load(&all_on).is_enabled(*gate),
                all_on.is_active(&gate.id())
            );
            assert!(TxFeatureFlags::load(&all_on).is_enabled(*gate));

            let all_off = FeatureSet::default();
            assert_eq!(
                TxFeatureFlags::load(&all_off).is_enabled(*gate),
                all_off.is_active(&gate.id())
            );
            assert!(!TxFeatureFlags::load(&all_off).is_enabled(*gate));

            // Single gate flipped off via the Version override path.
            let single_off =
                Version::base(ClusterSpecId::MainnetBeta).clone().with_feature(&gate.id(), false);
            assert_eq!(
                TxFeatureFlags::load(&single_off.feature_set).is_enabled(*gate),
                single_off.feature_set.is_active(&gate.id())
            );
            assert!(!TxFeatureFlags::load(&single_off.feature_set).is_enabled(*gate));
            // Every other gate stays on.
            for other in FeatureGate::ALL {
                if *other != *gate {
                    assert!(TxFeatureFlags::load(&single_off.feature_set).is_enabled(*other));
                }
            }

            // Single gate flipped on from the all-off set.
            let mut single_on = FeatureSet::default();
            single_on.activate(&gate.id(), 0);
            assert!(TxFeatureFlags::load(&single_on).is_enabled(*gate));
            for other in FeatureGate::ALL {
                if *other != *gate {
                    assert!(!TxFeatureFlags::load(&single_on).is_enabled(*other));
                }
            }
        }
    }

    #[test]
    fn tx_feature_flags_load_once_per_transaction_regardless_of_size() {
        use solana_keypair::Keypair;
        use solana_message::Message;
        use solana_signer::Signer;
        use solana_system_interface::instruction::transfer;
        use solana_transaction::Transaction;

        let mut svm = crate::Hpsvm::new();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).expect("setup airdrop should succeed");
        let blockhash = svm.latest_blockhash();

        TxFeatureFlags::reset_load_count();
        let recipient = Address::new_unique();
        let one_ix = Transaction::new(
            &[&payer],
            Message::new(&[transfer(&payer.pubkey(), &recipient, 42)], Some(&payer.pubkey())),
            blockhash,
        );
        svm.send_transaction(one_ix).expect("1-instruction transfer should succeed");
        assert_eq!(TxFeatureFlags::load_count(), 1, "exactly one flag load per transaction");

        TxFeatureFlags::reset_load_count();
        let five_ixs: Vec<_> =
            (0..5).map(|_| transfer(&payer.pubkey(), &Address::new_unique(), 42)).collect();
        let five_ix =
            Transaction::new(&[&payer], Message::new(&five_ixs, Some(&payer.pubkey())), blockhash);
        svm.send_transaction(five_ix).expect("5-instruction transfer should succeed");
        assert_eq!(
            TxFeatureFlags::load_count(),
            1,
            "flag loads must not grow with instruction count"
        );
    }
}
