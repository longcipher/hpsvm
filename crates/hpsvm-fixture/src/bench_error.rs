use std::{io, path::PathBuf};

use solana_address::Address;
use thiserror::Error;

use crate::FixtureError;

#[derive(Debug, Error)]
pub enum BenchError {
    #[error("bench requires at least one fixture case")]
    MissingCases,
    #[error("fixture bench case `{name}` failed: {source}")]
    Fixture {
        name: String,
        #[source]
        source: FixtureError,
    },
    #[error("fixture bench case `{name}` did not satisfy its expectations")]
    ExpectationFailed { name: String },
    #[error("matrix program variant `{name}` was added more than once")]
    DuplicateProgramName { name: String },
    #[error(
        "matrix variant `{name}` supplied program {program_id} for fixture bench case `{case}`, but that program is not bound in the fixture"
    )]
    UnboundVariantProgram { case: String, name: String, program_id: Address },
    #[error(
        "matrix variant `{name}` did not supply fixture-bound program {program_id} with loader {loader_id} for fixture bench case `{case}`"
    )]
    MissingVariantProgram { case: String, name: String, program_id: Address, loader_id: Address },
    #[error(
        "fixture bench case `{case}` binds program {program_id} with loader {fixture_loader_id}, which does not match matrix variant loader {variant_loader_id}"
    )]
    ProgramLoaderMismatch {
        case: String,
        program_id: Address,
        fixture_loader_id: Address,
        variant_loader_id: Address,
    },
    #[error("failed to create report output directory {path:?}: {source}")]
    CreateOutputDir {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to write report to {path:?}: {source}")]
    WriteReport {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to read baseline report from {path:?}: {source}")]
    ReadBaseline {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid baseline report {path:?}: {reason}")]
    InvalidBaseline { path: PathBuf, reason: String },
    #[error("report I/O via `{operation}` requires the `markdown` feature")]
    ReportIoDisabled { operation: &'static str },
}
