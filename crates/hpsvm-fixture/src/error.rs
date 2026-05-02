use hpsvm::error::HPSVMError;
use solana_address::Address;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum FixtureError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[cfg(feature = "json-codec")]
    #[error("JSON codec error: {0}")]
    Json(#[from] serde_json::Error),
    #[cfg(feature = "bin-codec")]
    #[error("binary codec encode error: {0}")]
    EncodeFixture(Box<bincode::ErrorKind>),
    #[cfg(feature = "bin-codec")]
    #[error("binary codec decode error: {0}")]
    DecodeFixture(Box<bincode::ErrorKind>),
    #[error("failed to encode transaction: {0}")]
    EncodeTransaction(Box<bincode::ErrorKind>),
    #[error("failed to decode transaction: {0}")]
    DecodeTransaction(Box<bincode::ErrorKind>),
    #[error("unsupported fixture format for {path}")]
    UnsupportedFormat { path: String },
    #[error("runtime config field {field} is not replayable with the current hpsvm API")]
    UnsupportedRuntimeConfig { field: &'static str },
    #[error("missing required field {field}")]
    MissingField { field: &'static str },
    #[error("missing program ELF for {program_id}")]
    MissingProgramElf { program_id: Address },
    #[error(transparent)]
    Hpsvm(#[from] HPSVMError),
}
