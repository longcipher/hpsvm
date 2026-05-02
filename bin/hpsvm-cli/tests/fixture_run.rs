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

struct FixtureDir(PathBuf);

impl FixtureDir {
    fn new(stem: &str) -> Self {
        let unique = Address::new_unique();
        let process = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time must be after unix epoch")
            .as_nanos();
        Self(std::env::temp_dir().join(format!("{stem}-{process}-{nanos}-{unique}")))
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn snapshot_account(svm: &HPSVM, address: Address) -> AccountSnapshot {
    let account = svm.get_account(&address).expect("account must exist");
    AccountSnapshot::from_readable(address, &account)
}

fn fixture_path(stem: &str) -> PathBuf {
    let unique = Address::new_unique();
    std::env::temp_dir().join(format!("{stem}-{unique}.json"))
}

fn write_fixture(name: &str, blockhash_check: bool) -> PathBuf {
    let path = fixture_path("hpsvm-cli-run");
    write_fixture_to_path(name, blockhash_check, path.clone(), FixtureFormat::Json);
    path
}

fn write_fixture_to_path(name: &str, blockhash_check: bool, path: PathBuf, format: FixtureFormat) {
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
        .runtime(RuntimeFixtureConfig::new(svm.block_env().slot, None, true, blockhash_check))
        .pre_accounts(vec![
            snapshot_account(&svm, payer.pubkey()),
            snapshot_account(&svm, recipient),
        ])
        .baseline(baseline)
        .compares(Compare::everything())
        .capture_transaction(&tx)
        .expect("fixture capture must succeed");

    fixture.save(&path, format).expect("fixture save must succeed");
}

#[test]
fn fixture_run_reports_pass_for_matching_fixture() {
    let path = write_fixture("cli-run", false);

    let output = Command::new(env!("CARGO_BIN_EXE_hpsvm"))
        .args(["fixture", "run", path.to_str().expect("temp path must be valid utf-8")])
        .output()
        .expect("run command must execute");

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("PASS:"));

    std::fs::remove_file(path).ok();
}

#[test]
fn fixture_run_reports_pass_for_fixture_directory() {
    let dir = FixtureDir::new("hpsvm-cli-run-dir");
    std::fs::create_dir(dir.path()).expect("fixture directory must be created");
    write_fixture_to_path(
        "cli-run-dir-second",
        false,
        dir.path().join("b.json"),
        FixtureFormat::Json,
    );
    write_fixture_to_path(
        "cli-run-dir-first",
        false,
        dir.path().join("a.bin"),
        FixtureFormat::Binary,
    );
    std::fs::write(dir.path().join("notes.txt"), "ignore me").expect("notes file must be written");

    let output = Command::new(env!("CARGO_BIN_EXE_hpsvm"))
        .args(["fixture", "run", dir.path().to_str().expect("temp path must be valid utf-8")])
        .output()
        .expect("run command must execute");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first = stdout.find("PASS: cli-run-dir-first").expect("first fixture should pass");
    let second = stdout.find("PASS: cli-run-dir-second").expect("second fixture should pass");
    assert!(first < second, "fixtures should run in sorted path order: {stdout}");
}

#[test]
fn fixture_run_rejects_empty_fixture_directory() {
    let dir = FixtureDir::new("hpsvm-cli-run-empty-dir");
    std::fs::create_dir(dir.path()).expect("fixture directory must be created");
    std::fs::write(dir.path().join("notes.txt"), "ignore me").expect("notes file must be written");

    let output = Command::new(env!("CARGO_BIN_EXE_hpsvm"))
        .args(["fixture", "run", dir.path().to_str().expect("temp path must be valid utf-8")])
        .output()
        .expect("run command must execute");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no fixture files found"));
}

#[test]
fn fixture_run_rejects_blockhash_checked_fixture() {
    let path = write_fixture("cli-run-blockhash", true);

    let output = Command::new(env!("CARGO_BIN_EXE_hpsvm"))
        .args(["fixture", "run", path.to_str().expect("temp path must be valid utf-8")])
        .output()
        .expect("run command must execute");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("blockhash_check"));

    std::fs::remove_file(path).ok();
}

#[test]
fn fixture_run_rejects_invalid_program_mapping() {
    let path = write_fixture("cli-run-programs", false);

    let output = Command::new(env!("CARGO_BIN_EXE_hpsvm"))
        .args([
            "fixture",
            "run",
            path.to_str().expect("temp path must be valid utf-8"),
            "--program",
            "not-a-mapping",
        ])
        .output()
        .expect("run command must execute");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("expected <program-id>=<path>"));

    std::fs::remove_file(path).ok();
}
