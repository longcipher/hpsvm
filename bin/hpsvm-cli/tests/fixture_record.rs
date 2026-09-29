#![allow(missing_docs)]

use std::{fs, path::PathBuf, process::Command};

use hpsvm::HPSVM;
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::versioned::VersionedTransaction;

struct TempFile(PathBuf);

impl TempFile {
    fn new(stem: &str, extension: &str) -> Self {
        let unique = Address::new_unique();
        let process = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time must be after unix epoch")
            .as_nanos();
        Self(std::env::temp_dir().join(format!("{stem}-{process}-{nanos}-{unique}.{extension}")))
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        fs::remove_file(&self.0).ok();
    }
}

/// A recorded transfer scenario: the signed transaction plus the pre-state it
/// needs, expressed the way `hpsvm fixture record` consumes them.
struct TransferScenario {
    transaction: VersionedTransaction,
    accounts: serde_json::Value,
    payer: Address,
    recipient: Address,
}

fn transfer_scenario() -> TransferScenario {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).expect("payer airdrop must succeed");
    svm.airdrop(&recipient, 1).expect("recipient airdrop must succeed");

    let transaction = VersionedTransaction::from(solana_transaction::Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    ));

    let accounts = serde_json::json!([
        {
            "address": payer.pubkey().to_string(),
            "lamports": 10_000u64,
            "owner": solana_sdk_ids::system_program::id().to_string(),
            "executable": false,
            "rent_epoch": 0,
            "data": [],
        },
        {
            "address": recipient.to_string(),
            "lamports": 1u64,
            "owner": solana_sdk_ids::system_program::id().to_string(),
            "executable": false,
            "rent_epoch": 0,
            "data": [],
        },
    ]);

    TransferScenario { transaction, accounts, payer: payer.pubkey(), recipient }
}

fn write_inputs(scenario: &TransferScenario) -> (TempFile, TempFile) {
    let transaction_file = TempFile::new("hpsvm-cli-record-tx", "bin");
    fs::write(
        transaction_file.path(),
        wincode::serialize(&scenario.transaction).expect("transaction encoding must succeed"),
    )
    .expect("transaction file must be written");

    let accounts_file = TempFile::new("hpsvm-cli-record-accounts", "json");
    fs::write(
        accounts_file.path(),
        serde_json::to_vec(&scenario.accounts).expect("accounts must serialize"),
    )
    .expect("accounts file must be written");

    (transaction_file, accounts_file)
}

fn record_command(
    transaction: &std::path::Path,
    accounts: &std::path::Path,
    output: &std::path::Path,
) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hpsvm"));
    command.args([
        "fixture",
        "record",
        "--transaction",
        transaction.to_str().expect("temp path must be valid utf-8"),
        "--accounts",
        accounts.to_str().expect("temp path must be valid utf-8"),
        "--output",
        output.to_str().expect("temp path must be valid utf-8"),
    ]);
    command
}

fn utf8(path: &std::path::Path) -> String {
    path.to_str().expect("temp path must be valid utf-8").to_owned()
}

