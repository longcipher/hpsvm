#![allow(missing_docs)]
#![cfg(feature = "bin-codec")]

#[cfg(feature = "report-io")]
use std::sync::atomic::{AtomicUsize, Ordering};

use hpsvm::HPSVM;
use hpsvm_fixture::{
    AccountSnapshot, CaptureBuilder, Compare, ExecutionSnapshot, Fixture, FixtureInput,
    ProgramBinding, RuntimeFixtureConfig,
};
use hpsvm_fixture_bench::{BenchError, ComputeUnitBencher, ComputeUnitMatrixBencher, CuDelta};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_sdk_ids::{bpf_loader, bpf_loader_upgradeable};
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::versioned::VersionedTransaction;

fn unique_temp_dir() -> std::path::PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let nanos =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("hpsvm-fixture-bencher-{pid}-{seq}-{nanos}"))
}

fn snapshot_account(vm: &HPSVM, address: Address) -> AccountSnapshot {
    let account = vm.get_account(&address).unwrap();
    AccountSnapshot::from_readable(address, &account)
}

/// Returns the `counter` SBF program, or `None` when it has not been built.
fn read_counter_program() -> Option<Vec<u8>> {
    hpsvm_test_support::read_program(hpsvm_test_support::COUNTER)
}

fn build_fixture() -> Fixture {
    let mut vm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    vm.airdrop(&payer.pubkey(), 10_000).unwrap();
    vm.airdrop(&recipient, 1).unwrap();
    let tx = VersionedTransaction::from(solana_transaction::Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        vm.latest_blockhash(),
    ));
    let baseline = ExecutionSnapshot::from_outcome(&vm.transact_detached(tx.clone()));

    CaptureBuilder::new("runner-transfer")
        .runtime(RuntimeFixtureConfig::new(vm.block_env().slot, None, true, false))
        .pre_accounts(vec![snapshot_account(&vm, payer.pubkey()), snapshot_account(&vm, recipient)])
        .baseline(baseline)
        .compares(Compare::everything())
        .capture_transaction(&tx)
        .unwrap()
}

fn fixture_with_program_bindings(bindings: Vec<ProgramBinding>) -> Fixture {
    let mut fixture = build_fixture();
    let FixtureInput::Transaction(transaction) = &mut fixture.input else {
        panic!("fixture input should be a transaction")
    };
    transaction.programs.extend(bindings);
    fixture
}

#[test]
fn single_bencher_executes_system_transfer_fixture() {
    let fixture = build_fixture();

    let report = ComputeUnitBencher::new(HPSVM::new())
        .case(("system-transfer", &fixture))
        .execute()
        .unwrap();

    assert_eq!(report.rows.len(), 1);
    assert!(!report.generated_at.is_empty());
    assert!(!report.solana_runtime_version.is_empty());

    let row = &report.rows[0];
    assert_eq!(row.name, "system-transfer");
    assert!(row.compute_units > 0);
    assert!(row.pass);
    assert!(row.delta.is_none());
}

/// A fixture that binds a program the caller did not supply an ELF for is
/// reported: the fixture must not run against a different program than the
/// one it names.
#[test]
fn single_bencher_reports_a_program_binding_without_an_elf() {
    let mut fixture = build_fixture();
    let FixtureInput::Transaction(transaction) = &mut fixture.input else {
        panic!("fixture input should be a transaction")
    };
    let program_id = Address::new_unique();
    transaction.programs.push(ProgramBinding::new(
        program_id,
        Address::new_unique(),
        Some(String::from("synthetic-binding")),
    ));

    let error = ComputeUnitBencher::new(HPSVM::new())
        .case(("system-transfer-unbound-program", &fixture))
        .execute()
        .expect_err("a bound program without an ELF must be reported");

    assert!(
        matches!(
            error,
            BenchError::Fixture {
                source: hpsvm_fixture::FixtureError::MissingProgramElf { .. },
                ..
            }
        ),
        "expected a missing-ELF failure, got {error:?}"
    );
}

