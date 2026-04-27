use std::{fmt, fmt::Debug, path::PathBuf};

use agave_feature_set::{FeatureSet, raise_cpi_nesting_limit_to_8};
use cucumber::{World as _, given, then, when};
use hpsvm::HPSVM;
use solana_address::address;
use solana_sdk_ids::{bpf_loader, bpf_loader_upgradeable};

#[derive(Default, cucumber::World)]
struct FeatureSetWorld {
    svm: Option<HPSVM>,
}

impl Debug for FeatureSetWorld {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FeatureSetWorld").field("has_svm", &self.svm.is_some()).finish()
    }
}

#[given("a default HPSVM instance with all features materialized")]
fn default_hpsvm(world: &mut FeatureSetWorld) {
    let svm = HPSVM::new();
    let token_program_id = address!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

    assert_eq!(
        svm.get_account(&token_program_id).expect("token program should exist").owner,
        bpf_loader_upgradeable::id()
    );
    assert!(svm.get_account(&raise_cpi_nesting_limit_to_8::id()).is_some());

    world.svm = Some(svm);
}

#[when("the VM feature set is replaced with the default disabled feature set")]
fn replace_feature_set(world: &mut FeatureSetWorld) {
    world.svm = Some(
        world
            .svm
            .take()
            .expect("world should contain an HPSVM instance")
            .with_feature_set(FeatureSet::default()),
    );
}

#[then("the old active feature account should be removed")]
fn feature_account_removed(world: &mut FeatureSetWorld) {
    let svm = world.svm.as_ref().expect("world should contain an HPSVM instance");
    assert!(svm.get_account(&raise_cpi_nesting_limit_to_8::id()).is_none());
}

#[then("the SPL token program should use the legacy loader")]
fn token_program_uses_legacy_loader(world: &mut FeatureSetWorld) {
    let svm = world.svm.as_ref().expect("world should contain an HPSVM instance");
    let token_program_id = address!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

    assert_eq!(
        svm.get_account(&token_program_id).expect("token program should exist").owner,
        bpf_loader::id()
    );
}

#[tokio::test]
async fn bdd() {
    let features_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("features");

    FeatureSetWorld::cucumber().run(features_dir).await;
}
