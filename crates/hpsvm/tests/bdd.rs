use std::{fmt, fmt::Debug, path::PathBuf};

use agave_feature_set::{FeatureSet, raise_cpi_nesting_limit_to_8};
use cucumber::{World as _, given, then, when};
use hpsvm::{HPSVM, types::TransactionMetadata};
use solana_account::Account;
use solana_address::{Address, address};
use solana_sdk_ids::{bpf_loader, bpf_loader_upgradeable, system_program};
use solana_system_interface::instruction::transfer;

#[derive(Default, cucumber::World)]
struct FeatureSetWorld {
    svm: Option<HPSVM>,
    recipient: Option<Address>,
    last_meta: Option<TransactionMetadata>,
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
    let mut svm = world.svm.take().expect("world should contain an HPSVM instance");
    svm.set_feature_set(FeatureSet::default()).expect("feature-set reconfiguration should succeed");
    world.svm = Some(svm);
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

#[when("a direct system transfer instruction is processed")]
fn process_system_transfer_instruction(world: &mut FeatureSetWorld) {
    let mut svm = world.svm.take().expect("world should contain an HPSVM instance");
    let sender = Address::new_unique();
    let recipient = Address::new_unique();
    svm.set_account(sender, Account::new(10_000, 0, &system_program::id()))
        .expect("sender account should be inserted");

    let meta = svm
        .process_instruction(transfer(&sender, &recipient, 64))
        .expect("instruction should succeed");

    world.recipient = Some(recipient);
    world.last_meta = Some(meta);
    world.svm = Some(svm);
}

#[then("the recipient account should be committed")]
fn recipient_account_committed(world: &mut FeatureSetWorld) {
    let svm = world.svm.as_ref().expect("world should contain an HPSVM instance");
    let recipient = world.recipient.expect("scenario should record recipient");

    assert_eq!(svm.get_balance(&recipient), Some(64));
}

#[then("the instruction metadata should include account diagnostics")]
fn instruction_metadata_has_account_diagnostics(world: &mut FeatureSetWorld) {
    let meta = world.last_meta.as_ref().expect("scenario should record metadata");
    let recipient = world.recipient.expect("scenario should record recipient");

    assert!(
        meta.diagnostics
            .account_diffs
            .iter()
            .any(|diff| diff.address == recipient && diff.pre.is_none())
    );
    assert!(!meta.diagnostics.execution_trace.instructions.is_empty());
}

#[tokio::test]
async fn bdd() {
    let features_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("features");

    FeatureSetWorld::cucumber().run(features_dir).await;
}
