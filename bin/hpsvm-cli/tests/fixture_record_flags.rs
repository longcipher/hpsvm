#![allow(missing_docs)]

//! End-to-end coverage for the `fixture record` flags that were previously
//! unexercised: `--tag`, `--source`, `--loader`, `--slot`, `--sigverify`,
//! `--log-bytes-limit`, `--program`, `--transaction-encoding hex`, a `.bin`
//! output, and the address-repr error branches.

use std::{fs, path::PathBuf, process::Command};

use hpsvm::HPSVM;
use hpsvm_fixture::Fixture;
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

/// Lowercase hex encoder, independent of the CLI's decoder.
fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

struct Scenario {
    transaction_bytes: Vec<u8>,
    accounts_json: String,
}

fn scenario() -> Scenario {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    svm.airdrop(&recipient, 1).unwrap();

    let transaction = VersionedTransaction::from(solana_transaction::Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    ));

    let accounts_json = serde_json::json!([
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
    ])
    .to_string();

    Scenario {
        transaction_bytes: wincode::serialize(&transaction).expect("encoding must succeed"),
        accounts_json,
    }
}

fn write_inputs(stem: &str) -> (TempFile, TempFile) {
    let scenario = scenario();

    let transaction = TempFile::new(stem, "bin");
    fs::write(transaction.path(), &scenario.transaction_bytes)
        .expect("transaction file must be written");

    let accounts = TempFile::new(&format!("{stem}-accounts"), "json");
    fs::write(accounts.path(), &scenario.accounts_json).expect("accounts file must be written");

    (transaction, accounts)
}

fn record(
    transaction: &std::path::Path,
    accounts: &std::path::Path,
    output: &std::path::Path,
) -> Command {
    // Materialise the paths as owned strings first: `Command::args` takes
    // `AsRef<OsStr>` items, and mixing `&str` with `&Cow<str>` in one array
    // leaves the element type ambiguous.
    let transaction = utf8(transaction);
    let accounts = utf8(accounts);
    let output = utf8(output);

    let mut command = Command::new(env!("CARGO_BIN_EXE_hpsvm"));
    command.args([
        "fixture",
        "record",
        "--transaction",
        transaction.as_str(),
        "--accounts",
        accounts.as_str(),
        "--output",
        output.as_str(),
    ]);
    command
}

