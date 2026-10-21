use hpsvm::error::HPSVMError;
use solana_address::Address;
use thiserror::Error;

/// A failure while loading, saving or executing a fixture.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum FixtureError {
    /// A file could not be read or written.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The JSON form of the fixture could not be decoded.
    #[cfg(feature = "json-codec")]
    #[error("JSON codec error: {0}")]
    Json(#[from] serde_json::Error),
    /// The binary form of the fixture could not be encoded.
    #[cfg(feature = "bin-codec")]
    #[error("binary codec encode error: {0}")]
    EncodeFixture(#[from] wincode::WriteError),
    /// The binary form of the fixture could not be decoded.
    #[cfg(feature = "bin-codec")]
    #[error("binary codec decode error: {0}")]
    DecodeFixture(#[from] wincode::ReadError),
    /// The transaction could not be encoded.
    #[cfg(feature = "bin-codec")]
    #[error("failed to encode transaction: {0}")]
    EncodeTransaction(wincode::WriteError),
    /// The transaction could not be decoded.
    #[cfg(feature = "bin-codec")]
    #[error("failed to decode transaction: {0}")]
    DecodeTransaction(wincode::ReadError),
    /// The operation needs a codec feature that is not enabled.
    #[error("fixture codec feature `{feature}` is not enabled")]
    CodecDisabled {
        /// The feature that is not enabled.
        feature: &'static str,
    },
    /// The file extension does not name a fixture format.
    #[error("unsupported fixture format for {path}")]
    UnsupportedFormat {
        /// The path that was rejected.
        path: String,
    },
    /// The runtime configuration cannot be replayed.
    #[error("runtime config field {field} is not replayable with the current hpsvm API")]
    UnsupportedRuntimeConfig {
        /// The name of the field that cannot be replayed.
        field: &'static str,
    },
    /// A field the fixture requires is absent.
    #[error("missing required field {field}")]
    MissingField {
        /// The name of the missing field.
        field: &'static str,
    },
    /// A program the fixture binds has no ELF.
    #[error("missing program ELF for {program_id}")]
    MissingProgramElf {
        /// The program that has no ELF.
        program_id: Address,
    },
    /// The fixture was written in a format version this build cannot read.
    #[error("unsupported fixture schema version {version}; this build reads version {supported}")]
    UnsupportedSchemaVersion {
        /// The version the fixture declares.
        version: u16,
        /// The version this build reads.
        supported: u16,
    },
    /// The VM reported a failure.
    #[error(transparent)]
    Hpsvm(#[from] HPSVMError),
}
