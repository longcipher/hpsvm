use std::path::Path;

use hpsvm::HPSVM;
use hpsvm_fixture_bench::ComputeUnitBencher;

use crate::{
    FixtureFormatArg, error::CliError, fixture::load_fixture, program_map::parse_program_map,
};

pub(crate) fn report_compute_units(
    path: &Path,
    format: FixtureFormatArg,
    output_dir: &Path,
    baseline_dir: Option<&Path>,
    program_args: &[String],
    must_pass: bool,
) -> Result<(), CliError> {
    let fixture = load_fixture(path, format)?;
    let programs = parse_program_map(program_args)?;
    let case_name = fixture.header.name.clone();

    // The fixture's program bindings are resolved from the supplied ELFs, so
    // a binding without an ELF is an error rather than a silent skip.
    let mut bencher = ComputeUnitBencher::new(HPSVM::new())
        .with_program_elfs(programs)
        .case((case_name.as_str(), &fixture))
        .output_dir(output_dir)
        .must_pass(must_pass);

    if let Some(baseline_dir) = baseline_dir {
        bencher = bencher.baseline_dir(baseline_dir);
    }

    let report = bencher.execute()?;

    if report.rows.iter().all(|row| row.pass) {
        println!("PASS: {}", fixture.header.name);
        return Ok(());
    }

    eprintln!("FAIL: {}", fixture.header.name);
    Err(crate::error::CliError::ComparisonFailed { name: fixture.header.name })
}
