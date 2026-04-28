use agave_feature_set::{FeatureSet, raise_cpi_nesting_limit_to_8};
use hpsvm::HPSVM;
use solana_address::address;
use solana_feature_gate_interface::{self as feature_gate, Feature};
use solana_sdk_ids::{bpf_loader, bpf_loader_upgradeable};

#[test_log::test]
fn new_initializes_accounts_for_enabled_features() {
    let svm = HPSVM::new();
    let feature_id = raise_cpi_nesting_limit_to_8::id();

    let account = svm.get_account(&feature_id).expect("active feature account should exist");
    let feature = feature_gate::from_account(&account).expect("feature account should deserialize");

    assert_eq!(account.owner, solana_sdk_ids::feature::id());
    assert_eq!(feature, Feature { activated_at: Some(0) });
    assert!(
        account.lamports >= svm.minimum_balance_for_rent_exemption(Feature::size_of()),
        "feature account should be rent exempt"
    );
}

#[test_log::test]
fn with_feature_set_rebuilds_materialized_defaults() {
    let feature_id = raise_cpi_nesting_limit_to_8::id();
    let token_program_id = address!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

    let mut svm = HPSVM::new();
    assert_eq!(
        svm.get_account(&token_program_id).expect("token program should exist").owner,
        bpf_loader_upgradeable::id()
    );
    assert!(svm.get_account(&feature_id).is_some());

    svm.set_feature_set(FeatureSet::default());

    assert!(svm.get_account(&feature_id).is_none());
    assert_eq!(
        svm.get_account(&token_program_id)
            .expect("token program should be rebuilt for the new feature set")
            .owner,
        bpf_loader::id()
    );
}
