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
                reports.insert(variant.name.clone(), report);
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
        FixtureInput::Instruction(instruction) => &instruction.programs,
    }
}
