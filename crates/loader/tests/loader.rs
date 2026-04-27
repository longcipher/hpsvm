//! Tests for the HPSVM loader functionality.

use agave_feature_set::FeatureSet;
use hpsvm::HPSVM;
use hpsvm_loader::{deploy_upgradeable_program, set_upgrade_authority};
use solana_account::{Account, state_traits::StateMut};
use solana_address::Address;
use solana_instruction::{Instruction, account_meta::AccountMeta};
use solana_keypair::Keypair;
use solana_loader_v3_interface::{get_program_data_address, state::UpgradeableLoaderState};
use solana_message::Message;
use solana_signer::Signer;
use solana_transaction::Transaction;

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
    let feature_set = FeatureSet::all_enabled();

    let mut svm = HPSVM::default()
        .with_feature_set(feature_set)
        .with_builtins()
        .with_lamports(1_000_000_000_000_000)
        .with_sysvars();

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
