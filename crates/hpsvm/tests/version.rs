//! `Version` and `ClusterSpecId` replace `SvmCfg` and `RuntimeEnv`.
//!
//! Every profile's base must reproduce the current default fee structure, rent
//! parameters and feature set exactly; `base()` must hand out `&'static`
//! references; `with_feature` must flip exactly one gate.

use hpsvm::{ClusterSpecId, Version};

fn takes_static(_: &'static Version) {}

#[test]
fn base_reproduces_current_defaults_for_every_profile() {
    for profile in ClusterSpecId::ALL {
        let version = Version::base(profile);
        takes_static(version);
        // Exact fee structure of `FeeStructure::default()`.
        assert_eq!(version.fee_structure.lamports_per_signature, 5_000);
        assert_eq!(version.fee_structure, solana_fee_structure::FeeStructure::default());
        // Exact rent parameters of `Rent::default()`.
        assert_eq!(version.rent, solana_rent::Rent::default());
        for data_len in [0usize, 82, 165] {
            assert_eq!(
                version.rent.minimum_balance(data_len),
                solana_rent::Rent::default().minimum_balance(data_len)
            );
        }
        // Exact feature set of the current default configuration.
        assert_eq!(version.feature_set.as_ref(), &agave_feature_set::FeatureSet::all_enabled());
        // Exact runtime defaults.
        assert_eq!(version.compute_budget, None);
        assert_eq!(version.limits.log_bytes_limit, Some(10_000));
        assert!(version.sigverify);
        assert!(version.blockhash_check);
        assert!(version.compute_diagnostics);
    }
}

#[test]
fn base_profiles_differ_only_in_cluster_info() {
    let mainnet = Version::base(ClusterSpecId::MainnetBeta);
    for profile in ClusterSpecId::ALL {
        let version = Version::base(profile);
        assert_eq!(version.cluster.spec_id, profile);
        assert_eq!(version.fee_structure, mainnet.fee_structure);
        assert_eq!(version.rent, mainnet.rent);
        assert_eq!(version.feature_set, mainnet.feature_set);
        assert_eq!(version.compute_budget, mainnet.compute_budget);
        assert_eq!(version.limits.log_bytes_limit, mainnet.limits.log_bytes_limit);
    }
    // Repeat calls return the same `&Version` pointer.
    for profile in ClusterSpecId::ALL {
        assert!(std::ptr::eq(Version::base(profile), Version::base(profile)));
    }
}

#[test]
fn with_feature_flips_exactly_one_gate() {
    let toggled = agave_feature_set::raise_cpi_nesting_limit_to_8::ID;
    let probe = agave_feature_set::increase_tx_account_lock_limit::id();
    assert!(Version::base(ClusterSpecId::MainnetBeta).feature_set.is_active(&toggled));

    let disabled = Version::base(ClusterSpecId::MainnetBeta).clone().with_feature(&toggled, false);
    assert!(!disabled.feature_set.is_active(&toggled));
    assert!(disabled.feature_set.is_active(&probe));

    let enabled = disabled.with_feature(&toggled, true);
    assert!(enabled.feature_set.is_active(&toggled));
    assert!(enabled.feature_set.is_active(&probe));
}
