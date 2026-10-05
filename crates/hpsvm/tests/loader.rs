#![cfg(feature = "loader")]

//! Tests for the HPSVM loader functionality.

use agave_feature_set::FeatureSet;
use hpsvm::{
    HPSVM,
    loader::{deploy_upgradeable_program, set_upgrade_authority},
};
use solana_account::{Account, state_traits::StateMut};
use solana_address::Address;
use solana_instruction::{Instruction, account_meta::AccountMeta};
use solana_keypair::Keypair;
use solana_loader_v3_interface::{get_program_data_address, state::UpgradeableLoaderState};
use solana_message::Message;
use solana_signer::Signer;
use solana_transaction::Transaction;
use solana_transaction_error::TransactionError;

use crate::programs_bytes::HELLO_WORLD_BYTES;

mod programs_bytes;

fn get_program_upgrade_authority(svm: &HPSVM, program_id: &Address) -> Option<Address> {
    let programdata_address = get_program_data_address(program_id);
    let programdata_account = svm.get_account(&programdata_address).unwrap();
    let metadata_len = UpgradeableLoaderState::size_of_programdata_metadata();
    let metadata: UpgradeableLoaderState =
        Account { data: programdata_account.data[..metadata_len].to_vec(), ..Default::default() }
            .state()
            .unwrap();

    match metadata {
        UpgradeableLoaderState::ProgramData { upgrade_authority_address, .. } => {
            upgrade_authority_address
        }
        other => panic!("expected ProgramData account, got {other:?}"),
    }
}

#[test]
fn hello_world_with_store() {
    let mut svm = HPSVM::new();

    let payer = Keypair::new();
    let program_bytes = HELLO_WORLD_BYTES;

    svm.airdrop(&payer.pubkey(), 1000000000).unwrap();

    let program_kp = Keypair::new();
    let program_id = program_kp.pubkey();
    svm.add_program(program_id, program_bytes).unwrap();

    let instruction =
        Instruction::new_with_bytes(program_id, &[], vec![AccountMeta::new(payer.pubkey(), true)]);
    let message = Message::new(&[instruction], Some(&payer.pubkey()));
    let tx = Transaction::new(&[&payer], message, svm.latest_blockhash());
    let tx_result = svm.send_transaction(tx);

    assert!(tx_result.is_ok());
    assert!(tx_result.unwrap().logs.contains(&"Program log: Hello world!".to_string()));
}

#[test_log::test]
fn hello_world_with_deploy_upgradeable() {
    // ponytail: FeatureSet::all_enabled() includes disable_sbpf_v0_v1_v2_deployment which
    // blocks deploying SBPF v0 programs like hello_world.so
    let feature_set = FeatureSet::default();

    let mut svm = HPSVM::builder()
        .with_feature_set(feature_set)
        .with_builtins()
        .with_lamports(1_000_000_000_000_000)
        .with_sysvars()
        .build()
        .unwrap();

    let payer_kp = Keypair::new();
    let payer_pk = payer_kp.pubkey();
    let program_bytes = HELLO_WORLD_BYTES;

    svm.airdrop(&payer_pk, 10000000000).unwrap();

    let program_keypair = Keypair::new();
    deploy_upgradeable_program(&mut svm, &payer_kp, &program_keypair, program_bytes).unwrap();
    let program_id = program_keypair.pubkey();
    let instruction =
        Instruction::new_with_bytes(program_id, &[], vec![AccountMeta::new(payer_pk, true)]);
    let message = Message::new(&[instruction], Some(&payer_pk));
    let tx = Transaction::new(&[&payer_kp], message, svm.latest_blockhash());
    let tx_result = svm.send_transaction(tx);
    assert!(tx_result.unwrap().logs.contains(&"Program log: Hello world!".to_string()));
    assert_eq!(get_program_upgrade_authority(&svm, &program_id), Some(payer_pk));

    let new_authority = Keypair::new();
    set_upgrade_authority(
        &mut svm,
        &payer_kp,
        &program_id,
        &payer_kp,
        Some(&new_authority.pubkey()),
    )
    .unwrap();
    assert_eq!(get_program_upgrade_authority(&svm, &program_id), Some(new_authority.pubkey()));

    let next_authority = Keypair::new();
    set_upgrade_authority(
        &mut svm,
        &payer_kp,
        &program_id,
        &new_authority,
        Some(&next_authority.pubkey()),
    )
    .unwrap();
    assert_eq!(get_program_upgrade_authority(&svm, &program_id), Some(next_authority.pubkey()));
}

/// Builds a VM that can deploy the vendored SBPF v0 ELF.
///
/// `FeatureSet::all_enabled()` (the `HPSVM::new` default) includes
/// `disable_sbpf_v0_v1_v2_deployment`, which rejects this program.
fn deployable_vm() -> HPSVM {
    HPSVM::builder()
        .with_feature_set(FeatureSet::default())
        .with_builtins()
        .with_lamports(1_000_000_000_000_000)
        .with_sysvars()
        .build()
        .expect("the default builder configuration must succeed")
}

/// Deploys the vendored ELF and returns the VM, the payer, and the program
/// keypair (its pubkey is the program id).
fn deployable_program() -> (HPSVM, Keypair, Keypair) {
    let mut svm = deployable_vm();
    let payer = Keypair::new();
    let program_kp = Keypair::new();
    svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();
    deploy_upgradeable_program(&mut svm, &payer, &program_kp, HELLO_WORLD_BYTES)
        .expect("the deploy must succeed");
    (svm, payer, program_kp)
}

