use std::{io, path::PathBuf};

use hpsvm_fixture::FixtureError;
use solana_address::Address;
use thiserror::Error;

/// A failure while benchmarking a fixture.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BenchError {
    /// The bencher was asked to run without a single case.
    #[error("bench requires at least one fixture case")]
    MissingCases,
    /// A fixture could not be executed.
    #[error("fixture bench case `{name}` failed: {source}")]
    Fixture {
        /// The name of the case.
        name: String,
        /// The underlying fixture error.
        #[source]
        source: FixtureError,
    },
    /// A fixture executed but did not satisfy its expectations, and the
    /// bencher was configured to require them.
    #[error("fixture bench case `{name}` did not satisfy its expectations")]
    ExpectationFailed {
        /// The name of the case.
        name: String,
    },
    /// Two program variants share a name.
    #[error("matrix program variant `{name}` was added more than once")]
    DuplicateProgramName {
        /// The duplicated variant name.
        name: String,
    },
    /// A variant supplies a program the fixture does not bind.
    #[error(
        "matrix variant `{name}` supplied program {program_id} for fixture bench case `{case}`, but that program is not bound in the fixture"
    )]
    UnboundVariantProgram {
        /// The name of the case.
        case: String,
        /// The name of the variant.
        name: String,
        /// The program the variant supplied.
        program_id: Address,
    },
    /// A variant does not supply a program the fixture binds.
    #[error(
        "matrix variant `{name}` did not supply fixture-bound program {program_id} with loader {loader_id} for fixture bench case `{case}`"
    )]
    MissingVariantProgram {
        /// The name of the case.
        case: String,
        /// The name of the variant.
        name: String,
        /// The program the fixture binds.
        program_id: Address,
        /// The loader the fixture binds it with.
        loader_id: Address,
    },
    /// A variant supplies a program with a different loader than the fixture
    /// binds it with.
    #[error(
        "fixture bench case `{case}` binds program {program_id} with loader {fixture_loader_id}, which does not match matrix variant loader {variant_loader_id}"
    )]
    ProgramLoaderMismatch {
        /// The name of the case.
        case: String,
        /// The program in question.
        program_id: Address,
        /// The loader the fixture binds it with.
        fixture_loader_id: Address,
        /// The loader the variant supplies it with.
        variant_loader_id: Address,
    },
    /// The report's output directory could not be created.
    #[error("failed to create report output directory {path:?}: {source}")]
    CreateOutputDir {
        /// The directory that could not be created.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// The report could not be written.
    #[error("failed to write report to {path:?}: {source}")]
    WriteReport {
        /// The file that could not be written.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// The baseline report could not be read.
    #[error("failed to read baseline report from {path:?}: {source}")]
    ReadBaseline {
        /// The file that could not be read.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// The baseline report exists but could not be understood.
    #[error("invalid baseline report {path:?}: {reason}")]
    InvalidBaseline {
        /// The file that could not be understood.
        path: PathBuf,
        /// Why it could not be understood.
        reason: String,
    },
    /// The operation needs the `report-io` feature, which is not enabled.
    #[error("report I/O via `{operation}` requires the `report-io` feature")]
    ReportIoDisabled {
        /// The operation that was attempted.
        operation: &'static str,
    },
}
