#![allow(missing_docs)]

//! End-to-end coverage for the `fixture compare`, `fixture inspect` and
//! `cu report` flags.
//!
//! The existing suites cover `compare` with identical inputs and `cu report`
//! with a baseline directory. This file covers:
//!
//! - `--config` (a JSON compare list) and each of its error paths,
//! - `--ignore-compute-units`,
//! - `--baseline-program` / `--candidate-program` for a genuine differential run,
//! - the `FAIL` / exit-code-1 path,
//! - `cu report --must-pass`, `--program`, and its error paths,
//! - and `fixture inspect` over a directory, a missing file, and a bad extension.
//!
//! Two behaviours of the CLI shape these tests:
//!
//! - `compare` diffs a *baseline run* against a *candidate run* of the same fixture, so it only
//!   fails when the two program maps produce different executions. A fixture whose recorded
//!   baseline is unreachable still passes.
//! - The binary is built with `hotpath::main`, which appends a profiler report to stdout after the
//!   command output. Anything that parses stdout must read only the first JSON document.

use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

use hpsvm::HPSVM;
use hpsvm_fixture::{
    AccountSnapshot, CaptureBuilder, Compare, ExecutionSnapshot, Fixture, FixtureFormat,
    ProgramBinding, RuntimeFixtureConfig,
};
use solana_address::Address;
use solana_instruction::AccountMeta;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::versioned::VersionedTransaction;

/// The upgradeable loader, which every vendored ELF in this workspace targets.
const UPGRADEABLE_LOADER: &str = "BPFLoaderUpgradeab1e11111111111111111111111";

/// Two builds of the same memo program, so a differential run produces
/// genuinely different compute unit counts.
const MEMO_V1: &str = "../../crates/hpsvm/elf/spl_memo-1.0.0.so";
const MEMO_V3: &str = "../../crates/hpsvm/elf/spl_memo-3.0.0.so";

/// Resolves a path relative to this crate's manifest directory.
fn workspace_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// A temp path that is removed on drop.
struct TempPath(PathBuf);

impl TempPath {
    fn new(stem: &str, extension: &str) -> Self {
        let unique = std::env::temp_dir().join(format!(
            "{stem}-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time must be after unix epoch")
                .as_nanos(),
            Address::new_unique()
        ));
        Self(unique.with_extension(extension))
    }

    fn new_dir(stem: &str) -> Self {
        let path = Self(std::env::temp_dir().join(format!(
            "{stem}-{}-{}",
            std::process::id(),
            Address::new_unique()
        )));
        std::fs::create_dir_all(&path.0).expect("temp directory must be created");
        path
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        if self.0.is_dir() {
            std::fs::remove_dir_all(&self.0).ok();
        } else {
            std::fs::remove_file(&self.0).ok();
        }
    }
}

