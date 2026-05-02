#![allow(missing_docs)]

use std::{path::PathBuf, process::Command};

use hpsvm::HPSVM;
use hpsvm_fixture::{
    AccountSnapshot, CaptureBuilder, Compare, ExecutionSnapshot, FixtureFormat,
    RuntimeFixtureConfig,
};
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

fn report_dir(stem: &str) -> PathBuf {
    let unique = Address::new_unique();
    std::env::temp_dir().join(format!("{stem}-{unique}"))
}

fn write_fixture(name: &str) -> PathBuf {
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
    let fixture = CaptureBuilder::new(name)
        .runtime(RuntimeFixtureConfig::new(svm.block_env().slot, None, true, false))
        .pre_accounts(vec![
            snapshot_account(&svm, payer.pubkey()),
            snapshot_account(&svm, recipient),
        ])
        .baseline(baseline)
        .compares(Compare::everything())
        .capture_transaction(&tx)
        .expect("fixture capture must succeed");

    let path = fixture_path("hpsvm-cli-cu-report");
    fixture.save(&path, FixtureFormat::Json).expect("fixture save must succeed");
    path
}

#[test]
fn cu_report_writes_markdown_report_for_fixture() {
    const REPORT_FILE_NAME: &str = "cu-report.md";

    let fixture_path = write_fixture("cli-cu-report");
    let output_dir = report_dir("hpsvm-cli-cu-report-output");

    let output = Command::new(env!("CARGO_BIN_EXE_hpsvm"))
        .args([
            "cu",
            "report",
            fixture_path.to_str().expect("temp fixture path must be valid utf-8"),
            "--output-dir",
            output_dir.to_str().expect("temp output path must be valid utf-8"),
        ])
        .output()
        .expect("cu report command must execute");

    assert!(output.status.success());
    assert!(output_dir.join(REPORT_FILE_NAME).exists());
    assert!(String::from_utf8_lossy(&output.stdout).contains("PASS:"));

    std::fs::remove_file(fixture_path).ok();
    std::fs::remove_dir_all(output_dir).ok();
}

#[test]
fn cu_report_uses_baseline_dir_to_emit_deltas() {
    const REPORT_FILE_NAME: &str = "cu-report.md";

    let fixture_path = write_fixture("cli-cu-report-baseline");
    let baseline_dir = report_dir("hpsvm-cli-cu-report-baseline");
    let comparison_dir = report_dir("hpsvm-cli-cu-report-comparison");

    let baseline_output = Command::new(env!("CARGO_BIN_EXE_hpsvm"))
        .args([
            "cu",
            "report",
            fixture_path.to_str().expect("temp fixture path must be valid utf-8"),
            "--output-dir",
            baseline_dir.to_str().expect("temp baseline path must be valid utf-8"),
        ])
        .output()
        .expect("baseline cu report command must execute");

    assert!(baseline_output.status.success());

    let comparison_output = Command::new(env!("CARGO_BIN_EXE_hpsvm"))
        .args([
            "cu",
            "report",
            fixture_path.to_str().expect("temp fixture path must be valid utf-8"),
            "--output-dir",
            comparison_dir.to_str().expect("temp comparison path must be valid utf-8"),
            "--baseline-dir",
            baseline_dir.to_str().expect("temp baseline path must be valid utf-8"),
        ])
        .output()
        .expect("comparison cu report command must execute");

    assert!(comparison_output.status.success());

    let markdown = std::fs::read_to_string(comparison_dir.join(REPORT_FILE_NAME))
        .expect("comparison report markdown must exist");
    assert!(markdown.contains("+0 (+0.00%)"));

    std::fs::remove_file(fixture_path).ok();
    std::fs::remove_dir_all(baseline_dir).ok();
    std::fs::remove_dir_all(comparison_dir).ok();
}
