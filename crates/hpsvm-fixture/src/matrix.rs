use std::collections::BTreeMap;

use hpsvm::HPSVM;
use solana_address::Address;

use crate::{
    BUILTIN_VARIANT_NAME, BenchError, FixtureBenchCase, FixtureInput, FixtureRunner, ResultConfig,
    generated_at_string,
    report::{CuReport, CuReportRow},
    solana_runtime_version_string,
};

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct MatrixReport {
    pub generated_at: String,
    pub solana_runtime_version: String,
    pub reports: BTreeMap<String, CuReport>,
}

#[derive(Debug, Clone)]
struct ProgramVariant {
    name: String,
    loader_id: Address,
    program_id: Address,
    elf: Vec<u8>,
}

#[derive(Debug, Clone)]
struct ProgramVariantSet {
    name: String,
    programs: BTreeMap<Address, ProgramVariant>,
}

#[derive(Debug)]
#[must_use = "benchers must be configured and executed"]
pub struct ComputeUnitMatrixBencher<'a> {
    programs: Vec<ProgramVariant>,
    cases: Vec<FixtureBenchCase<'a>>,
}

impl<'a> ComputeUnitMatrixBencher<'a> {
    pub fn new() -> Self {
        Self { programs: Vec::new(), cases: Vec::new() }
    }

    pub fn program(
        mut self,
        name: impl Into<String>,
        loader_id: Address,
        program_id: Address,
        elf: Vec<u8>,
    ) -> Self {
        self.programs.push(ProgramVariant { name: name.into(), loader_id, program_id, elf });
        self
    }

    pub fn case(mut self, case: FixtureBenchCase<'a>) -> Self {
        self.cases.push(case);
        self
    }

    pub fn execute(self) -> Result<MatrixReport, BenchError> {
        let Self { programs, cases } = self;
        if cases.is_empty() {
            return Err(BenchError::MissingCases);
        }

        let mut reports = BTreeMap::new();
        if programs.is_empty() {
            reports.insert(String::from(BUILTIN_VARIANT_NAME), execute_variant(&cases, None)?);
        } else {
            for variant in group_variant_sets(programs) {
                let report = execute_variant(&cases, Some(&variant))?;
                reports.insert(variant.name, report);
            }
        }

        Ok(MatrixReport {
            generated_at: generated_at_string(),
            solana_runtime_version: solana_runtime_version_string(),
            reports,
        })
    }
}

impl Default for ComputeUnitMatrixBencher<'_> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "markdown")]
impl MatrixReport {
    pub fn render_markdown(&self) -> String {
        let mut markdown = String::from("# Compute Unit Matrix\n\n");
        markdown.push_str(&format!("Generated at: {}\n", self.generated_at));
        markdown.push_str(&format!("Solana runtime version: {}\n\n", self.solana_runtime_version));

        for (name, report) in &self.reports {
            markdown.push_str("## ");
            markdown.push_str(name);
            markdown.push_str("\n\n");
            markdown.push_str(&crate::report::render_table(report));
            markdown.push('\n');
        }

        markdown
    }
}