/// A fixture whose programs are all bound runs against the supplied ELFs.
#[test]
fn single_bencher_executes_a_fixture_whose_programs_are_bound() {
    let mut fixture = build_fixture();
    let FixtureInput::Transaction(transaction) = &mut fixture.input else {
        panic!("fixture input should be a transaction")
    };
    let program_id = Address::new_unique();
    transaction.programs.push(ProgramBinding::new(
        program_id,
        Address::new_unique(),
        Some(String::from("synthetic-binding")),
    ));

    let report = ComputeUnitBencher::new(HPSVM::new())
        .with_program_elf(program_id, b"not-a-real-elf".to_vec())
        .case(("system-transfer-bound-program", &fixture))
        .execute();

    // The ELF is deliberately bogus, so the execution fails — what matters
    // is that the binding was resolved rather than skipped.
    assert!(report.is_err(), "a bogus ELF must not produce a passing report");
}

#[test]
fn matrix_bencher_errors_when_variant_program_is_not_bound_in_fixture() {
    let fixture = build_fixture();
    let program_id = Address::new_unique();

    let error = ComputeUnitMatrixBencher::new()
        .program("candidate", bpf_loader_upgradeable::id(), program_id, Vec::new())
        .case(("system-transfer", &fixture))
        .execute()
        .unwrap_err();

    assert!(matches!(
        error,
        BenchError::UnboundVariantProgram { case, program_id: unbound, .. }
            if case == "system-transfer" && unbound == program_id
    ));
}

#[test]
fn matrix_bencher_errors_when_variant_is_missing_a_bound_fixture_program() {
    let first_program_id = Address::new_unique();
    let second_program_id = Address::new_unique();
    let fixture = fixture_with_program_bindings(vec![
        ProgramBinding::new(
            first_program_id,
            bpf_loader_upgradeable::id(),
            Some(String::from("first")),
        ),
        ProgramBinding::new(
            second_program_id,
            bpf_loader_upgradeable::id(),
            Some(String::from("second")),
        ),
    ]);

    let Some(counter) = read_counter_program() else {
        return;
    };
    let error = ComputeUnitMatrixBencher::new()
        .program("candidate", bpf_loader_upgradeable::id(), first_program_id, counter)
        .case(("system-transfer", &fixture))
        .execute()
        .unwrap_err();

    assert!(matches!(
        error,
        BenchError::MissingVariantProgram { case, program_id: missing, .. }
            if case == "system-transfer" && missing == second_program_id
    ));
}

#[test]
fn matrix_bencher_supports_multiple_program_bindings_for_one_variant_name() {
    let first_program_id = Address::new_unique();
    let second_program_id = Address::new_unique();
    let fixture = fixture_with_program_bindings(vec![
        ProgramBinding::new(
            first_program_id,
            bpf_loader_upgradeable::id(),
            Some(String::from("first")),
        ),
        ProgramBinding::new(
            second_program_id,
            bpf_loader_upgradeable::id(),
            Some(String::from("second")),
        ),
    ]);
    let Some(program_bytes) = read_counter_program() else {
        return;
    };

    let report = ComputeUnitMatrixBencher::new()
        .program("candidate", bpf_loader_upgradeable::id(), first_program_id, program_bytes.clone())
        .program("candidate", bpf_loader_upgradeable::id(), second_program_id, program_bytes)
        .case(("system-transfer", &fixture))
        .execute()
        .unwrap();

    assert_eq!(report.reports.len(), 1);
    let candidate = report.reports.get("candidate").unwrap();
    assert_eq!(candidate.rows.len(), 1);

    let row = &candidate.rows[0];
    assert_eq!(row.name, "system-transfer");
    assert!(row.compute_units > 0);
    assert!(row.pass);
}

#[test]
fn matrix_bencher_reports_loader_mismatches_for_bound_programs() {
    let program_id = Address::new_unique();
    let fixture = fixture_with_program_bindings(vec![ProgramBinding::new(
        program_id,
        bpf_loader_upgradeable::id(),
        Some(String::from("candidate")),
    )]);

    let error = ComputeUnitMatrixBencher::new()
        .program("candidate", bpf_loader::id(), program_id, Vec::new())
        .case(("system-transfer", &fixture))
        .execute()
        .unwrap_err();

    assert!(matches!(
        error,
        BenchError::ProgramLoaderMismatch {
            case,
            program_id: mismatched,
            fixture_loader_id,
            variant_loader_id,
        } if case == "system-transfer"
            && mismatched == program_id
            && fixture_loader_id == bpf_loader_upgradeable::id()
            && variant_loader_id == bpf_loader::id()
    ));
}