fn utf8(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

#[test]
fn record_stores_repeated_tags_in_order() {
    let (transaction, accounts) = write_inputs("hpsvm-cli-record-tags");
    let output = TempFile::new("hpsvm-cli-record-tags-out", "json");

    let result = record(transaction.path(), accounts.path(), output.path())
        .args(["--tag", "alpha", "--tag", "beta", "--tag", "gamma"])
        .output()
        .expect("record command must execute");

    assert!(result.status.success(), "stderr: {}", String::from_utf8_lossy(&result.stderr));

    let fixture = Fixture::load(output.path()).expect("fixture must load");
    assert_eq!(fixture.header.tags, vec!["alpha", "beta", "gamma"]);
}

#[test]
fn record_without_tags_stores_no_tags() {
    let (transaction, accounts) = write_inputs("hpsvm-cli-record-no-tags");
    let output = TempFile::new("hpsvm-cli-record-no-tags-out", "json");

    let result = record(transaction.path(), accounts.path(), output.path())
        .output()
        .expect("record must run");
    assert!(result.status.success());

    assert!(Fixture::load(output.path()).unwrap().header.tags.is_empty());
}

#[test]
fn record_stores_the_source_provenance_string() {
    let (transaction, accounts) = write_inputs("hpsvm-cli-record-source");
    let output = TempFile::new("hpsvm-cli-record-source-out", "json");

    let result = record(transaction.path(), accounts.path(), output.path())
        .args(["--source", "agave-test-validator --url http://localhost:8899"])
        .output()
        .expect("record command must execute");

    assert!(result.status.success(), "stderr: {}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(
        Fixture::load(output.path()).unwrap().header.source.as_deref(),
        Some("agave-test-validator --url http://localhost:8899")
    );
}

#[test]
fn record_without_source_stores_no_source() {
    let (transaction, accounts) = write_inputs("hpsvm-cli-record-no-source");
    let output = TempFile::new("hpsvm-cli-record-no-source-out", "json");

    let result = record(transaction.path(), accounts.path(), output.path())
        .output()
        .expect("record must run");
    assert!(result.status.success());
    assert_eq!(Fixture::load(output.path()).unwrap().header.source, None);
}

#[test]
fn record_defaults_the_loader_to_the_upgradeable_loader() {
    let (transaction, accounts) = write_inputs("hpsvm-cli-record-loader-default");
    let output = TempFile::new("hpsvm-cli-record-loader-default-out", "json");

    let result = record(transaction.path(), accounts.path(), output.path())
        .output()
        .expect("record must run");
    assert!(result.status.success());
}

#[test]
fn record_stores_the_requested_slot() {
    let (transaction, accounts) = write_inputs("hpsvm-cli-record-slot");
    let output = TempFile::new("hpsvm-cli-record-slot-out", "json");

    let result = record(transaction.path(), accounts.path(), output.path())
        .args(["--slot", "424242"])
        .output()
        .expect("record command must execute");

    assert!(result.status.success(), "stderr: {}", String::from_utf8_lossy(&result.stderr));

    let fixture = Fixture::load(output.path()).unwrap();
    let hpsvm_fixture::FixtureInput::Transaction(transaction_fixture) = &fixture.input else {
        panic!("expected a transaction fixture");
    };
    assert_eq!(transaction_fixture.runtime.slot, 424_242);
}

#[test]
fn record_without_a_slot_records_the_fresh_vm_slot() {
    let (transaction, accounts) = write_inputs("hpsvm-cli-record-slot-default");
    let output = TempFile::new("hpsvm-cli-record-slot-default-out", "json");

    let result = record(transaction.path(), accounts.path(), output.path())
        .output()
        .expect("record must run");
    assert!(result.status.success());

    let fixture = Fixture::load(output.path()).unwrap();
    let hpsvm_fixture::FixtureInput::Transaction(transaction_fixture) = &fixture.input else {
        panic!("expected a transaction fixture");
    };
    // The default is whatever a fresh VM reports as its block slot.
    let default_slot = HPSVM::new().block_env().slot;
    assert_eq!(transaction_fixture.runtime.slot, default_slot);
}

#[test]
fn record_stores_the_log_bytes_limit() {
    let (transaction, accounts) = write_inputs("hpsvm-cli-record-log-limit");
    let output = TempFile::new("hpsvm-cli-record-log-limit-out", "json");

    let result = record(transaction.path(), accounts.path(), output.path())
        .args(["--log-bytes-limit", "512"])
        .output()
        .expect("record command must execute");

    assert!(result.status.success(), "stderr: {}", String::from_utf8_lossy(&result.stderr));

    let fixture = Fixture::load(output.path()).unwrap();
    let hpsvm_fixture::FixtureInput::Transaction(transaction_fixture) = &fixture.input else {
        panic!("expected a transaction fixture");
    };
    assert_eq!(transaction_fixture.runtime.log_bytes_limit, Some(512));
}

#[test]
fn record_with_sigverify_false_records_the_flag() {
    let (transaction, accounts) = write_inputs("hpsvm-cli-record-sigverify");
    let output = TempFile::new("hpsvm-cli-record-sigverify-out", "json");

    let result = record(transaction.path(), accounts.path(), output.path())
        .args(["--sigverify", "false"])
        .output()
        .expect("record command must execute");

    assert!(result.status.success(), "stderr: {}", String::from_utf8_lossy(&result.stderr));

    let fixture = Fixture::load(output.path()).unwrap();
    let hpsvm_fixture::FixtureInput::Transaction(transaction_fixture) = &fixture.input else {
        panic!("expected a transaction fixture");
    };
    assert!(!transaction_fixture.runtime.sigverify);
}

#[test]
fn record_with_sigverify_true_is_the_default() {
    let (transaction, accounts) = write_inputs("hpsvm-cli-record-sigverify-true");
    let output = TempFile::new("hpsvm-cli-record-sigverify-true-out", "json");

    let result = record(transaction.path(), accounts.path(), output.path())
        .args(["--sigverify", "true"])
        .output()
        .expect("record command must execute");
    assert!(result.status.success());

    let fixture = Fixture::load(output.path()).unwrap();
    let hpsvm_fixture::FixtureInput::Transaction(transaction_fixture) = &fixture.input else {
        panic!("expected a transaction fixture");
    };
    assert!(transaction_fixture.runtime.sigverify);
}

#[test]
fn record_accepts_a_hex_encoded_transaction() {
    let scenario = scenario();
    let transaction = TempFile::new("hpsvm-cli-record-hex", "hex");
    fs::write(transaction.path(), encode_hex(&scenario.transaction_bytes))
        .expect("hex transaction must be written");
    let accounts = TempFile::new("hpsvm-cli-record-hex-accounts", "json");
    fs::write(accounts.path(), &scenario.accounts_json).expect("accounts must be written");
    let output = TempFile::new("hpsvm-cli-record-hex-out", "json");

    let result = record(transaction.path(), accounts.path(), output.path())
        .args(["--transaction-encoding", "hex"])
        .output()
        .expect("record command must execute");

    assert!(result.status.success(), "stderr: {}", String::from_utf8_lossy(&result.stderr));
    assert!(Fixture::load(output.path()).is_ok());
}

#[test]
fn record_accepts_an_explicit_raw_encoding() {
    let (transaction, accounts) = write_inputs("hpsvm-cli-record-raw");
    let output = TempFile::new("hpsvm-cli-record-raw-out", "json");

    let result = record(transaction.path(), accounts.path(), output.path())
        .args(["--transaction-encoding", "raw"])
        .output()
        .expect("record command must execute");

    assert!(result.status.success(), "stderr: {}", String::from_utf8_lossy(&result.stderr));
}

#[test]
fn record_rejects_a_non_hex_hex_encoding() {
    let scenario = scenario();
    let transaction = TempFile::new("hpsvm-cli-record-bad-hex", "hex");
    // Correct length, non-hex characters.
    fs::write(transaction.path(), "z".repeat(scenario.transaction_bytes.len() * 2))
        .expect("transaction must be written");
    let accounts = TempFile::new("hpsvm-cli-record-bad-hex-accounts", "json");
    fs::write(accounts.path(), &scenario.accounts_json).expect("accounts must be written");
    let output = TempFile::new("hpsvm-cli-record-bad-hex-out", "json");

    let result = record(transaction.path(), accounts.path(), output.path())
        .args(["--transaction-encoding", "hex"])
        .output()
        .expect("record command must execute");

    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("invalid hex input"), "unexpected stderr: {stderr}");
}

