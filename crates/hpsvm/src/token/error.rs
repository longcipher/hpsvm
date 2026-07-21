//! Error types for the token helper crate.

use solana_address::Address;
use solana_transaction_error::TransactionError;

use crate::types::FailedTransactionMetadata;

/// Errors returned by token helper operations.
#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    /// The requested account does not exist in the VM.
    #[error("account not found: {0}")]
    AccountNotFound(Address),

    /// The account data is too small to hold the expected token type.
    #[error(
        "account data too small for token type (expected at least {expected} bytes, got {actual})"
    )]
    AccountDataTooSmall {
        /// Minimum byte length required.
        expected: usize,
        /// Actual byte length of the account data.
        actual: usize,
    },

    /// The token account data could not be unpacked.
    #[error("failed to unpack token account data: {0}")]
    UnpackError(#[from] solana_program_error::ProgramError),

    /// A transaction submitted by a token helper failed.
    #[error("transaction failed: {}", .0.err)]
    TransactionFailed(Box<FailedTransactionMetadata>),
}

impl From<TokenError> for FailedTransactionMetadata {
    fn from(error: TokenError) -> Self {
        match error {
            TokenError::AccountNotFound(_) => {
                Self { err: TransactionError::AccountNotFound, meta: Default::default() }
            }
            TokenError::AccountDataTooSmall { .. } => Self {
                err: TransactionError::InstructionError(
                    0,
                    solana_instruction::error::InstructionError::AccountDataTooSmall,
                ),
                meta: Default::default(),
            },
            TokenError::UnpackError(e) => Self::from(e),
            TokenError::TransactionFailed(meta) => *meta,
        }
    }
}

impl From<FailedTransactionMetadata> for TokenError {
    fn from(meta: FailedTransactionMetadata) -> Self {
        Self::TransactionFailed(Box::new(meta))
    }
}