#[test]
fn fixture_record_writes_a_fixture_that_replays_cleanly() {
    let scenario = transfer_scenario();
    let (transaction_file, accounts_file) = write_inputs(&scenario);
    let output = TempFile::new("hpsvm-cli-record-out", "json");

    let recorded = record_command(transaction_file.path(), accounts_file.path(), output.path())
        .args(["--name", "cli-record"])
        .output()
        .expect("record command must execute");

    assert!(
        recorded.status.success(),
        "record failed: {}",
        String::from_utf8_lossy(&recorded.stderr)
    );
    assert!(output.path().exists(), "record should write the fixture to --output");

    let fixture = fs::read_to_string(output.path()).expect("fixture must be readable");
    let fixture: serde_json::Value =
        serde_json::from_str(&fixture).expect("fixture must be valid json");
    assert_eq!(fixture["header"]["name"], "cli-record");
    assert_eq!(fixture["header"]["kind"], "Transaction");

    // The recorded baseline must reflect the real transfer.
    let baseline = &fixture["expectations"]["baseline"];
    assert_eq!(baseline["status"]["Success"], serde_json::Value::Null);
    assert_eq!(baseline["compute_units_consumed"], 150u64);
    assert_eq!(baseline["fee"], 5000u64);

    let post_payer = baseline["post_accounts"]
        .as_array()
        .expect("post_accounts must be an array")
        .iter()
        .find(|account| account["address"] == serde_json::to_value(scenario.payer).unwrap())
        .expect("payer must appear in post_accounts");
    assert_eq!(post_payer["lamports"], 4936u64);

    let post_recipient = baseline["post_accounts"]
        .as_array()
        .expect("post_accounts must be an array")
        .iter()
        .find(|account| account["address"] == serde_json::to_value(scenario.recipient).unwrap())
        .expect("recipient must appear in post_accounts");
    assert_eq!(post_recipient["lamports"], 65u64);

    // End-to-end: the freshly recorded fixture must replay and pass.
    let replayed = Command::new(env!("CARGO_BIN_EXE_hpsvm"))
        .args(["fixture", "run", &utf8(output.path())])
        .output()
        .expect("replay command must execute");
    assert!(
        replayed.status.success(),
        "recorded fixture should replay: {}",
        String::from_utf8_lossy(&replayed.stderr)
    );
    assert!(String::from_utf8_lossy(&replayed.stdout).contains("PASS: cli-record"));
}

#[test]
fn fixture_record_defaults_the_name_to_the_output_file_stem() {
    let scenario = transfer_scenario();
    let (transaction_file, accounts_file) = write_inputs(&scenario);
    let output = TempFile::new("hpsvm-cli-record-stem", "json");

    let recorded = record_command(transaction_file.path(), accounts_file.path(), output.path())
        .output()
        .expect("record command must execute");
    assert!(
        recorded.status.success(),
        "record failed: {}",
        String::from_utf8_lossy(&recorded.stderr)
    );

    let fixture: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(output.path()).expect("fixture readable"))
            .expect("fixture must be valid json");
    let expected_stem =
        output.path().file_stem().expect("output must have a stem").to_string_lossy().into_owned();
    assert_eq!(fixture["header"]["name"], expected_stem);
}

#[test]
fn fixture_record_accepts_base64_transactions() {
    let scenario = transfer_scenario();
    let accounts_file = TempFile::new("hpsvm-cli-record-b64-accounts", "json");
    fs::write(
        accounts_file.path(),
        serde_json::to_vec(&scenario.accounts).expect("accounts must serialize"),
    )
    .expect("accounts file must be written");

    let encoded = base64_encode(
        &wincode::serialize(&scenario.transaction).expect("transaction encoding must succeed"),
    );
    let transaction_file = TempFile::new("hpsvm-cli-record-b64-tx", "b64");
    fs::write(transaction_file.path(), encoded).expect("transaction file must be written");

    let output = TempFile::new("hpsvm-cli-record-b64-out", "json");
    let recorded = record_command(transaction_file.path(), accounts_file.path(), output.path())
        .args(["--transaction-encoding", "base64"])
        .output()
        .expect("record command must execute");

    assert!(
        recorded.status.success(),
        "record failed: {}",
        String::from_utf8_lossy(&recorded.stderr)
    );
    assert!(output.path().exists());
}

#[test]
fn fixture_record_persists_the_compute_unit_limit_into_the_fixture() {
    // The limit is applied to the recording VM, so it must also be stored in the
    // fixture's runtime block. Otherwise `FixtureRunner` replays under the VM
    // default and a fixture recorded against a tight limit stops reproducing.
    let scenario = transfer_scenario();
    let (transaction_file, accounts_file) = write_inputs(&scenario);
    let output = TempFile::new("hpsvm-cli-record-cu-limit", "json");

    let recorded = record_command(transaction_file.path(), accounts_file.path(), output.path())
        .args(["--compute-unit-limit", "20000"])
        .output()
        .expect("record command must execute");
    assert!(
        recorded.status.success(),
        "record failed: {}",
        String::from_utf8_lossy(&recorded.stderr)
    );

    let fixture: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(output.path()).expect("fixture readable"))
            .expect("fixture must be valid json");

    // `input` is an externally-tagged enum, so a transaction fixture nests
    // under the `Transaction` variant.
    assert_eq!(
        fixture["input"]["Transaction"]["runtime"]["compute_unit_limit"], 20000u64,
        "--compute-unit-limit must be recorded so a replay uses the same budget"
    );
}

