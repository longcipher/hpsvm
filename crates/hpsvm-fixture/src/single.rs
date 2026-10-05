use std::path::PathBuf;

use hpsvm::HPSVM;

use crate::{
    BenchError, Fixture, FixtureBenchCase, FixtureInput, FixtureRunner, ResultConfig,
    report::{CuDelta, CuReport, CuReportRow},
};

#[derive(Debug)]
#[must_use = "benchers must be configured and executed"]
pub struct ComputeUnitBencher<'a> {
    vm: HPSVM,
    cases: Vec<FixtureBenchCase<'a>>,
    must_pass: bool,
    baseline_dir: Option<PathBuf>,
    output_dir: Option<PathBuf>,
}

impl<'a> ComputeUnitBencher<'a> {
    pub fn new(vm: HPSVM) -> Self {
        Self { vm, cases: Vec::new(), must_pass: false, baseline_dir: None, output_dir: None }
    }

    pub fn case(mut self, case: FixtureBenchCase<'a>) -> Self {
        self.cases.push(case);
        self
    }

    pub fn must_pass(mut self, must_pass: bool) -> Self {
        self.must_pass = must_pass;
        self
    }

    pub fn baseline_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.baseline_dir = Some(path.into());
        self
    }

    pub fn output_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.output_dir = Some(path.into());
        self
    }

    pub fn execute(self) -> Result<CuReport, BenchError> {
        let Self { vm, cases, must_pass, baseline_dir, output_dir } = self;
        if cases.is_empty() {
            return Err(BenchError::MissingCases);
        }

        let baseline_map = CuReport::baseline_map(baseline_dir.as_deref())?;
        let mut runner = FixtureRunner::new(vm);
        let mut rows = Vec::with_capacity(cases.len());

        for (name, fixture) in cases {
            let normalized_fixture = fixture_for_preloaded_vm(fixture);
            let execution = runner
                .run(&normalized_fixture)
                .map_err(|source| BenchError::Fixture { name: String::from(name), source })?;

            let pass = execution.snapshot.compare_with(
                &fixture.expectations.baseline,
                &fixture.expectations.compares,
                &ResultConfig { panic: false, verbose: true },
            );
            if must_pass && !pass {
                return Err(BenchError::ExpectationFailed { name: String::from(name) });
            }

            rows.push(CuReportRow {
                name: String::from(name),
                compute_units: execution.snapshot.compute_units_consumed,
                delta: baseline_map.get(name).copied().map(|baseline| {
                    CuDelta::between(baseline, execution.snapshot.compute_units_consumed)
                }),
                pass,
            });
        }

        let report = CuReport::new(rows);
        report.write_to_dir(output_dir.as_deref())?;
        Ok(report)
    }
}

fn fixture_for_preloaded_vm(fixture: &Fixture) -> Fixture {
    let mut normalized = fixture.clone();
    match &mut normalized.input {
        FixtureInput::Transaction(transaction) => transaction.programs.clear(),
        #[cfg(feature = "instruction-fixture")]
        FixtureInput::Instruction(instruction) => instruction.programs.clear(),
    }
    normalized
}

#[cfg(all(test, feature = "bin-codec"))]
mod tests {
    use solana_address::Address;
    use solana_keypair::Keypair;
    use solana_message::{Message, VersionedMessage};
    use solana_signer::Signer;
    use solana_system_interface::instruction::transfer;
    use solana_transaction::versioned::VersionedTransaction;

    use super::*;
    use crate::{
        AccountSnapshot, CaptureBuilder, Compare, ExecutionSnapshot, ExecutionSnapshotFields,
        ExecutionStatus, FixtureExpectations, FixtureHeader, FixtureKind, RuntimeFixtureConfig,
        TransactionFixture,
    };

    fn empty_snapshot() -> ExecutionSnapshot {
        ExecutionSnapshot::from_fields(ExecutionSnapshotFields {
            status: ExecutionStatus::Success,
            included: true,
            compute_units_consumed: 0,
            fee: 0,
            logs: Vec::new(),
            return_data: None,
            inner_instructions: Vec::new(),
            post_accounts: Vec::new(),
        })
    }

    /// Builds a transfer fixture whose baseline matches the fresh VM, so the
    /// bencher row reports `pass == true` and a non-zero compute unit count.
    fn passing_fixture() -> Fixture {
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
        let pre_accounts = vec![
            AccountSnapshot::from_readable(
                payer.pubkey(),
                &svm.get_account(&payer.pubkey()).unwrap(),
            ),
            AccountSnapshot::from_readable(recipient, &svm.get_account(&recipient).unwrap()),
        ];

        CaptureBuilder::new("case")
            .runtime(RuntimeFixtureConfig::new(svm.block_env().slot, None, true, false))
            .pre_accounts(pre_accounts)
            .baseline(baseline)
            .compares(Compare::everything())
            .capture_transaction(&tx)
            .expect("fixture capture must succeed")
    }

