use thiserror::Error;

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON codec error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("protobuf codec decode error: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("unsupported firedancer fixture format for {path}")]
    UnsupportedFormat { path: String },
    #[error("missing firedancer fixture field {field}")]
    MissingField { field: &'static str },
    #[error("invalid 32-byte address length for {field}: got {actual}")]
    InvalidAddressLength { field: &'static str, actual: usize },
    #[error("instruction account index {index} is out of range for {accounts_len} accounts")]
    InvalidInstructionAccountIndex { index: usize, accounts_len: usize },
    #[error("instruction account {address} is missing from pre_accounts")]
    MissingInstructionAccount { address: String },
    #[error("seed-derived account metadata is not supported in {field}")]
    UnsupportedSeedAddress { field: &'static str },
    #[error("firedancer compute units are inconsistent: before={before}, after={after}")]
    InconsistentComputeUnits { before: u64, after: u64 },
    #[error(
        "return data program {program_id} cannot be exported to firedancer fixture for instruction program {instruction_program_id}"
    )]
    UnsupportedReturnDataProgram { program_id: String, instruction_program_id: String },
    #[error("hpsvm fixture kind {kind} cannot be exported to firedancer; expected {expected}")]
    UnsupportedFixtureKind { kind: &'static str, expected: &'static str },
    #[error(
        "exporting hpsvm instruction fixtures requires the initial compute unit budget, which the canonical fixture model does not store"
    )]
    MissingComputeUnitBudget,
}