#[test]
fn fixture_record_leaves_the_compute_unit_limit_unset_by_default() {
    let scenario = transfer_scenario();
    let (transaction_file, accounts_file) = write_inputs(&scenario);
    let output = TempFile::new("hpsvm-cli-record-no-cu-limit", "json");

    let recorded = record_command(transaction_file.path(), accounts_file.path(), output.path())
        .output()
        .expect("record command must execute");
    assert!(
        recorded.status.success(),
        "record failed: {}",
        String::from_utf8_lossy(&recorded.stderr)
    );

    let fixture: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(output.path()).expect("fixture readable"))
            .expect("fixture must be valid json");

    assert_eq!(
        fixture["input"]["Transaction"]["runtime"]["compute_unit_limit"],
        serde_json::Value::Null,
        "omitting --compute-unit-limit must leave the field unset so the replay \
         keeps the VM default"
    );
}

#[test]
fn fixture_record_compares_everything_by_default() {
    let scenario = transfer_scenario();
    let (transaction_file, accounts_file) = write_inputs(&scenario);
    let output = TempFile::new("hpsvm-cli-record-compares", "json");

    let recorded = record_command(transaction_file.path(), accounts_file.path(), output.path())
        .output()
        .expect("record command must execute");
    assert!(
        recorded.status.success(),
        "record failed: {}",
        String::from_utf8_lossy(&recorded.stderr)
    );

    let fixture: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(output.path()).expect("fixture readable"))
            .expect("fixture must be valid json");
    let compares =
        fixture["expectations"]["compares"].as_array().expect("compares must be an array");

    // A recorded fixture is the regression gate, so it must compare compute
    // units as well as status, fee, logs, and post-state.
    assert!(compares.contains(&serde_json::json!("ComputeUnits")));
    assert!(compares.contains(&serde_json::json!("Status")));
    assert!(compares.contains(&serde_json::json!({"Accounts": "All"})));
}

#[test]
fn fixture_record_can_drop_the_compute_unit_comparison() {
    let scenario = transfer_scenario();
    let (transaction_file, accounts_file) = write_inputs(&scenario);
    let output = TempFile::new("hpsvm-cli-record-no-cu", "json");

    let recorded = record_command(transaction_file.path(), accounts_file.path(), output.path())
        .arg("--ignore-compute-units")
        .output()
        .expect("record command must execute");
    assert!(
        recorded.status.success(),
        "record failed: {}",
        String::from_utf8_lossy(&recorded.stderr)
    );

    let fixture: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(output.path()).expect("fixture readable"))
            .expect("fixture must be valid json");
    let compares =
        fixture["expectations"]["compares"].as_array().expect("compares must be an array");

    assert!(
        !compares.contains(&serde_json::json!("ComputeUnits")),
        "--ignore-compute-units should drop the compute unit check"
    );
    assert!(compares.contains(&serde_json::json!("Status")));
}

#[test]
fn fixture_record_accepts_pre_accounts_copied_from_an_existing_fixture() {
    // A fixture stores `Address` as a 32-byte array, so recording must accept
    // that shape too: pre-accounts should be copyable straight out of a
    // fixture's `input.pre_accounts` without hand-conversion.
    let scenario = transfer_scenario();
    let payer = serde_json::to_value(scenario.payer).expect("payer address must serialize");
    let owner = serde_json::to_value(solana_sdk_ids::system_program::id())
        .expect("system program address must serialize");

    let (transaction_file, _) = write_inputs(&scenario);
    let accounts_file = TempFile::new("hpsvm-cli-record-copied-accounts", "json");
    fs::write(
        accounts_file.path(),
        serde_json::to_vec(&serde_json::json!([{
            "address": payer,
            "lamports": 10_000u64,
            "owner": owner,
            "executable": false,
            "rent_epoch": 0,
            "data": [],
        }]))
        .expect("copied accounts must serialize"),
    )
    .expect("accounts file must be written");

    let output = TempFile::new("hpsvm-cli-record-copied-out", "json");
    let recorded = record_command(transaction_file.path(), accounts_file.path(), output.path())
        .output()
        .expect("record command must execute");

    assert!(
        recorded.status.success(),
        "record failed: {}",
        String::from_utf8_lossy(&recorded.stderr)
    );
    assert!(output.path().exists());
}

