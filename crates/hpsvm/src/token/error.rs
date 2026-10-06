//! Error types for the token helper crate.

use solana_address::Address;
use solana_transaction::InstructionError;
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
                err: TransactionError::InstructionError(0, InstructionError::AccountDataTooSmall),
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

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use solana_program_error::ProgramError;
    use solana_transaction::InstructionError;

    use super::*;

    fn failed(err: TransactionError) -> FailedTransactionMetadata {
        FailedTransactionMetadata { err, meta: Default::default() }
    }

    #[test]
    fn account_not_found_renders_the_address() {
        let address = Address::new_unique();
        let error = TokenError::AccountNotFound(address);
        assert_eq!(error.to_string(), format!("account not found: {address}"));
    }

    #[test]
    fn account_data_too_small_renders_both_lengths() {
        let error = TokenError::AccountDataTooSmall { expected: 165, actual: 10 };
        assert_eq!(
            error.to_string(),
            "account data too small for token type (expected at least 165 bytes, got 10)"
        );
    }

    #[test]
    fn unpack_error_renders_the_program_error() {
        let error = TokenError::UnpackError(ProgramError::InvalidAccountData);
        // The cause renders with `ProgramError`'s own display text, which is
        // not the same as its variant name.
        assert_eq!(
            error.to_string(),
            format!("failed to unpack token account data: {}", ProgramError::InvalidAccountData)
        );
        assert_ne!(
            error.to_string(),
            "failed to unpack token account data: InvalidAccountData",
            "the cause must be rendered, not the variant name"
        );
    }

    #[test]
    fn transaction_failed_renders_the_underlying_error() {
        let error =
            TokenError::TransactionFailed(Box::new(failed(TransactionError::AlreadyProcessed)));
        assert_eq!(
            error.to_string(),
            format!("transaction failed: {}", TransactionError::AlreadyProcessed)
        );
    }

    /// `TokenError::UnpackError` must be reachable through `?` in helper code.
    #[test]
    fn unpack_error_converts_from_a_program_error() {
        let error: TokenError = ProgramError::InvalidAccountData.into();
        match &error {
            TokenError::UnpackError(ProgramError::InvalidAccountData) => {}
            other => panic!("expected TokenError::UnpackError, got {other:?}"),
        }
    }

    #[test]
    fn account_not_found_maps_onto_the_transaction_error() {
        let error: FailedTransactionMetadata =
            TokenError::AccountNotFound(Address::new_unique()).into();
        assert!(matches!(error.err, TransactionError::AccountNotFound));
        assert_eq!(error.meta, Default::default());
    }

    #[test]
    fn account_data_too_small_maps_onto_the_instruction_error() {
        let error: FailedTransactionMetadata =
            TokenError::AccountDataTooSmall { expected: 165, actual: 3 }.into();
        match error.err {
            TransactionError::InstructionError(0, InstructionError::AccountDataTooSmall) => {}
            other => panic!("expected InstructionError(0, AccountDataTooSmall), got {other:?}"),
        }
    }

    #[test]
    fn unpack_error_maps_onto_the_program_error_conversion() {
        let error: FailedTransactionMetadata =
            TokenError::UnpackError(ProgramError::Custom(9)).into();
        match error.err {
            TransactionError::InstructionError(0, InstructionError::Custom(9)) => {}
            other => panic!("expected InstructionError(0, Custom(9)), got {other:?}"),
        }
    }

    #[test]
    fn transaction_failed_unwraps_to_the_original_metadata() {
        let original = failed(TransactionError::AccountInUse);
        let error: FailedTransactionMetadata =
            TokenError::TransactionFailed(Box::new(original.clone())).into();
        assert_eq!(error.err, original.err);
        assert_eq!(error.meta, original.meta);
    }

    /// The reverse conversion must box the metadata so large failures do not
    /// inflate `TokenError`'s size on the token helper hot path.
    #[test]
    fn failed_metadata_converts_into_a_boxed_transaction_failed() {
        let original = failed(TransactionError::BlockhashNotFound);
        let error: TokenError = original.clone().into();
        match error {
            TokenError::TransactionFailed(boxed) => {
                assert_eq!(boxed.err, original.err);
                assert_eq!(boxed.meta, original.meta);
            }
            other => panic!("expected TokenError::TransactionFailed, got {other:?}"),
        }
    }

    /// Round trip: metadata -> TokenError -> metadata must be lossless.
    #[test]
    fn failed_metadata_round_trips_through_token_error() {
        let original = FailedTransactionMetadata {
            err: TransactionError::InstructionError(2, InstructionError::Custom(77)),
            meta: Default::default(),
        };
        let round_tripped: FailedTransactionMetadata = TokenError::from(original.clone()).into();
        assert_eq!(round_tripped.err, original.err);
        assert_eq!(round_tripped.meta, original.meta);
    }

    #[test]
    fn failed_metadata_conversion_is_an_identity_for_every_error_kind() {
        let errors = [
            TransactionError::AlreadyProcessed,
            TransactionError::AccountInUse,
            TransactionError::BlockhashNotFound,
            TransactionError::AccountNotFound,
            TransactionError::InstructionError(1, InstructionError::Custom(1)),
            TransactionError::InsufficientFundsForRent { account_index: 3 },
            TransactionError::UnsupportedVersion,
        ];
        for err in errors {
            let expected = err.clone();
            let original = failed(err);
            let round_tripped: FailedTransactionMetadata =
                TokenError::from(original.clone()).into();
            assert_eq!(round_tripped.err, expected);
        }
    }

    // Property: any `ProgramError` survives `TokenError -> FailedTransactionMetadata`
    // as a `Custom` code equal to `u64::from(error) as u32`.
    proptest! {
        #[test]
        fn program_errors_survive_the_token_error_conversion(code in any::<u32>()) {
            let error: FailedTransactionMetadata =
                TokenError::UnpackError(ProgramError::Custom(code)).into();
            match error.err {
                TransactionError::InstructionError(0, InstructionError::Custom(actual)) => {
                    prop_assert_eq!(actual, code);
                }
                other => prop_assert!(false, "unexpected error {other:?}"),
            }
        }
    }

    #[test]
    fn token_error_is_not_too_large_to_pass_by_value() {
        // `ProgramError` is the widest payload. The exact size is pinned so that
        // adding a larger variant shows up as a hot-path regression here.
        const EXPECTED: usize = 40;
        assert_eq!(
            std::mem::size_of::<TokenError>(),
            EXPECTED,
            "TokenError is {} bytes",
            std::mem::size_of::<TokenError>()
        );
        assert!(
            std::mem::size_of::<TokenError>() >= std::mem::size_of::<ProgramError>(),
            "the enum must be at least as large as its widest payload"
        );
    }
}
