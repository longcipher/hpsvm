use agave_feature_set::{FeatureSet, raise_cpi_nesting_limit_to_8};
use hpsvm::HPSVM;
use solana_address::address;
use solana_feature_gate_interface::{self as feature_gate, Feature};
use solana_native_token::LAMPORTS_PER_SOL;
use solana_sdk_ids::{bpf_loader, bpf_loader_upgradeable};

#[test_log::test]
fn builder_program_test_defaults_match_new() {
    let svm = HPSVM::builder().with_program_test_defaults().build().unwrap();
    let baseline = HPSVM::new();
    let feature_id = raise_cpi_nesting_limit_to_8::id();
    let token_program_id = address!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

    let builder_feature = svm.get_account(&feature_id).expect("builder should materialize feature account");
    let baseline_feature =
        baseline.get_account(&feature_id).expect("new should materialize feature account");

    assert_eq!(feature_gate::from_account(&builder_feature), feature_gate::from_account(&baseline_feature));
    assert_eq!(
        svm.get_account(&token_program_id).expect("builder should materialize token program").owner,
        baseline.get_account(&token_program_id).expect("new should materialize token program").owner,
    );
    assert_eq!(svm.latest_blockhash(), baseline.latest_blockhash());
}

#[test_log::test]
fn builder_locks_feature_set_before_materialization() {
    let feature_id = raise_cpi_nesting_limit_to_8::id();
    let token_program_id = address!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

    let svm = HPSVM::builder()
        .with_feature_set(FeatureSet::default())
        .with_lamports(1_000_000u64.wrapping_mul(LAMPORTS_PER_SOL))
        .with_sysvars()
        .with_feature_accounts()
        .with_default_programs()
        .build()
        .unwrap();

    assert!(svm.get_account(&feature_id).is_none());
    assert_eq!(
        svm.get_account(&token_program_id)
            .expect("token program should be built for the configured feature set")
            .owner,
        bpf_loader::id()
    );
}

#[test_log::test]
fn builder_can_customize_feature_set_before_defaults() {
    let feature_id = raise_cpi_nesting_limit_to_8::id();
    let token_program_id = address!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

    let svm = HPSVM::builder()
        .with_feature_set(FeatureSet::all_enabled())
        .with_program_test_defaults()
        .build()
        .unwrap();

    let feature_account = svm.get_account(&feature_id).expect("active feature account should exist");
    let feature = feature_gate::from_account(&feature_account).expect("feature should deserialize");

    assert_eq!(feature, Feature { activated_at: Some(0) });
    assert_eq!(
        svm.get_account(&token_program_id).expect("default token program should exist").owner,
        bpf_loader_upgradeable::id()
    );
}