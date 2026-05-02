use hpsvm::error::HPSVMError;
use hpsvm_fixture::BenchError;
use thiserror::Error;

#[derive(Error, Debug)]
pub(crate) enum CliError {
    #[error(transparent)]
    Bench(#[from] BenchError),
    #[error(transparent)]
    Hpsvm(#[from] HPSVMError),
    #[error(transparent)]
    Fixture(#[from] hpsvm_fixture::FixtureError),
    #[cfg(feature = "fd-compat")]
    #[error(transparent)]
    Firedancer(#[from] hpsvm_fixture_fd::AdapterError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("failed to parse config {path}: {reason}")]
    ConfigParse { path: String, reason: String },
    #[error("unsupported config format for {path}")]
    UnsupportedConfigFormat { path: String },
    #[error("invalid --program value {value}, expected <program-id>=<path>")]
    InvalidProgramMapping { value: String },
    #[error("invalid program id {value}: {reason}")]
    InvalidProgramId { value: String, reason: String },
    #[error("no fixture files found in directory {path}")]
    NoFixturesInDirectory { path: String },
    #[cfg(not(feature = "fd-compat"))]
    #[error("firedancer fixture compatibility requires the hpsvm-cli `fd-compat` feature")]
    FiredancerCompatDisabled,
}