fn execute_variant(
    cases: &[FixtureBenchCase<'_>],
    variant: Option<&ProgramVariantSet>,
) -> Result<CuReport, BenchError> {
    let mut runner = FixtureRunner::new(HPSVM::new());
    if let Some(variant) = variant {
        for program in variant.programs.values() {
            runner = runner.with_program_elf(program.program_id, program.elf.clone());
        }
    }

    let mut rows = Vec::with_capacity(cases.len());
    for (name, fixture) in cases {
        if let Some(variant) = variant {
            validate_variant_programs(name, fixture, variant)?;
        }

        let execution = runner
            .run(fixture)
            .map_err(|source| BenchError::Fixture { name: String::from(*name), source })?;

        rows.push(CuReportRow {
            name: String::from(*name),
            compute_units: execution.snapshot.compute_units_consumed,
            delta: None,
            pass: execution.snapshot.compare_with(
                &fixture.expectations.baseline,
                &fixture.expectations.compares,
                &ResultConfig { panic: false, verbose: true },
            ),
        });
    }

    Ok(CuReport::new(rows))
}

fn group_variant_sets(programs: Vec<ProgramVariant>) -> Vec<ProgramVariantSet> {
    let mut grouped = BTreeMap::<String, BTreeMap<Address, ProgramVariant>>::new();

    for variant in programs {
        grouped.entry(variant.name.clone()).or_default().insert(variant.program_id, variant);
    }

    grouped.into_iter().map(|(name, programs)| ProgramVariantSet { name, programs }).collect()
}

fn validate_variant_programs(
    case_name: &str,
    fixture: &crate::Fixture,
    variant: &ProgramVariantSet,
) -> Result<(), BenchError> {
    let mut bound_programs = BTreeMap::new();
    for program in fixture_programs(fixture) {
        bound_programs.insert(program.program_id, program.loader_id);
    }

    for program in variant.programs.values() {
        let Some(fixture_loader_id) = bound_programs.get(&program.program_id).copied() else {
            return Err(BenchError::UnboundVariantProgram {
                case: String::from(case_name),
                name: variant.name.clone(),
                program_id: program.program_id,
            });
        };

        if fixture_loader_id != program.loader_id {
            return Err(BenchError::ProgramLoaderMismatch {
                case: String::from(case_name),
                program_id: program.program_id,
                fixture_loader_id,
                variant_loader_id: program.loader_id,
            });
        }
    }

    for (program_id, loader_id) in bound_programs {
        if !variant.programs.contains_key(&program_id) {
            return Err(BenchError::MissingVariantProgram {
                case: String::from(case_name),
                name: variant.name.clone(),
                program_id,
                loader_id,
            });
        }
    }

    Ok(())
}

fn fixture_programs(fixture: &crate::Fixture) -> &[crate::ProgramBinding] {
    match &fixture.input {
        FixtureInput::Transaction(transaction) => &transaction.programs,
        #[cfg(feature = "instruction-fixture")]
        FixtureInput::Instruction(instruction) => &instruction.programs,
    }
}

#[cfg(all(test, feature = "bin-codec"))]
mod tests {
    use solana_keypair::Keypair;
    use solana_message::{Message, VersionedMessage};
    use solana_signer::Signer;
    use solana_system_interface::instruction::transfer;
    use solana_transaction::versioned::VersionedTransaction;

    use super::*;
    use crate::{
        AccountSnapshot, CaptureBuilder, Compare, ExecutionSnapshot, ExecutionSnapshotFields,
        ExecutionStatus, Fixture, FixtureExpectations, FixtureHeader, FixtureKind, ProgramBinding,
        RuntimeFixtureConfig, TransactionFixture,
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

    /// A transfer fixture whose baseline matches what the VM produces, so the
    /// matrix rows report `pass == true`.
    fn passing_fixture() -> crate::Fixture {
        let mut svm = hpsvm::HPSVM::new();
        let payer = Keypair::new();
        let recipient = Address::new_unique();
        svm.airdrop(&payer.pubkey(), 10_000).unwrap();
        svm.airdrop(&recipient, 1).unwrap();

        // A real transfer, so the baseline records a non-zero compute unit
        // count rather than the zero of an empty message.
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

        CaptureBuilder::new("matrix-case")
            .runtime(RuntimeFixtureConfig::new(svm.block_env().slot, None, true, false))
            .pre_accounts(pre_accounts)
            .baseline(baseline)
            .compares(Compare::everything_but_compute_units())
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
        let crate::FixtureInput::Transaction(transaction) = &fixture.input else {
            unreachable!("transaction fixture");
        };
        let actual_fee = fixture.expectations.baseline.fee;
        Fixture::new(
            fixture.header.clone(),
            crate::FixtureInput::Transaction(TransactionFixture::new(
                transaction.runtime,
                transaction.programs.clone(),
                transaction.pre_accounts.clone(),
                transaction.transaction_bytes.clone(),
            )),
            FixtureExpectations::new(
                ExecutionSnapshot { fee: actual_fee.saturating_add(1), ..empty_snapshot() },
                Compare::everything(),
            ),
        )
    }

    #[test]
    fn execute_without_cases_reports_missing_cases() {
        let error = ComputeUnitMatrixBencher::new().execute().expect_err("no cases should fail");
        assert!(matches!(error, BenchError::MissingCases), "got {error:?}");
    }

    #[test]
    fn default_constructor_matches_new() {
        let fixture = passing_fixture();
        let report = ComputeUnitMatrixBencher::default()
            .case(("case", &fixture))
            .execute()
            .expect("a single case should run");
        assert_eq!(report.reports.len(), 1);
    }

    /// With no program variants the matrix runs the builtin slice.
    #[test]
    fn no_variants_produces_the_builtin_report() {
        let fixture = passing_fixture();
        let report = ComputeUnitMatrixBencher::new()
            .case(("case", &fixture))
            .execute()
            .expect("a single case should run");

        assert_eq!(report.reports.len(), 1);
        let builtin = report.reports.get(BUILTIN_VARIANT_NAME).expect("builtin report must exist");
        assert_eq!(builtin.rows.len(), 1);
        assert_eq!(builtin.rows[0].name, "case");
        assert!(builtin.rows[0].pass);
        assert!(builtin.rows[0].delta.is_none());
    }

    #[test]
    fn multiple_cases_all_appear_in_the_report() {
        let first = passing_fixture();
        let second = passing_fixture();
        let report = ComputeUnitMatrixBencher::new()
            .case(("a", &first))
            .case(("b", &second))
            .execute()
            .expect("two cases should run");

        let builtin = report.reports.get(BUILTIN_VARIANT_NAME).unwrap();
        assert_eq!(builtin.rows.len(), 2);
        assert_eq!(builtin.rows[0].name, "a");
        assert_eq!(builtin.rows[1].name, "b");
    }

    /// `compute_units` must be recorded even when the comparison passes without
    /// checking compute units.
    #[test]
    fn rows_record_compute_units_without_a_baseline_delta() {
        let fixture = passing_fixture();
        let report = ComputeUnitMatrixBencher::new().case(("case", &fixture)).execute().unwrap();
        let row = &report.reports.get(BUILTIN_VARIANT_NAME).unwrap().rows[0];
        assert!(row.compute_units > 0, "expected a non-zero compute unit count");
        assert_eq!(row.delta, None);
    }

    /// A failing expectation is recorded as `pass == false` rather than aborting,
    /// because the matrix has no `must_pass` mode.
    #[test]
    fn a_failing_expectation_is_reported_as_a_failed_row() {
        let fixture = failing_fixture();
        let report = ComputeUnitMatrixBencher::new()
            .case(("case", &fixture))
            .execute()
            .expect("a failing comparison must not abort the matrix");
        assert!(!report.reports.get(BUILTIN_VARIANT_NAME).unwrap().rows[0].pass);
    }

    /// A variant program the fixture does not bind is rejected before running.
    #[test]
    fn an_unbound_variant_program_is_rejected() {
        let fixture = passing_fixture();
        let error = ComputeUnitMatrixBencher::new()
            .program("variant", Address::new_unique(), Address::new_unique(), vec![0; 8])
            .case(("case", &fixture))
            .execute()
            .expect_err("unbound variant should fail");

        match error {
            BenchError::UnboundVariantProgram { case, name, .. } => {
                assert_eq!(case, "case");
                assert_eq!(name, "variant");
            }
            other => panic!("expected BenchError::UnboundVariantProgram, got {other:?}"),
        }
    }

    /// A loader mismatch between the variant and the binding is rejected.
    #[test]
    fn a_loader_mismatch_is_rejected() {
        let program_id = Address::new_unique();
        let fixture = bound_fixture(program_id);
        let error = ComputeUnitMatrixBencher::new()
            .program("variant", Address::new_unique(), program_id, vec![0; 8])
            .case(("case", &fixture))
            .execute()
            .expect_err("loader mismatch should fail");

        assert!(matches!(error, BenchError::ProgramLoaderMismatch { .. }), "got {error:?}");
    }

    /// A bound program the variant does not supply is rejected. The variant must
    /// supply the *other* bound program correctly, otherwise the unbound-program
    /// check fires first.
    #[test]
    fn a_missing_variant_program_is_rejected() {
        let first = Address::new_unique();
        let second = Address::new_unique();
        // The fixture binds both programs; the variant supplies only `first`, so
        // `second` is the one that goes missing.
        let fixture = bound_fixture_multi(first, second);
        let loader = fixture_loader_id(first);

        let error = ComputeUnitMatrixBencher::new()
            .program("variant", loader, first, vec![0; 8])
            .case(("case", &fixture))
            .execute()
            .expect_err("missing variant program should fail");

        match error {
            BenchError::MissingVariantProgram { case, name, program_id: missing, .. } => {
                assert_eq!(case, "case");
                assert_eq!(name, "variant");
                assert_eq!(missing, second);
            }
            other => panic!("expected BenchError::MissingVariantProgram, got {other:?}"),
        }
    }

    /// Several programs sharing one variant name collapse into one variant set.
    #[test]
    fn variants_sharing_a_name_are_grouped() {
        let loader = fixture_loader_id(Address::new_unique());
        let first = Address::new_unique();
        let second = Address::new_unique();
        let third = Address::new_unique();

        let sets = group_variant_sets(vec![
            ProgramVariant {
                name: String::from("shared"),
                loader_id: loader,
                program_id: first,
                elf: vec![1; 8],
            },
            ProgramVariant {
                name: String::from("shared"),
                loader_id: loader,
                program_id: second,
                elf: vec![2; 8],
            },
            ProgramVariant {
                name: String::from("shared"),
                loader_id: loader,
                program_id: third,
                elf: vec![3; 8],
            },
        ]);

        assert_eq!(sets.len(), 1, "one name must produce one variant set");
        assert_eq!(sets[0].name, "shared");
        assert_eq!(sets[0].programs.len(), 3);
    }

    /// Two distinct variant names produce two separate variant sets, each
    /// holding only its own program.
    #[test]
    fn distinct_variant_names_produce_distinct_reports() {
        let loader = fixture_loader_id(Address::new_unique());
        let program_id = Address::new_unique();

        let sets = group_variant_sets(vec![
            ProgramVariant {
                name: String::from("alpha"),
                loader_id: loader,
                program_id,
                elf: vec![1; 8],
            },
            ProgramVariant {
                name: String::from("beta"),
                loader_id: loader,
                program_id,
                elf: vec![2; 8],
            },
        ]);

        assert_eq!(sets.len(), 2);
        let names: Vec<&str> = sets.iter().map(|set| set.name.as_str()).collect();
        assert!(names.contains(&"alpha"), "got {names:?}");
        assert!(names.contains(&"beta"), "got {names:?}");
        // The sets are ordered by name, which keeps report output stable.
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    /// A program registered under two names yields one entry per name, and a
    /// repeated registration of the same program keeps only the last ELF.
    #[test]
    fn grouping_is_ordered_and_deduplicated() {
        let loader = fixture_loader_id(Address::new_unique());
        let program_id = Address::new_unique();

        let sets = group_variant_sets(vec![
            ProgramVariant {
                name: String::from("zulu"),
                loader_id: loader,
                program_id,
                elf: vec![1; 8],
            },
            ProgramVariant {
                name: String::from("zulu"),
                loader_id: loader,
                program_id,
                elf: vec![9; 8],
            },
            ProgramVariant {
                name: String::from("alpha"),
                loader_id: loader,
                program_id,
                elf: vec![2; 8],
            },
        ]);

        let names: Vec<&str> = sets.iter().map(|set| set.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "zulu"]);
        assert_eq!(sets[1].programs.len(), 1, "a program id appears once per set");
        assert_eq!(
            sets[1].programs.get(&program_id).expect("program is registered").elf,
            vec![9u8; 8]
        );
    }

    /// An unrunnable fixture must be wrapped with the case name.
    #[test]
    fn a_runner_failure_is_wrapped_with_the_case_name() {
        let fixture = Fixture::new(
            FixtureHeader::new("broken", FixtureKind::Transaction),
            crate::FixtureInput::Transaction(TransactionFixture::new(
                RuntimeFixtureConfig::new(0, None, false, false),
                Vec::new(),
                Vec::new(),
                vec![0xff; 8],
            )),
            FixtureExpectations::new(empty_snapshot(), Compare::everything()),
        );

        let error = ComputeUnitMatrixBencher::new()
            .case(("broken-case", &fixture))
            .execute()
            .expect_err("an undecodable transaction should fail");

        match error {
            BenchError::Fixture { name, source } => {
                assert_eq!(name, "broken-case");
                let _ = source;
            }
            other => panic!("expected BenchError::Fixture, got {other:?}"),
        }
    }

    /// A blockhash-checked fixture is refused by the runner before execution.
    #[test]
    fn a_blockhash_checked_fixture_is_rejected() {
        let fixture = blockhash_checked_fixture();
        let error = ComputeUnitMatrixBencher::new()
            .case(("checked", &fixture))
            .execute()
            .expect_err("blockhash_check should be refused");

        assert!(
            matches!(error, BenchError::Fixture { .. }),
            "expected BenchError::Fixture, got {error:?}"
        );
    }

    #[test]
    fn the_matrix_report_carries_generation_metadata() {
        let fixture = passing_fixture();
        let report = ComputeUnitMatrixBencher::new().case(("case", &fixture)).execute().unwrap();

        assert!(!report.generated_at.is_empty());
        assert!(report.generated_at.chars().all(|c| c.is_ascii_digit()));
        assert_ne!(report.solana_runtime_version, "unknown");
    }

    #[cfg(feature = "markdown")]
    #[test]
    fn the_matrix_report_renders_one_section_per_variant() {
        let fixture = passing_fixture();
        let report = ComputeUnitMatrixBencher::new().case(("case", &fixture)).execute().unwrap();

        let markdown = report.render_markdown();

        assert!(markdown.starts_with("# Compute Unit Matrix\n"));
        assert!(markdown.contains(&format!("## {BUILTIN_VARIANT_NAME}\n")));
        assert!(markdown.contains("| Name | Compute Units | Delta | Pass |"));
        assert!(markdown.contains("| case |"));
    }

    #[cfg(feature = "markdown")]
    #[test]
    fn an_empty_matrix_report_renders_just_the_header() {
        let report = MatrixReport {
            generated_at: String::from("now"),
            solana_runtime_version: String::from("4.3.0"),
            reports: BTreeMap::new(),
        };

        let markdown = report.render_markdown();

        assert!(markdown.contains("# Compute Unit Matrix"));
        assert!(!markdown.contains("## "));
    }

    // -------------------------------------------------------------------
    // helpers
    // -------------------------------------------------------------------

    fn bound_fixture(program_id: Address) -> crate::Fixture {
        bound_fixture_multi(program_id, Address::new_unique())
    }

    fn fixture_loader_id(_program_id: Address) -> Address {
        // Every binding in these fixtures uses the same synthetic loader id.
        static LOADER: [u8; 32] = [42; 32];
        Address::new_from_array(LOADER)
    }

    fn bound_fixture_multi(first: Address, second: Address) -> crate::Fixture {
        let mut fixture = passing_fixture();
        let crate::FixtureInput::Transaction(transaction) = &mut fixture.input else {
            unreachable!("transaction fixture");
        };
        transaction.programs = vec![
            ProgramBinding::new(first, fixture_loader_id(first), None),
            ProgramBinding::new(second, fixture_loader_id(second), None),
        ];
        fixture
    }

    fn blockhash_checked_fixture() -> crate::Fixture {
        let mut svm = hpsvm::HPSVM::new();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000).unwrap();
        let tx = VersionedTransaction::try_new(
            VersionedMessage::Legacy(Message::new(&[], Some(&payer.pubkey()))),
            &[&payer],
        )
        .unwrap();

        CaptureBuilder::new("checked")
            .runtime(RuntimeFixtureConfig::new(svm.block_env().slot, None, true, true))
            .baseline(empty_snapshot())
            .compares(Compare::everything())
            .capture_transaction(&tx)
            .expect("fixture capture must succeed")
    }
}
