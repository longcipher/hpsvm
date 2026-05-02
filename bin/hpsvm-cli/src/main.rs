#![allow(clippy::print_stderr, clippy::print_stdout, missing_docs)]

mod config;
mod cu;
mod error;
mod fixture;
mod program_map;

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::{
    cu::report_compute_units,
    fixture::{compare_fixture, inspect_fixture, run_fixture},
};

#[derive(Parser)]
#[command(name = "hpsvm")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Cu(CuArgs),
    Fixture(FixtureArgs),
}

#[derive(Args)]
struct CuArgs {
    #[command(subcommand)]
    command: CuCommand,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum FixtureFormatArg {
    #[default]
    Hpsvm,
    Firedancer,
}

#[derive(Subcommand)]
enum CuCommand {
    Report {
        fixture: PathBuf,
        #[arg(long = "fixture-format", value_enum, default_value_t)]
        fixture_format: FixtureFormatArg,
        #[arg(long)]
        output_dir: PathBuf,
        #[arg(long)]
        baseline_dir: Option<PathBuf>,
        #[arg(long = "program")]
        programs: Vec<String>,
        #[arg(long)]
        must_pass: bool,
    },
}

#[derive(Args)]
struct FixtureArgs {
    #[command(subcommand)]
    command: FixtureCommand,
}

#[derive(Subcommand)]
enum FixtureCommand {
    Inspect {
        fixture: PathBuf,
        #[arg(long = "fixture-format", value_enum, default_value_t)]
        fixture_format: FixtureFormatArg,
    },
    Run {
        fixture: PathBuf,
        #[arg(long = "fixture-format", value_enum, default_value_t)]
        fixture_format: FixtureFormatArg,
        #[arg(long = "program")]
        programs: Vec<String>,
    },
    Compare {
        fixture: PathBuf,
        #[arg(long = "fixture-format", value_enum, default_value_t)]
        fixture_format: FixtureFormatArg,
        #[arg(long = "baseline-program")]
        baseline_programs: Vec<String>,
        #[arg(long = "candidate-program")]
        candidate_programs: Vec<String>,
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        ignore_compute_units: bool,
    },
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), error::CliError> {
    match Cli::parse().command {
        Command::Cu(args) => match args.command {
            CuCommand::Report {
                fixture,
                fixture_format,
                output_dir,
                baseline_dir,
                programs,
                must_pass,
            } => report_compute_units(
                &fixture,
                fixture_format,
                &output_dir,
                baseline_dir.as_deref(),
                &programs,
                must_pass,
            ),
        },
        Command::Fixture(args) => match args.command {
            FixtureCommand::Inspect { fixture, fixture_format } => {
                inspect_fixture(&fixture, fixture_format)
            }
            FixtureCommand::Run { fixture, fixture_format, programs } => {
                run_fixture(&fixture, fixture_format, &programs)
            }
            FixtureCommand::Compare {
                fixture,
                fixture_format,
                baseline_programs,
                candidate_programs,
                config,
                ignore_compute_units,
            } => compare_fixture(
                &fixture,
                fixture_format,
                &baseline_programs,
                &candidate_programs,
                config.as_deref(),
                ignore_compute_units,
            ),
        },
    }
}