/// Materialises a path as an owned string.
///
/// `Command::args` takes `AsRef<OsStr>` items, so mixing `&str` with a
/// `Cow<str>` in one array leaves the element type ambiguous. Every argument
/// here is therefore an explicit `&str`.
fn utf8(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Runs the CLI with the supplied arguments.
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hpsvm")).args(args).output().expect("command must execute")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Reads only the first JSON document from stdout, ignoring the trailing
/// `hotpath` profiler report the binary appends on exit.
fn first_json(text: &str) -> serde_json::Value {
    serde_json::Deserializer::from_str(text)
        .into_iter::<serde_json::Value>()
        .next()
        .expect("stdout must contain a json document")
        .expect("the first json document must parse")
}

/// Builds a captured fixture that invokes the memo program once, so a
/// differential run over two program builds actually differs.
///
/// The baseline is captured on a VM that already has `baseline_elf` deployed at
/// the memo id, otherwise the recorded baseline would be a pre-invocation
/// failure that every replay reproduces identically.
fn memo_fixture(name: &str, baseline_elf: &Path) -> (Fixture, Address) {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let memo_id = Address::new_unique();
    svm.add_program_from_file(memo_id, baseline_elf)
        .expect("the vendored memo program must deploy");
    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    svm.airdrop(&memo_id, 1).unwrap();

    // Invoke the memo program directly; memo v1 accepts any instruction data.
    let invoke = solana_instruction::Instruction {
        program_id: memo_id,
        accounts: vec![AccountMeta::new(payer.pubkey(), true)],
        data: Vec::new(),
    };
    let tx = VersionedTransaction::from(solana_transaction::Transaction::new(
        &[&payer],
        Message::new(&[invoke], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    ));
    let baseline = ExecutionSnapshot::from_outcome(&svm.transact(tx.clone()));
    let pre_accounts = vec![
        AccountSnapshot::from_readable(payer.pubkey(), &svm.get_account(&payer.pubkey()).unwrap()),
        AccountSnapshot::from_readable(memo_id, &svm.get_account(&memo_id).unwrap()),
    ];

    let loader: Address = UPGRADEABLE_LOADER.parse().expect("the loader id must be base58");
    let fixture = CaptureBuilder::new(name)
        .runtime(RuntimeFixtureConfig::new(svm.block_env().slot, None, true, false))
        .programs(vec![ProgramBinding::new(memo_id, loader, None)])
        .pre_accounts(pre_accounts)
        .baseline(baseline)
        .compares(Compare::everything())
        .capture_transaction(&tx)
        .expect("fixture capture must succeed");

    (fixture, memo_id)
}

/// Builds a plain transfer fixture that binds no programs.
fn transfer_fixture(name: &str) -> Fixture {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();
    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    svm.airdrop(&recipient, 1).unwrap();

    let tx = VersionedTransaction::from(solana_transaction::Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    ));
    let baseline = ExecutionSnapshot::from_outcome(&svm.transact(tx.clone()));

    CaptureBuilder::new(name)
        .runtime(RuntimeFixtureConfig::new(svm.block_env().slot, None, true, false))
        .pre_accounts(vec![
            AccountSnapshot::from_readable(
                payer.pubkey(),
                &svm.get_account(&payer.pubkey()).unwrap(),
            ),
            AccountSnapshot::from_readable(recipient, &svm.get_account(&recipient).unwrap()),
        ])
        .baseline(baseline)
        .compares(Compare::everything())
        .capture_transaction(&tx)
        .expect("fixture capture must succeed")
}

/// Writes a fixture with an unreachable recorded baseline by corrupting the
/// fee, which makes `cu report` (but not `compare`) fail.
fn write_fixture_with_unreachable_baseline(stem: &str, name: &str) -> TempPath {
    let path = TempPath::new(stem, "json");
    let mut fixture = transfer_fixture(name);
    fixture.expectations.baseline.fee = u64::MAX;
    fixture.save(path.path(), FixtureFormat::Json).expect("fixture save must succeed");
    path
}

fn write_fixture(stem: &str, name: &str, format: FixtureFormat) -> TempPath {
    let path = TempPath::new(
        stem,
        match format {
            FixtureFormat::Json => "json",
            FixtureFormat::Binary => "bin",
        },
    );
    let fixture = transfer_fixture(name);
    fixture.save(path.path(), format).expect("fixture save must succeed");
    path
}

fn write_fixture_at(path: &Path, name: &str, format: FixtureFormat) {
    let fixture = transfer_fixture(name);
    fixture.save(path, format).expect("fixture save must succeed");
}

/// Writes a `--config` file and returns its path alongside the path string.
fn write_config(stem: &str, extension: &str, body: &str) -> (TempPath, String) {
    let path = TempPath::new(stem, extension);
    std::fs::write(path.path(), body).expect("config must be written");
    let as_str = utf8(path.path());
    (path, as_str)
}

// ---------------------------------------------------------------------
// fixture compare: the happy path
// ---------------------------------------------------------------------

#[test]
fn fixture_compare_succeeds_against_itself() {
    let path = write_fixture("hpsvm-cli-compare-self", "self", FixtureFormat::Json);
    let fixture = utf8(path.path());

    let output = run(&["fixture", "compare", fixture.as_str()]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("PASS: self"));
}

#[test]
fn fixture_compare_of_a_binary_fixture_succeeds() {
    let path = write_fixture("hpsvm-cli-compare-bin", "bin-case", FixtureFormat::Binary);
    let fixture = utf8(path.path());

    let output = run(&["fixture", "compare", fixture.as_str()]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("PASS: bin-case"));
}

/// A fixture that binds a program cannot be replayed without supplying that
/// program's ELF, in either map.
#[test]
fn fixture_compare_requires_an_elf_for_every_bound_program() {
    let (fixture, memo_id) = memo_fixture("unbound", &workspace_path(MEMO_V1));
    let path = TempPath::new("hpsvm-cli-compare-unbound", "json");
    fixture.save(path.path(), FixtureFormat::Json).unwrap();
    let fixture_path = utf8(path.path());

    let output = run(&["fixture", "compare", fixture_path.as_str()]);

    assert!(!output.status.success());
    let message = stderr(&output);
    assert!(message.contains("missing program ELF"), "stderr: {message}");
    assert!(message.contains(&memo_id.to_string()), "stderr: {message}");
}

// ---------------------------------------------------------------------
// fixture compare: differential programs
// ---------------------------------------------------------------------

#[test]
fn fixture_compare_accepts_matching_baseline_and_candidate_programs() {
    let (fixture, memo_id) = memo_fixture("same-elf", &workspace_path(MEMO_V1));
    let path = TempPath::new("hpsvm-cli-compare-same-elf", "json");
    fixture.save(path.path(), FixtureFormat::Json).unwrap();
    let fixture_path = utf8(path.path());
    let elf = utf8(&workspace_path(MEMO_V1));
    let mapping = format!("{memo_id}={elf}");

    let output = run(&[
        "fixture",
        "compare",
        fixture_path.as_str(),
        "--baseline-program",
        mapping.as_str(),
        "--candidate-program",
        mapping.as_str(),
    ]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("PASS: same-elf"));
}

/// Two builds of the same program burn different compute units, so the
/// differential comparison must report a difference and exit non-zero.
#[test]
fn fixture_compare_of_two_program_builds_fails_with_a_nonzero_exit_code() {
    let (fixture, memo_id) = memo_fixture("drift", &workspace_path(MEMO_V1));
    let path = TempPath::new("hpsvm-cli-compare-drift", "json");
    fixture.save(path.path(), FixtureFormat::Json).unwrap();
    let fixture_path = utf8(path.path());
    let baseline = format!("{memo_id}={}", utf8(&workspace_path(MEMO_V1)));
    let candidate = format!("{memo_id}={}", utf8(&workspace_path(MEMO_V3)));

    let output = run(&[
        "fixture",
        "compare",
        fixture_path.as_str(),
        "--baseline-program",
        baseline.as_str(),
        "--candidate-program",
        candidate.as_str(),
    ]);

    assert!(!output.status.success(), "a differing run must exit non-zero");
    assert!(stderr(&output).contains("FAIL: drift"), "stderr: {}", stderr(&output));
}

// ---------------------------------------------------------------------
// fixture compare: --config
// ---------------------------------------------------------------------

#[test]
fn fixture_compare_with_a_config_still_passes() {
    let path = write_fixture("hpsvm-cli-compare-config", "case", FixtureFormat::Json);
    let fixture = utf8(path.path());
    let (_config, config) = write_config(
        "hpsvm-cli-compare-config-cfg",
        "json",
        r#"{"compares":["Status","Included","Fee"]}"#,
    );

    let output = run(&["fixture", "compare", fixture.as_str(), "--config", config.as_str()]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("PASS: case"));
}

/// An empty compare list makes the comparison vacuously true, so even two
/// differing program builds pass.
#[test]
fn fixture_compare_with_an_empty_config_list_always_passes() {
    let (fixture, memo_id) = memo_fixture("empty-config", &workspace_path(MEMO_V1));
    let path = TempPath::new("hpsvm-cli-compare-empty-config", "json");
    fixture.save(path.path(), FixtureFormat::Json).unwrap();
    let fixture_path = utf8(path.path());
    let baseline = format!("{memo_id}={}", utf8(&workspace_path(MEMO_V1)));
    let candidate = format!("{memo_id}={}", utf8(&workspace_path(MEMO_V3)));
    let (_config, config) =
        write_config("hpsvm-cli-compare-empty-config-cfg", "json", r#"{"compares":[]}"#);

    let output = run(&[
        "fixture",
        "compare",
        fixture_path.as_str(),
        "--config",
        config.as_str(),
        "--baseline-program",
        baseline.as_str(),
        "--candidate-program",
        candidate.as_str(),
    ]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
}

/// `--ignore-compute-units` strips `ComputeUnits` from a `--config` list too.
#[test]
fn fixture_compare_ignore_compute_units_applies_to_a_config_list() {
    let (fixture, memo_id) = memo_fixture("config-cu", &workspace_path(MEMO_V1));
    let path = TempPath::new("hpsvm-cli-compare-config-cu", "json");
    fixture.save(path.path(), FixtureFormat::Json).unwrap();
    let fixture_path = utf8(path.path());
    let baseline = format!("{memo_id}={}", utf8(&workspace_path(MEMO_V1)));
    let candidate = format!("{memo_id}={}", utf8(&workspace_path(MEMO_V3)));
    let (_config, config) =
        write_config("hpsvm-cli-compare-config-cu-cfg", "json", r#"{"compares":["ComputeUnits"]}"#);

    let without_flag = run(&[
        "fixture",
        "compare",
        fixture_path.as_str(),
        "--config",
        config.as_str(),
        "--baseline-program",
        baseline.as_str(),
        "--candidate-program",
        candidate.as_str(),
    ]);
    assert!(!without_flag.status.success(), "compute units alone must fail");

    let with_flag = run(&[
        "fixture",
        "compare",
        fixture_path.as_str(),
        "--config",
        config.as_str(),
        "--baseline-program",
        baseline.as_str(),
        "--candidate-program",
        candidate.as_str(),
        "--ignore-compute-units",
    ]);
    assert!(
        with_flag.status.success(),
        "the stripped list must leave nothing to compare; stderr: {}",
        stderr(&with_flag)
    );
}

#[test]
fn fixture_compare_with_a_malformed_config_fails_with_a_parse_error() {
    let path = write_fixture("hpsvm-cli-compare-bad-config", "case", FixtureFormat::Json);
    let fixture = utf8(path.path());
    let (_config, config) = write_config("hpsvm-cli-compare-bad-config-cfg", "json", "{ not json");

    let output = run(&["fixture", "compare", fixture.as_str(), "--config", config.as_str()]);

    assert!(!output.status.success());
    assert!(stderr(&output).contains("failed to parse"), "unexpected stderr: {}", stderr(&output));
}

#[test]
fn fixture_compare_with_a_config_containing_an_unknown_variant_fails() {
    let path = write_fixture("hpsvm-cli-compare-unknown-variant", "case", FixtureFormat::Json);
    let fixture = utf8(path.path());
    let (_config, config) = write_config(
        "hpsvm-cli-compare-unknown-variant-cfg",
        "json",
        r#"{"compares":["Nonsense"]}"#,
    );

    let output = run(&["fixture", "compare", fixture.as_str(), "--config", config.as_str()]);

    assert!(!output.status.success());
    assert!(stderr(&output).contains("failed to parse"), "unexpected stderr: {}", stderr(&output));
}

/// A config that is valid JSON but missing the required `compares` key.
#[test]
fn fixture_compare_with_a_config_missing_the_compares_key_fails() {
    let path = write_fixture("hpsvm-cli-compare-no-key", "case", FixtureFormat::Json);
    let fixture = utf8(path.path());
    let (_config, config) = write_config("hpsvm-cli-compare-no-key-cfg", "json", r#"{"other":[]}"#);

    let output = run(&["fixture", "compare", fixture.as_str(), "--config", config.as_str()]);

    assert!(!output.status.success());
    assert!(stderr(&output).contains("failed to parse"), "unexpected stderr: {}", stderr(&output));
}

#[test]
fn fixture_compare_with_a_missing_config_fails_with_an_io_error() {
    let path = write_fixture("hpsvm-cli-compare-missing-config", "case", FixtureFormat::Json);
    let fixture = utf8(path.path());
    let absent = utf8(
        &std::env::temp_dir()
            .join(format!("hpsvm-cli-absent-config-{}.json", Address::new_unique())),
    );

    let output = run(&["fixture", "compare", fixture.as_str(), "--config", absent.as_str()]);

    assert!(!output.status.success());
    assert!(!stderr(&output).contains("failed to parse"), "the read must fail first");
}

#[test]
fn fixture_compare_rejects_an_unsupported_config_extension() {
    let path = write_fixture("hpsvm-cli-compare-cfg-ext", "case", FixtureFormat::Json);
    let fixture = utf8(path.path());
    let (_config, config) = write_config("hpsvm-cli-compare-cfg-ext-cfg", "toml", "compares = []");

    let output = run(&["fixture", "compare", fixture.as_str(), "--config", config.as_str()]);

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("unsupported config format"),
        "unexpected stderr: {}",
        stderr(&output)
    );
}

// ---------------------------------------------------------------------
// fixture compare: program mapping errors
// ---------------------------------------------------------------------

#[test]
fn fixture_compare_rejects_an_invalid_baseline_program_mapping() {
    let path = write_fixture("hpsvm-cli-compare-bad-baseline", "case", FixtureFormat::Json);
    let fixture = utf8(path.path());

    let output =
        run(&["fixture", "compare", fixture.as_str(), "--baseline-program", "not-a-mapping"]);

    assert!(!output.status.success());
    assert!(stderr(&output).contains("expected <program-id>=<path>"));
}

#[test]
fn fixture_compare_rejects_an_invalid_candidate_program_mapping() {
    let path = write_fixture("hpsvm-cli-compare-bad-candidate", "case", FixtureFormat::Json);
    let fixture = utf8(path.path());

    let output =
        run(&["fixture", "compare", fixture.as_str(), "--candidate-program", "not-a-mapping"]);

    assert!(!output.status.success());
    assert!(stderr(&output).contains("expected <program-id>=<path>"));
}

#[test]
fn fixture_compare_rejects_an_unparseable_program_id() {
    let path = write_fixture("hpsvm-cli-compare-bad-id", "case", FixtureFormat::Json);
    let fixture = utf8(path.path());
    let elf = utf8(&workspace_path(MEMO_V1));
    let bad_mapping = format!("not-an-address={elf}");

    let output =
        run(&["fixture", "compare", fixture.as_str(), "--baseline-program", bad_mapping.as_str()]);

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("invalid program id"),
        "unexpected stderr: {}",
        stderr(&output)
    );
}

#[test]
fn fixture_compare_rejects_an_unreadable_program_elf() {
    let path = write_fixture("hpsvm-cli-compare-missing-elf", "case", FixtureFormat::Json);
    let fixture = utf8(path.path());
    let mapping = format!("{}=/definitely/not/a/real/file.so", Address::new_unique());

    let output =
        run(&["fixture", "compare", fixture.as_str(), "--baseline-program", mapping.as_str()]);

    assert!(!output.status.success());
}

// ---------------------------------------------------------------------
// fixture compare: directories
// ---------------------------------------------------------------------

#[test]
fn fixture_compare_over_a_directory_processes_every_fixture_in_order() {
    let dir = TempPath::new_dir("hpsvm-cli-compare-dir");
    write_fixture_at(&dir.path().join("a.json"), "dir-a", FixtureFormat::Json);
    write_fixture_at(&dir.path().join("b.bin"), "dir-b", FixtureFormat::Binary);
    std::fs::write(dir.path().join("notes.txt"), "ignored").expect("notes must be written");
    let dir_str = utf8(dir.path());

    let output = run(&["fixture", "compare", dir_str.as_str()]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    let first = text.find("PASS: dir-a").expect("first fixture should pass");
    let second = text.find("PASS: dir-b").expect("second fixture should pass");
    assert!(first < second, "fixtures must run in sorted path order: {text}");
}

/// The comparison exits at the first difference, so the remaining fixtures in
/// the directory are never reported.
#[test]
fn fixture_compare_over_a_directory_stops_at_the_first_failure() {
    let dir = TempPath::new_dir("hpsvm-cli-compare-dir-fail");
    write_fixture_at(&dir.path().join("a.json"), "dir-a", FixtureFormat::Json);
    let (failing, memo_id) = memo_fixture("dir-b", &workspace_path(MEMO_V1));
    failing.save(dir.path().join("b.json"), FixtureFormat::Json).unwrap();
    write_fixture_at(&dir.path().join("c.json"), "dir-c", FixtureFormat::Json);
    let dir_str = utf8(dir.path());
    let baseline = format!("{memo_id}={}", utf8(&workspace_path(MEMO_V1)));
    let candidate = format!("{memo_id}={}", utf8(&workspace_path(MEMO_V3)));

    let output = run(&[
        "fixture",
        "compare",
        dir_str.as_str(),
        "--baseline-program",
        baseline.as_str(),
        "--candidate-program",
        candidate.as_str(),
    ]);

    assert!(!output.status.success());
    assert!(stderr(&output).contains("FAIL: dir-b"), "stderr: {}", stderr(&output));
    // `dir-a` ran before the failure; `dir-c` never ran.
    assert!(stdout(&output).contains("PASS: dir-a"));
    assert!(!stdout(&output).contains("dir-c"), "comparison must stop at the first failure");
}

#[test]
fn fixture_compare_rejects_an_empty_directory() {
    let dir = TempPath::new_dir("hpsvm-cli-compare-empty");
    std::fs::write(dir.path().join("notes.txt"), "ignored").expect("notes must be written");
    let dir_str = utf8(dir.path());

    let output = run(&["fixture", "compare", dir_str.as_str()]);

    assert!(!output.status.success());
    assert!(stderr(&output).contains("no fixture files found"), "stderr: {}", stderr(&output));
}

// ---------------------------------------------------------------------
// fixture inspect
// ---------------------------------------------------------------------

#[test]
fn fixture_inspect_rejects_a_directory() {
    let dir = TempPath::new_dir("hpsvm-cli-inspect-dir");
    write_fixture_at(&dir.path().join("a.json"), "a", FixtureFormat::Json);
    let dir_str = utf8(dir.path());

    let output = run(&["fixture", "inspect", dir_str.as_str()]);

    assert!(!output.status.success(), "inspect only accepts a single fixture file");
}

#[test]
fn fixture_inspect_rejects_a_missing_file() {
    let missing = TempPath::new("hpsvm-cli-inspect-absent", "json");
    let missing_str = utf8(missing.path());

    let output = run(&["fixture", "inspect", missing_str.as_str()]);

    assert!(!output.status.success());
}

#[test]
fn fixture_inspect_rejects_an_unsupported_extension() {
    let path = TempPath::new("hpsvm-cli-inspect-ext", "txt");
    std::fs::write(path.path(), "not a fixture").expect("file must be written");
    let path_str = utf8(path.path());

    let output = run(&["fixture", "inspect", path_str.as_str()]);

    assert!(!output.status.success());
    assert!(stderr(&output).contains("unsupported fixture format"), "stderr: {}", stderr(&output));
}

#[test]
fn fixture_inspect_emits_the_whole_fixture_as_json() {
    let path = write_fixture("hpsvm-cli-inspect-json", "inspect-me", FixtureFormat::Json);
    let path_str = utf8(path.path());

    let output = run(&["fixture", "inspect", path_str.as_str()]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let parsed = first_json(&stdout(&output));
    assert_eq!(parsed["header"]["name"], "inspect-me");
    assert_eq!(parsed["header"]["kind"], "Transaction");
    assert!(parsed["expectations"]["baseline"].is_object());
}

// ---------------------------------------------------------------------
// cu report
// ---------------------------------------------------------------------

#[test]
fn cu_report_writes_a_report_and_passes() {
    let path = write_fixture("hpsvm-cli-cu-report", "cu-case", FixtureFormat::Json);
    let fixture = utf8(path.path());
    let output_dir = TempPath::new_dir("hpsvm-cli-cu-report-out");
    let out = utf8(output_dir.path());

    let output = run(&["cu", "report", fixture.as_str(), "--output-dir", out.as_str()]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("PASS: cu-case"));
    assert!(output_dir.path().join("cu-report.md").exists());
    assert!(output_dir.path().join("cu-report.baseline.json").exists());
}

#[test]
fn cu_report_requires_output_dir() {
    let path = write_fixture("hpsvm-cli-cu-no-out", "cu-case", FixtureFormat::Json);
    let fixture = utf8(path.path());

    let output = run(&["cu", "report", fixture.as_str()]);

    assert!(!output.status.success(), "--output-dir is required");
}

/// `cu report` compares against the fixture's *recorded* baseline, so a fee the
/// replay can never reproduce is reported as a failure.
#[test]
fn cu_report_with_an_unreachable_baseline_fails() {
    let path = write_fixture_with_unreachable_baseline("hpsvm-cli-cu-unreachable", "cu-fail");
    let fixture = utf8(path.path());
    let output_dir = TempPath::new_dir("hpsvm-cli-cu-unreachable-out");
    let out = utf8(output_dir.path());

    let output = run(&["cu", "report", fixture.as_str(), "--output-dir", out.as_str()]);

    assert!(!output.status.success(), "an unreachable expectation must exit non-zero");
    assert!(stderr(&output).contains("FAIL: cu-fail"), "stderr: {}", stderr(&output));
    assert!(stderr(&output).contains("Fee"), "the fee is the drifting field: {}", stderr(&output));
    // The report is still written so the failure can be inspected.
    assert!(output_dir.path().join("cu-report.md").exists());
}

#[test]
fn cu_report_must_pass_turns_a_failing_fixture_into_an_error() {
    let path = write_fixture_with_unreachable_baseline("hpsvm-cli-cu-must-pass", "cu-fail");
    let fixture = utf8(path.path());
    let output_dir = TempPath::new_dir("hpsvm-cli-cu-must-pass-out");
    let out = utf8(output_dir.path());

    let output =
        run(&["cu", "report", fixture.as_str(), "--output-dir", out.as_str(), "--must-pass"]);

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("did not satisfy its expectations"),
        "unexpected stderr: {}",
        stderr(&output)
    );
}

#[test]
fn cu_report_must_pass_accepts_a_passing_fixture() {
    let path = write_fixture("hpsvm-cli-cu-must-pass-ok", "cu-ok", FixtureFormat::Json);
    let fixture = utf8(path.path());
    let output_dir = TempPath::new_dir("hpsvm-cli-cu-must-pass-ok-out");
    let out = utf8(output_dir.path());

    let output =
        run(&["cu", "report", fixture.as_str(), "--output-dir", out.as_str(), "--must-pass"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
}

#[test]
fn cu_report_rejects_a_missing_fixture() {
    let missing = TempPath::new("hpsvm-cli-cu-absent", "json");
    let missing_str = utf8(missing.path());
    let output_dir = TempPath::new_dir("hpsvm-cli-cu-absent-out");
    let out = utf8(output_dir.path());

    let output = run(&["cu", "report", missing_str.as_str(), "--output-dir", out.as_str()]);

    assert!(!output.status.success());
}

#[test]
fn cu_report_rejects_a_directory_as_input() {
    let dir = TempPath::new_dir("hpsvm-cli-cu-dir-input");
    let dir_str = utf8(dir.path());
    let output_dir = TempPath::new_dir("hpsvm-cli-cu-dir-input-out");
    let out = utf8(output_dir.path());

    let output = run(&["cu", "report", dir_str.as_str(), "--output-dir", out.as_str()]);

    assert!(!output.status.success(), "cu report takes a single fixture file, not a directory");
}

#[test]
fn cu_report_rejects_an_invalid_program_mapping() {
    let path = write_fixture("hpsvm-cli-cu-bad-program", "cu-case", FixtureFormat::Json);
    let fixture = utf8(path.path());
    let output_dir = TempPath::new_dir("hpsvm-cli-cu-bad-program-out");
    let out = utf8(output_dir.path());

    let output = run(&[
        "cu",
        "report",
        fixture.as_str(),
        "--output-dir",
        out.as_str(),
        "--program",
        "not-a-mapping",
    ]);

    assert!(!output.status.success());
    assert!(stderr(&output).contains("expected <program-id>=<path>"));
}

#[test]
fn cu_report_rejects_an_unparseable_program_id() {
    let path = write_fixture("hpsvm-cli-cu-bad-id", "cu-case", FixtureFormat::Json);
    let fixture = utf8(path.path());
    let output_dir = TempPath::new_dir("hpsvm-cli-cu-bad-id-out");
    let out = utf8(output_dir.path());
    let elf = utf8(&workspace_path(MEMO_V1));
    let bad_mapping = format!("not-an-address={elf}");

    let output = run(&[
        "cu",
        "report",
        fixture.as_str(),
        "--output-dir",
        out.as_str(),
        "--program",
        bad_mapping.as_str(),
    ]);

    assert!(!output.status.success());
    assert!(stderr(&output).contains("invalid program id"), "stderr: {}", stderr(&output));
}

/// A fixture that binds the memo program can be replayed with a vendored ELF.
#[test]
fn cu_report_accepts_a_program_mapping() {
    let (fixture, memo_id) = memo_fixture("cu-program", &workspace_path(MEMO_V1));
    let path = TempPath::new("hpsvm-cli-cu-program", "json");
    fixture.save(path.path(), FixtureFormat::Json).unwrap();
    let fixture_path = utf8(path.path());
    let output_dir = TempPath::new_dir("hpsvm-cli-cu-program-out");
    let out = utf8(output_dir.path());
    let mapping = format!("{memo_id}={}", utf8(&workspace_path(MEMO_V1)));

    let output = run(&[
        "cu",
        "report",
        fixture_path.as_str(),
        "--output-dir",
        out.as_str(),
        "--program",
        mapping.as_str(),
    ]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("PASS: cu-program"));
}

#[test]
fn cu_report_rejects_an_unwritable_output_directory() {
    let path = write_fixture("hpsvm-cli-cu-bad-out", "cu-case", FixtureFormat::Json);
    let fixture = utf8(path.path());
    // A regular file cannot be turned into the report output directory.
    let blocked = TempPath::new("hpsvm-cli-cu-blocked-out", "json");
    std::fs::write(blocked.path(), b"a file, not a directory").expect("file must be written");
    let out = utf8(blocked.path());

    let output = run(&["cu", "report", fixture.as_str(), "--output-dir", out.as_str()]);

    assert!(!output.status.success());
    assert!(stderr(&output).contains("output directory"), "stderr: {}", stderr(&output));
}

#[test]
fn cu_report_with_a_baseline_directory_renders_a_delta() {
    let path = write_fixture("hpsvm-cli-cu-baseline", "cu-case", FixtureFormat::Json);
    let fixture = utf8(path.path());
    let baseline_dir = TempPath::new_dir("hpsvm-cli-cu-baseline-in");
    let baseline = utf8(baseline_dir.path());
    let output_dir = TempPath::new_dir("hpsvm-cli-cu-baseline-out");
    let out = utf8(output_dir.path());

    // First run: no baseline exists yet, so no delta is rendered.
    let first = run(&[
        "cu",
        "report",
        fixture.as_str(),
        "--output-dir",
        baseline.as_str(),
        "--baseline-dir",
        baseline.as_str(),
    ]);
    assert!(first.status.success(), "stderr: {}", stderr(&first));

    // Second run against the written baseline: a zero delta is rendered.
    let second = run(&[
        "cu",
        "report",
        fixture.as_str(),
        "--output-dir",
        out.as_str(),
        "--baseline-dir",
        baseline.as_str(),
    ]);
    assert!(second.status.success(), "stderr: {}", stderr(&second));

    let markdown =
        std::fs::read_to_string(output_dir.path().join("cu-report.md")).expect("report must exist");
    assert!(markdown.contains("+0 (+0.00%)"), "unexpected report: {markdown}");
}
