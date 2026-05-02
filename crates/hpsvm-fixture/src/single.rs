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
        FixtureInput::Instruction(instruction) => instruction.programs.clear(),
    }
    normalized
}