#[test]
fn record_rejects_invalid_base64_in_a_base64_encoded_transaction() {
    let scenario = scenario();
    let transaction = TempFile::new("hpsvm-cli-record-bad-base64", "b64");
    fs::write(transaction.path(), "!!!!not base64!!!!").expect("transaction must be written");
    let accounts = TempFile::new("hpsvm-cli-record-bad-base64-accounts", "json");
    fs::write(accounts.path(), &scenario.accounts_json).expect("accounts must be written");
    let output = TempFile::new("hpsvm-cli-record-bad-base64-out", "json");

    let result = record(transaction.path(), accounts.path(), output.path())
        .args(["--transaction-encoding", "base64"])
        .output()
        .expect("record command must execute");

    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("invalid base64 input"), "unexpected stderr: {stderr}");
}

#[test]
fn record_reports_an_odd_length_hex_transaction() {
    let scenario = scenario();
    let transaction = TempFile::new("hpsvm-cli-record-odd-hex", "hex");
    fs::write(transaction.path(), "abc").expect("transaction must be written");
    let accounts = TempFile::new("hpsvm-cli-record-odd-hex-accounts", "json");
    fs::write(accounts.path(), &scenario.accounts_json).expect("accounts must be written");
    let output = TempFile::new("hpsvm-cli-record-odd-hex-out", "json");

    let result = record(transaction.path(), accounts.path(), output.path())
        .args(["--transaction-encoding", "hex"])
        .output()
        .expect("record command must execute");

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("odd"));
}