    /// The passing fixture's baseline with only the fee changed, so the
    /// comparison fails for exactly one reason.
    ///
    /// Building this from `passing_fixture`'s real baseline rather than from
    /// `empty_snapshot` matters: an expectation that differs from reality in
    /// every field would still report a failed row if the fee mutation were
    /// dropped, which would leave the mutation unnoticed.
    fn failing_fixture() -> Fixture {
        let fixture = passing_fixture();
        let actual_fee = fixture.expectations.baseline.fee;
        Fixture::new(
            fixture.header,
            fixture.input,
            FixtureExpectations::new(
                ExecutionSnapshot { fee: actual_fee.saturating_add(1), ..empty_snapshot() },
                Compare::everything(),
            ),
        )
    }

    #[test]
    fn execute_without_cases_reports_missing_cases() {
        let error =
            ComputeUnitBencher::new(HPSVM::new()).execute().expect_err("no cases should fail");
        assert!(matches!(error, BenchError::MissingCases), "got {error:?}");
    }

    #[test]
    fn a_passing_case_produces_a_row_with_no_delta() {
        let fixture = passing_fixture();
        let report = ComputeUnitBencher::new(HPSVM::new())
            .case(("case", &fixture))
            .execute()
            .expect("a passing case should run");

        assert_eq!(report.rows.len(), 1);
        assert_eq!(report.rows[0].name, "case");
        assert!(report.rows[0].pass);
        assert!(report.rows[0].compute_units > 0);
        assert_eq!(report.rows[0].delta, None);
    }

    /// With `must_pass(false)` a failing expectation is reported as a failed row
    /// instead of aborting the run.
    #[test]
    fn a_failing_case_is_reported_when_must_pass_is_false() {
        let fixture = failing_fixture();
        let report = ComputeUnitBencher::new(HPSVM::new())
            .case(("case", &fixture))
            .must_pass(false)
            .execute()
            .expect("a failing comparison must not abort by default");

        assert_eq!(report.rows.len(), 1);
        assert!(!report.rows[0].pass);
    }

    /// `must_pass(true)` turns a failing expectation into a hard error.
    #[test]
    fn a_failing_case_aborts_when_must_pass_is_true() {
        let fixture = failing_fixture();
        let error = ComputeUnitBencher::new(HPSVM::new())
            .case(("failing", &fixture))
            .must_pass(true)
            .execute()
            .expect_err("must_pass should abort");

        match error {
            BenchError::ExpectationFailed { name } => assert_eq!(name, "failing"),
            other => panic!("expected BenchError::ExpectationFailed, got {other:?}"),
        }
    }

    #[test]
    fn must_pass_does_not_reject_a_passing_case() {
        let fixture = passing_fixture();
        let report = ComputeUnitBencher::new(HPSVM::new())
            .case(("case", &fixture))
            .must_pass(true)
            .execute()
            .expect("a passing case should survive must_pass");
        assert!(report.rows[0].pass);
    }

    #[test]
    fn several_cases_all_appear_in_the_report() {
        let first = passing_fixture();
        let second = passing_fixture();
        let report = ComputeUnitBencher::new(HPSVM::new())
            .case(("a", &first))
            .case(("b", &second))
            .execute()
            .expect("two cases should run");

        assert_eq!(report.rows.len(), 2);
        assert_eq!(report.rows[0].name, "a");
        assert_eq!(report.rows[1].name, "b");
    }

    /// A runner failure must be wrapped with the case name so CI logs identify
    /// the offending fixture.
    #[test]
    fn a_runner_failure_is_wrapped_with_the_case_name() {
        let fixture = Fixture::new(
            FixtureHeader::new("broken", FixtureKind::Transaction),
            FixtureInput::Transaction(TransactionFixture::new(
                RuntimeFixtureConfig::new(0, None, false, false),
                Vec::new(),
                Vec::new(),
                vec![0xff; 8],
            )),
            FixtureExpectations::new(empty_snapshot(), Compare::everything()),
        );

        let error = ComputeUnitBencher::new(HPSVM::new())
            .case(("broken-case", &fixture))
            .execute()
            .expect_err("an undecodable transaction should fail");

        match error {
            BenchError::Fixture { name, .. } => assert_eq!(name, "broken-case"),
            other => panic!("expected BenchError::Fixture, got {other:?}"),
        }
    }

    /// A blockhash-checked fixture is refused by the runner.
    #[test]
    fn a_blockhash_checked_fixture_is_rejected() {
        let mut svm = HPSVM::new();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000).unwrap();
        let tx = VersionedTransaction::try_new(
            VersionedMessage::Legacy(Message::new(&[], Some(&payer.pubkey()))),
            &[&payer],
        )
        .unwrap();
        let fixture = CaptureBuilder::new("checked")
            .runtime(RuntimeFixtureConfig::new(svm.block_env().slot, None, true, true))
            .baseline(empty_snapshot())
            .compares(Compare::everything())
            .capture_transaction(&tx)
            .unwrap();

