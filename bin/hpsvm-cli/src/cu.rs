use std::{collections::HashMap, path::Path};

use hpsvm::HPSVM;
use hpsvm_fixture::{ComputeUnitBencher, Fixture, FixtureError};
use solana_address::Address;

use crate::{
    FixtureFormatArg,
    error::CliError,
    fixture::load_fixture,
    program_map::{fixture_programs, parse_program_map},
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
    let vm = preload_vm(HPSVM::new(), &fixture, &programs)?;
    let case_name = fixture.header.name.clone();

    let mut bencher = ComputeUnitBencher::new(vm)
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
    std::process::exit(1)
}

fn preload_vm(
    mut vm: HPSVM,
    fixture: &Fixture,
    programs: &HashMap<Address, Vec<u8>>,
) -> Result<HPSVM, CliError> {
    for binding in fixture_programs(fixture) {
        let Some(bytes) = programs.get(&binding.program_id) else {
            return Err(FixtureError::MissingProgramElf { program_id: binding.program_id }.into());
        };
        vm.add_program_with_loader(binding.program_id, bytes, binding.loader_id)?;
    }

    Ok(vm)
}