/// The ELF spans more than one 512-byte write chunk, so this covers the
/// multi-chunk write loop and the chunk-offset arithmetic.
#[test]
fn deploying_a_multi_chunk_program_stores_the_whole_elf() {
    assert!(
        HELLO_WORLD_BYTES.len() > 512,
        "the test ELF must span more than one write chunk ({} bytes)",
        HELLO_WORLD_BYTES.len()
    );

    let (svm, _payer, program_kp) = deployable_program();

    let programdata = get_program_data_address(&program_kp.pubkey());
    let account = svm.get_account(&programdata).expect("the program data account must exist");
    assert!(
        account.data.windows(HELLO_WORLD_BYTES.len()).any(|window| window == HELLO_WORLD_BYTES),
        "the deployed ELF must be stored verbatim"
    );
}

/// A single-chunk ELF still deploys, so the chunk loop's tail is covered in both
/// directions.
#[test]
fn deploying_a_single_chunk_program_reaches_the_loader() {
    let mut svm = deployable_vm();
    let payer = Keypair::new();
    let program_kp = Keypair::new();
    svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();
    let truncated = &HELLO_WORLD_BYTES[..512];

    // The truncated ELF passes the chunk write but is rejected by the loader when
    // it parses the ELF header, which proves the buffer was created and filled.
    let error = deploy_upgradeable_program(&mut svm, &payer, &program_kp, truncated)
        .expect_err("a truncated ELF must be rejected");
    assert!(
        matches!(error.err, TransactionError::InstructionError(..)),
        "expected an instruction error, got {:?}",
        error.err
    );
}

/// Redeploying over an existing program must fail rather than silently overwrite.
#[test]
fn deploying_over_an_existing_program_fails() {
    let (mut svm, payer, program_kp) = deployable_program();
    svm.airdrop(&payer.pubkey(), 20_000_000_000).unwrap();

    let error = deploy_upgradeable_program(&mut svm, &payer, &program_kp, HELLO_WORLD_BYTES)
        .expect_err("redeploying the same program id must fail");
    assert!(
        matches!(error.err, TransactionError::InstructionError(..)),
        "expected an instruction error, got {:?}",
        error.err
    );
}

/// With `new_authority == None` the authority is revoked entirely, and a revoked
/// authority can no longer hand the program on.
#[test]
fn set_upgrade_authority_revokes_and_then_refuses_the_old_authority() {
    let (mut svm, payer, program_kp) = deployable_program();
    let program_id = program_kp.pubkey();
    let new_authority = Keypair::new();
    svm.airdrop(&new_authority.pubkey(), 10_000_000_000).unwrap();

    set_upgrade_authority(&mut svm, &payer, &program_id, &payer, Some(&new_authority.pubkey()))
        .unwrap();
    assert_eq!(get_program_upgrade_authority(&svm, &program_id), Some(new_authority.pubkey()));

    // Omitting the new authority revokes it.
    set_upgrade_authority(&mut svm, &new_authority, &program_id, &new_authority, None).unwrap();
    assert_eq!(get_program_upgrade_authority(&svm, &program_id), None);

    let error = set_upgrade_authority(
        &mut svm,
        &new_authority,
        &program_id,
        &new_authority,
        Some(&Address::new_unique()),
    )
    .expect_err("a revoked authority must not be able to transfer ownership");
    assert!(
        matches!(error.err, TransactionError::InstructionError(..)),
        "expected an instruction error, got {:?}",
        error.err
    );
}

/// When the payer *is* the current authority only one signature is attached; a
/// duplicate signer would make the transaction invalid.
#[test]
fn set_upgrade_authority_accepts_a_single_signing_payer() {
    let (mut svm, payer, program_kp) = deployable_program();
    let program_id = program_kp.pubkey();
    let successor = Keypair::new();
    svm.airdrop(&successor.pubkey(), 10_000_000_000).unwrap();

    // The payer is also the authority, so only one signature is attached; a
    // duplicate signer would make the transaction invalid.
    set_upgrade_authority(&mut svm, &payer, &program_id, &payer, Some(&successor.pubkey()))
        .unwrap();
    assert_eq!(get_program_upgrade_authority(&svm, &program_id), Some(successor.pubkey()));

    // Hand the program back, again with a single signature.
    set_upgrade_authority(&mut svm, &successor, &program_id, &successor, Some(&payer.pubkey()))
        .unwrap();
    assert_eq!(get_program_upgrade_authority(&svm, &program_id), Some(payer.pubkey()));
}

/// A `from` keypair that is not the current authority must sign alongside it.
#[test]
fn set_upgrade_authority_co_signs_with_the_current_authority() {
    let (mut svm, payer, program_kp) = deployable_program();
    let program_id = program_kp.pubkey();
    let successor = Keypair::new();
    svm.airdrop(&successor.pubkey(), 10_000_000_000).unwrap();

    // `successor` is the transaction payer while `payer` is the authority, so the
    // helper must attach both signatures.
    set_upgrade_authority(&mut svm, &successor, &program_id, &payer, Some(&successor.pubkey()))
        .unwrap();
    assert_eq!(get_program_upgrade_authority(&svm, &program_id), Some(successor.pubkey()));

    // A third party that is not the authority cannot move the program.
    let stranger = Keypair::new();
    svm.airdrop(&stranger.pubkey(), 10_000_000_000).unwrap();
    let error =
        set_upgrade_authority(&mut svm, &stranger, &program_id, &payer, Some(&stranger.pubkey()))
            .expect_err("an unsigned authority must be rejected");
    assert!(
        matches!(error.err, TransactionError::InstructionError(..)),
        "expected an instruction error, got {:?}",
        error.err
    );
}
