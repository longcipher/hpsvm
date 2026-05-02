#![allow(missing_docs)]

use std::{path::PathBuf, process::Command};

use hpsvm::HPSVM;
use hpsvm_fixture::{
    AccountSnapshot, CaptureBuilder, Compare, ExecutionSnapshot, FixtureFormat,
    RuntimeFixtureConfig,
};
use serde_json::Value;
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::versioned::VersionedTransaction;

fn snapshot_account(svm: &HPSVM, address: Address) -> AccountSnapshot {
    let account = svm.get_account(&address).expect("account must exist");
    AccountSnapshot::from_readable(address, &account)
}

fn fixture_path(stem: &str) -> PathBuf {
    let unique = Address::new_unique();
    std::env::temp_dir().join(format!("{stem}-{unique}.json"))
}

fn write_fixture() -> PathBuf {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).expect("payer airdrop must succeed");
    svm.airdrop(&recipient, 1).expect("recipient airdrop must succeed");
    let tx = VersionedTransaction::from(solana_transaction::Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    ));
    let baseline = ExecutionSnapshot::from_outcome(&svm.transact(tx.clone()));
    let fixture = CaptureBuilder::new("cli-inspect")
        .runtime(RuntimeFixtureConfig::new(svm.block_env().slot, None, true, false))
        .pre_accounts(vec![
            snapshot_account(&svm, payer.pubkey()),
            snapshot_account(&svm, recipient),
        ])
        .baseline(baseline)
        .compares(Compare::everything())
        .capture_transaction(&tx)
        .expect("fixture capture must succeed");

    let path = fixture_path("hpsvm-cli-inspect");
    fixture.save(&path, FixtureFormat::Json).expect("fixture save must succeed");
    path
}

#[test]
fn fixture_inspect_prints_fixture_name() {
    let path = write_fixture();

    let output = Command::new(env!("CARGO_BIN_EXE_hpsvm"))
        .args(["fixture", "inspect", path.to_str().expect("temp path must be valid utf-8")])
        .output()
        .expect("inspect command must execute");

    assert!(output.status.success());

    let decoded: Value = serde_json::from_slice(&output.stdout).expect("stdout must be json");
    assert_eq!(decoded["header"]["name"], Value::String(String::from("cli-inspect")));

    std::fs::remove_file(path).ok();
}
