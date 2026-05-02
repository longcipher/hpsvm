use std::path::{Path, PathBuf};

use hpsvm::HPSVM;
use hpsvm_fixture::{Fixture, FixtureRunner, ResultConfig};

use crate::{
    FixtureFormatArg,
    config::load_compares,
    error::CliError,
    program_map::{parse_program_map, preload_runner},
};

pub(crate) fn inspect_fixture(path: &Path, format: FixtureFormatArg) -> Result<(), CliError> {
    let fixture = load_fixture(path, format)?;
    println!("{}", serde_json::to_string_pretty(&fixture)?);
    Ok(())
}

pub(crate) fn run_fixture(
    path: &Path,
    format: FixtureFormatArg,
    program_args: &[String],
) -> Result<(), CliError> {
    let programs = parse_program_map(program_args)?;

    for fixture_path in fixture_paths(path, format)? {
        let fixture = load_fixture(&fixture_path, format)?;
        let mut runner = preload_runner(FixtureRunner::new(HPSVM::new()), &fixture, &programs);

        let pass =
            runner.run_and_validate(&fixture, &ResultConfig { panic: false, verbose: true })?;

        if pass {
            println!("PASS: {}", fixture.header.name);
            continue;
        }

        eprintln!("FAIL: {}", fixture.header.name);
        std::process::exit(1)
    }

    Ok(())
}

pub(crate) fn compare_fixture(
    path: &Path,
    format: FixtureFormatArg,
    baseline_program_args: &[String],
    candidate_program_args: &[String],
    config_path: Option<&Path>,
    ignore_compute_units: bool,
) -> Result<(), CliError> {
    let baseline_programs = parse_program_map(baseline_program_args)?;
    let candidate_programs = parse_program_map(candidate_program_args)?;

    for fixture_path in fixture_paths(path, format)? {
        let fixture = load_fixture(&fixture_path, format)?;
        let compares =
            load_compares(config_path, &fixture.expectations.compares, ignore_compute_units)?;

        let mut baseline_runner =
            preload_runner(FixtureRunner::new(HPSVM::new()), &fixture, &baseline_programs);
        let mut candidate_runner =
            preload_runner(FixtureRunner::new(HPSVM::new()), &fixture, &candidate_programs);

        let baseline_snapshot = baseline_runner.run(&fixture)?.snapshot;
        let candidate_snapshot = candidate_runner.run(&fixture)?.snapshot;

        let pass = baseline_snapshot.compare_with(
            &candidate_snapshot,
            &compares,
            &ResultConfig { panic: false, verbose: true },
        );

        if pass {
            println!("PASS: {}", fixture.header.name);
            continue;
        }

        eprintln!("FAIL: {}", fixture.header.name);
        std::process::exit(1)
    }

    Ok(())
}

fn fixture_paths(path: &Path, format: FixtureFormatArg) -> Result<Vec<PathBuf>, CliError> {
    if !path.is_dir() {
        return Ok(vec![path.to_path_buf()]);
    }

    let mut paths = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();
        if entry.file_type()?.is_file() && is_fixture_file(&entry_path, format) {
            paths.push(entry_path);
        }
    }
    paths.sort();

    if paths.is_empty() {
        return Err(CliError::NoFixturesInDirectory { path: path.display().to_string() });
    }

    Ok(paths)
}

fn is_fixture_file(path: &Path, format: FixtureFormatArg) -> bool {
    path.extension().and_then(std::ffi::OsStr::to_str).is_some_and(|extension| match format {
        FixtureFormatArg::Hpsvm => matches!(extension, "json" | "bin"),
        FixtureFormatArg::Firedancer => matches!(extension, "fix" | "json"),
    })
}

pub(crate) fn load_fixture(path: &Path, format: FixtureFormatArg) -> Result<Fixture, CliError> {
    match format {
        FixtureFormatArg::Hpsvm => Ok(Fixture::load(path)?),
        FixtureFormatArg::Firedancer => load_firedancer_fixture(path),
    }
}

#[cfg(feature = "fd-compat")]
fn load_firedancer_fixture(path: &Path) -> Result<Fixture, CliError> {
    Ok(hpsvm_fixture_fd::FiredancerFixture::load(path)?.try_into()?)
}

#[cfg(not(feature = "fd-compat"))]
fn load_firedancer_fixture(_path: &Path) -> Result<Fixture, CliError> {
    Err(CliError::FiredancerCompatDisabled)
}