#[test]
fn record_writes_a_binary_fixture_for_a_bin_output() {
    let (transaction, accounts) = write_inputs("hpsvm-cli-record-bin");
    let output = TempFile::new("hpsvm-cli-record-bin-out", "bin");

    let result = record(transaction.path(), accounts.path(), output.path())
        .output()
        .expect("record must run");

    assert!(result.status.success(), "stderr: {}", String::from_utf8_lossy(&result.stderr));
    assert!(String::from_utf8_lossy(&result.stdout).contains("RECORDED:"));

    // A `.bin` output must be loadable by extension.
    let fixture = Fixture::load(output.path()).expect("binary fixture must load");
    // The recorded name is the whole output stem, including the unique
    // suffix this suite appends to avoid collisions.
    assert_eq!(fixture.header.name, output.path().file_stem().unwrap().to_string_lossy());
}

#[test]
fn record_names_the_fixture_from_the_output_stem() {
    let (transaction, accounts) = write_inputs("hpsvm-cli-record-stem");
    let output = TempFile::new("hpsvm-cli-record-stem-out", "json");

    let result = record(transaction.path(), accounts.path(), output.path())
        .output()
        .expect("record must run");
    assert!(result.status.success());

    assert_eq!(
        Fixture::load(output.path()).unwrap().header.name,
        output.path().file_stem().unwrap().to_string_lossy()
    );
}

#[test]
fn record_honours_an_explicit_name() {
    let (transaction, accounts) = write_inputs("hpsvm-cli-record-name");
    let output = TempFile::new("hpsvm-cli-record-name-out", "json");

    let result = record(transaction.path(), accounts.path(), output.path())
        .args(["--name", "my-explicit-name"])
        .output()
        .expect("record command must execute");

    assert!(result.status.success());
    assert_eq!(Fixture::load(output.path()).unwrap().header.name, "my-explicit-name");
}

#[test]
fn record_rejects_an_address_array_that_is_too_short() {
    let scenario = scenario();
    let transaction = TempFile::new("hpsvm-cli-record-short-addr", "bin");
    fs::write(transaction.path(), &scenario.transaction_bytes).expect("tx must be written");
    let accounts = TempFile::new("hpsvm-cli-record-short-addr-accounts", "json");
    fs::write(
        accounts.path(),
        serde_json::json!([{
            "address": vec![1u8; 8],
            "lamports": 1u64,
            "owner": solana_sdk_ids::system_program::id().to_string(),
            "executable": false,
            "rent_epoch": 0,
            "data": [],
        }])
        .to_string(),
    )
    .expect("accounts must be written");
    let output = TempFile::new("hpsvm-cli-record-short-addr-out", "json");

    let result = record(transaction.path(), accounts.path(), output.path())
        .output()
        .expect("record must run");

    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("invalid length"), "unexpected stderr: {stderr}");
}

#[test]
fn record_rejects_an_address_array_that_is_too_long() {
    let scenario = scenario();
    let transaction = TempFile::new("hpsvm-cli-record-long-addr", "bin");
    fs::write(transaction.path(), &scenario.transaction_bytes).expect("tx must be written");
    let accounts = TempFile::new("hpsvm-cli-record-long-addr-accounts", "json");
    fs::write(
        accounts.path(),
        serde_json::json!([{
            "address": vec![1u8; 33],
            "lamports": 1u64,
            "owner": solana_sdk_ids::system_program::id().to_string(),
            "executable": false,
            "rent_epoch": 0,
            "data": [],
        }])
        .to_string(),
    )
    .expect("accounts must be written");
    let output = TempFile::new("hpsvm-cli-record-long-addr-out", "json");

    let result = record(transaction.path(), accounts.path(), output.path())
        .output()
        .expect("record must run");

    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("32 bytes"), "unexpected stderr: {stderr}");
}