#[test]
fn fixture_record_rejects_malformed_accounts() {
    let scenario = transfer_scenario();
    let transaction_file = TempFile::new("hpsvm-cli-record-bad-accounts-tx", "bin");
    fs::write(
        transaction_file.path(),
        wincode::serialize(&scenario.transaction).expect("transaction encoding must succeed"),
    )
    .expect("transaction file must be written");

    let accounts_file = TempFile::new("hpsvm-cli-record-bad-accounts", "json");
    fs::write(
        accounts_file.path(),
        serde_json::to_vec(&serde_json::json!([
            { "address": "not-a-pubkey", "lamports": 1u64, "owner": "11111111111111111111111111111111" }
        ]))
        .expect("bad accounts must serialize"),
    )
    .expect("accounts file must be written");

    let output = TempFile::new("hpsvm-cli-record-bad-accounts-out", "json");
    let recorded = record_command(transaction_file.path(), accounts_file.path(), output.path())
        .output()
        .expect("record command must execute");

    assert!(!recorded.status.success());
    assert!(!output.path().exists(), "no fixture should be written on failure");
    let stderr = String::from_utf8_lossy(&recorded.stderr);
    assert!(stderr.contains("not-a-pubkey"), "unexpected stderr: {stderr}");
}

#[test]
fn fixture_record_rejects_an_undecodable_transaction() {
    let accounts_file = TempFile::new("hpsvm-cli-record-bad-tx-accounts", "json");
    fs::write(accounts_file.path(), b"[]").expect("accounts file must be written");

    let transaction_file = TempFile::new("hpsvm-cli-record-bad-tx", "bin");
    fs::write(transaction_file.path(), [0xff, 0xfe, 0xfd, 0xfc])
        .expect("transaction file must be written");

    let output = TempFile::new("hpsvm-cli-record-bad-tx-out", "json");
    let recorded = record_command(transaction_file.path(), accounts_file.path(), output.path())
        .output()
        .expect("record command must execute");

    assert!(!recorded.status.success());
    assert!(!output.path().exists(), "no fixture should be written on failure");
    let stderr = String::from_utf8_lossy(&recorded.stderr);
    assert!(stderr.contains("transaction"), "unexpected stderr: {stderr}");
}

#[test]
fn fixture_record_rejects_an_unsupported_output_extension() {
    let scenario = transfer_scenario();
    let (transaction_file, accounts_file) = write_inputs(&scenario);
    let output = TempFile::new("hpsvm-cli-record-bad-ext", "txt");

    let recorded = record_command(transaction_file.path(), accounts_file.path(), output.path())
        .output()
        .expect("record command must execute");

    assert!(!recorded.status.success());
    assert!(!output.path().exists());
    let stderr = String::from_utf8_lossy(&recorded.stderr);
    assert!(stderr.contains("unsupported"), "unexpected stderr: {stderr}");
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(chunk.get(1).copied().unwrap_or_default());
        let b2 = u32::from(chunk.get(2).copied().unwrap_or_default());
        let triple = (b0 << 16) | (b1 << 8) | b2;
        let sextet = |shift: u32| char::from(ALPHABET[((triple >> shift) & 0x3f) as usize]);
        encoded.push(sextet(18));
        encoded.push(sextet(12));
        encoded.push(if chunk.len() > 1 { sextet(6) } else { '=' });
        encoded.push(if chunk.len() > 2 { sextet(0) } else { '=' });
    }
    encoded
}