        let error = ComputeUnitBencher::new(HPSVM::new())
            .case(("checked", &fixture))
            .execute()
            .expect_err("blockhash_check should be refused");
        assert!(matches!(error, BenchError::Fixture { .. }), "got {error:?}");
    }

    /// The bencher clears the fixture's program bindings so the preloaded VM is
    /// used as-is; a stale ELF requirement would surface as `MissingProgramElf`.
    #[test]
    fn preloaded_vm_bypasses_the_fixture_program_bindings() {
        let fixture = passing_fixture();
        let report = ComputeUnitBencher::new(HPSVM::new())
            .case(("case", &fixture))
            .execute()
            .expect("the preloaded VM should satisfy the fixture");
        assert_eq!(report.rows.len(), 1);
    }

    /// A fixture that *does* bind a program still runs, because the bindings are
    /// stripped before the runner resolves them.
    #[test]
    fn fixture_program_bindings_are_stripped_before_running() {
        let mut fixture = passing_fixture();
        let FixtureInput::Transaction(transaction) = &mut fixture.input else {
            unreachable!("transaction fixture");
        };
        transaction.programs =
            vec![crate::ProgramBinding::new(Address::new_unique(), Address::new_unique(), None)];

        let report = ComputeUnitBencher::new(HPSVM::new())
            .case(("bound", &fixture))
            .execute()
            .expect("bindings should be stripped, not resolved");
        assert!(report.rows[0].pass);
    }

    /// A baseline directory that does not exist yields no deltas rather than an
    /// error.
    #[test]
    fn a_missing_baseline_directory_yields_no_deltas() {
        let fixture = passing_fixture();
        let report = ComputeUnitBencher::new(HPSVM::new())
            .case(("case", &fixture))
            .baseline_dir(std::env::temp_dir().join("hpsvm-bencher-no-baseline"))
            .execute()
            .expect("a missing baseline is not an error");
        assert_eq!(report.rows[0].delta, None);
    }

    /// A baseline row makes the row carry a delta computed from it.
    #[cfg(feature = "report-io")]
    #[test]
    fn a_baseline_row_produces_a_delta() {
        let dir =
            std::env::temp_dir().join(format!("hpsvm-bencher-baseline-{}", Address::new_unique()));
        let fixture = passing_fixture();

        // Write a baseline of zero so the delta equals the measured units.
        let baseline_report = crate::CuReport::new(vec![crate::CuReportRow {
            name: String::from("case"),
            compute_units: 0,
            delta: None,
            pass: true,
        }]);
        baseline_report.write_to_dir(Some(&dir)).expect("baseline should write");

        let report = ComputeUnitBencher::new(HPSVM::new())
            .case(("case", &fixture))
            .baseline_dir(&dir)
            .execute()
            .expect("a matching baseline should run");

        let delta = report.rows[0].delta.expect("a baseline row must produce a delta");
        assert_eq!(delta.absolute, report.rows[0].compute_units as i64);
        assert_eq!(delta.percent, 100.0);

        std::fs::remove_dir_all(dir).ok();
    }

    /// An output directory that cannot be created must surface as a bench error.
    #[cfg(feature = "report-io")]
    #[test]
    fn an_unwritable_output_directory_is_reported() {
        let fixture = passing_fixture();
        let blocked = std::env::temp_dir().join(format!("hpsvm-bencher-{}", Address::new_unique()));
        std::fs::write(&blocked, b"a file, not a directory").unwrap();

        let error = ComputeUnitBencher::new(HPSVM::new())
            .case(("case", &fixture))
            .output_dir(&blocked)
            .execute()
            .expect_err("the output dir cannot be created");

        assert!(matches!(error, BenchError::CreateOutputDir { .. }), "got {error:?}");

        std::fs::remove_file(blocked).ok();
    }

    /// A malformed baseline directory surfaces as `InvalidBaseline`.
    #[cfg(feature = "report-io")]
    #[test]
    fn a_malformed_baseline_is_reported() {
        let dir = std::env::temp_dir().join(format!("hpsvm-bencher-bad-{}", Address::new_unique()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(crate::report::BASELINE_REPORT_FILE_NAME), b"{ broken").unwrap();

        let error = ComputeUnitBencher::new(HPSVM::new())
            .case(("case", &passing_fixture()))
            .baseline_dir(&dir)
            .execute()
            .expect_err("a malformed baseline should fail");

        assert!(matches!(error, BenchError::InvalidBaseline { .. }), "got {error:?}");

        std::fs::remove_dir_all(dir).ok();
    }

    /// A well-formed output directory receives both artifacts.
    #[cfg(all(feature = "report-io", feature = "markdown"))]
    #[test]
    fn an_output_directory_receives_the_report_files() {
        let dir = std::env::temp_dir().join(format!("hpsvm-bencher-out-{}", Address::new_unique()));
        let report = ComputeUnitBencher::new(HPSVM::new())
            .case(("case", &passing_fixture()))
            .output_dir(&dir)
            .execute()
            .expect("the output dir should be written");

        assert!(dir.join(crate::report::BASELINE_REPORT_FILE_NAME).exists());
        let markdown =
            std::fs::read_to_string(dir.join(crate::report::MARKDOWN_REPORT_FILE_NAME)).unwrap();
        assert!(markdown.contains("| case |"));
        assert_eq!(report.rows.len(), 1);

        std::fs::remove_dir_all(dir).ok();
    }
}