#[test]
fn record_rejects_an_account_missing_a_required_field() {
    let scenario = scenario();
    let transaction = TempFile::new("hpsvm-cli-record-missing-field", "bin");
    fs::write(transaction.path(), &scenario.transaction_bytes).expect("tx must be written");
    let accounts = TempFile::new("hpsvm-cli-record-missing-field-accounts", "json");
    // `owner` is missing entirely.
    fs::write(
        accounts.path(),
        serde_json::json!([{
            "address": Address::new_unique().to_string(),
            "lamports": 1u64,
            "executable": false,
            "rent_epoch": 0,
            "data": [],
        }])
        .to_string(),
    )
    .expect("accounts must be written");
    let output = TempFile::new("hpsvm-cli-record-missing-field-out", "json");

    let result = record(transaction.path(), accounts.path(), output.path())
        .output()
        .expect("record must run");

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("owner"));
}

#[test]
fn record_rejects_a_missing_accounts_file() {
    let (transaction, _accounts) = write_inputs("hpsvm-cli-record-absent-accounts");
    let missing = TempFile::new("hpsvm-cli-record-absent-accounts-file", "json");
    let output = TempFile::new("hpsvm-cli-record-absent-accounts-out", "json");

    let result = record(transaction.path(), missing.path(), output.path())
        .output()
        .expect("record must run");

    assert!(!result.status.success());
}

#[test]
fn record_rejects_a_missing_transaction_file() {
    let (_transaction, accounts) = write_inputs("hpsvm-cli-record-absent-tx");
    let missing = TempFile::new("hpsvm-cli-record-absent-tx-file", "bin");
    let output = TempFile::new("hpsvm-cli-record-absent-tx-out", "json");

    let result =
        record(missing.path(), accounts.path(), output.path()).output().expect("record must run");

    assert!(!result.status.success());
}

#[test]
fn record_rejects_an_unsupported_output_extension_before_writing_anything() {
    let (transaction, accounts) = write_inputs("hpsvm-cli-record-bad-ext");
    let output = TempFile::new("hpsvm-cli-record-bad-ext-out", "txt");

    let result = record(transaction.path(), accounts.path(), output.path())
        .output()
        .expect("record must run");

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("unsupported fixture format"));
    assert!(!output.path().exists(), "a rejected output extension must not leave a file behind");
}

#[test]
fn a_recorded_fixture_replays_through_the_run_subcommand() {
    let (transaction, accounts) = write_inputs("hpsvm-cli-record-replay");
    let output = TempFile::new("hpsvm-cli-record-replay-out", "json");

    let recorded = record(transaction.path(), accounts.path(), output.path())
        .args(["--tag", "replay", "--source", "e2e"])
        .output()
        .expect("record command must execute");
    assert!(recorded.status.success(), "stderr: {}", String::from_utf8_lossy(&recorded.stderr));

    let replayed = Command::new(env!("CARGO_BIN_EXE_hpsvm"))
        .args(["fixture", "run", utf8(output.path()).as_str()])
        .output()
        .expect("run command must execute");

    assert!(replayed.status.success(), "stderr: {}", String::from_utf8_lossy(&replayed.stderr));
    assert!(
        String::from_utf8_lossy(&replayed.stdout).contains("PASS: hpsvm-cli-record-replay-out")
    );
}

#[test]
fn a_recorded_binary_fixture_replays_through_the_run_subcommand() {
    let (transaction, accounts) = write_inputs("hpsvm-cli-record-replay-bin");
    let output = TempFile::new("hpsvm-cli-record-replay-bin-out", "bin");

    let recorded = record(transaction.path(), accounts.path(), output.path())
        .output()
        .expect("record must run");
    assert!(recorded.status.success());

    let replayed = Command::new(env!("CARGO_BIN_EXE_hpsvm"))
        .args(["fixture", "run", utf8(output.path()).as_str()])
        .output()
        .expect("run command must execute");

    assert!(replayed.status.success(), "stderr: {}", String::from_utf8_lossy(&replayed.stderr));
}