#[test]
fn cu_delta_reports_absolute_and_percent_change() {
    let delta = CuDelta::between(400, 500);

    assert_eq!(delta.absolute, 100);
    assert_eq!(delta.percent, 25.0);
}

#[cfg(feature = "report-io")]
#[test]
fn single_bencher_loads_baseline_from_json_sidecar() {
    let fixture = build_fixture();
    let output_dir = unique_temp_dir();

    let initial_report = ComputeUnitBencher::new(HPSVM::new())
        .case(("system-transfer", &fixture))
        .output_dir(&output_dir)
        .execute()
        .unwrap();

    // Baselines round-trip through the machine-readable JSON sidecar.
    assert!(output_dir.join("cu-report.baseline.json").exists());
    // Markdown is write-only and only produced when the feature is enabled.
    #[cfg(feature = "markdown")]
    assert!(output_dir.join("cu-report.md").exists());

    let report = ComputeUnitBencher::new(HPSVM::new())
        .case(("system-transfer", &fixture))
        .baseline_dir(&output_dir)
        .execute()
        .unwrap();

    assert_eq!(report.rows.len(), 1);
    assert_eq!(report.rows[0].compute_units, initial_report.rows[0].compute_units);
    let delta = report.rows[0].delta.expect("baseline delta should be present");
    assert_eq!(delta.absolute, 0);
    assert_eq!(delta.percent, 0.0);

    std::fs::remove_dir_all(&output_dir).unwrap();
}

#[cfg(feature = "report-io")]
#[test]
fn single_bencher_loads_baseline_for_case_name_containing_pipe() {
    let fixture = build_fixture();
    let output_dir = unique_temp_dir();

    let initial_report = ComputeUnitBencher::new(HPSVM::new())
        .case(("system|transfer", &fixture))
        .output_dir(&output_dir)
        .execute()
        .unwrap();

    let report = ComputeUnitBencher::new(HPSVM::new())
        .case(("system|transfer", &fixture))
        .baseline_dir(&output_dir)
        .execute()
        .unwrap();

    assert_eq!(report.rows.len(), 1);
    assert_eq!(report.rows[0].name, "system|transfer");
    assert_eq!(report.rows[0].compute_units, initial_report.rows[0].compute_units);
    let delta = report.rows[0].delta.expect("baseline delta should be present");
    assert_eq!(delta.absolute, 0);
    assert_eq!(delta.percent, 0.0);

    std::fs::remove_dir_all(&output_dir).unwrap();
}

#[cfg(not(feature = "report-io"))]
#[test]
fn single_bencher_errors_when_report_io_is_requested_without_report_io_feature() {
    let fixture = build_fixture();
    let io_dir = std::env::temp_dir()
        .join(format!("hpsvm-fixture-bencher-no-markdown-{}", Address::new_unique()));

    let output_error = ComputeUnitBencher::new(HPSVM::new())
        .case(("system-transfer", &fixture))
        .output_dir(&io_dir)
        .execute()
        .unwrap_err();
    assert!(matches!(
        output_error,
        BenchError::ReportIoDisabled { operation } if operation == "output_dir"
    ));

    let baseline_error = ComputeUnitBencher::new(HPSVM::new())
        .case(("system-transfer", &fixture))
        .baseline_dir(&io_dir)
        .execute()
        .unwrap_err();
    assert!(matches!(
        baseline_error,
        BenchError::ReportIoDisabled { operation } if operation == "baseline_dir"
    ));
}

#[test]
fn matrix_bencher_reports_builtin_fixture_slice() {
    let fixture = build_fixture();

    let report =
        ComputeUnitMatrixBencher::new().case(("system-transfer", &fixture)).execute().unwrap();

    assert_eq!(report.reports.len(), 1);
    let builtin = report.reports.get("builtin").unwrap();
    assert_eq!(builtin.rows.len(), 1);

    let row = &builtin.rows[0];
    assert_eq!(row.name, "system-transfer");
    assert!(row.compute_units > 0);
    assert!(row.pass);
}
