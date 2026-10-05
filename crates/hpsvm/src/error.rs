use solana_address::Address;
use solana_instruction::error::InstructionError;
use thiserror::Error;

use crate::AccountSourceError;

/// Errors related to invalid sysvar data
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum InvalidSysvarDataError {
    /// Invalid Clock sysvar data
    #[error("Invalid Clock sysvar data.")]
    Clock,
    /// Invalid EpochRewards sysvar data
    #[error("Invalid EpochRewards sysvar data.")]
    EpochRewards,
    /// Invalid EpochSchedule sysvar data
    #[error("Invalid EpochSchedule sysvar data.")]
    EpochSchedule,
    /// Invalid Fees sysvar data
    #[error("Invalid Fees sysvar data.")]
    Fees,
    /// Invalid LastRestartSlot sysvar data
    #[error("Invalid LastRestartSlot sysvar data.")]
    LastRestartSlot,
    /// Invalid RecentBlockhashes sysvar data
    #[error("Invalid RecentBlockhashes sysvar data.")]
    RecentBlockhashes,
    /// Invalid Rent sysvar data
    #[error("Invalid Rent sysvar data.")]
    Rent,
    /// Invalid SlotHashes sysvar data
    #[error("Invalid SlotHashes sysvar data.")]
    SlotHashes,
    /// Invalid StakeHistory sysvar data
    #[error("Invalid StakeHistory sysvar data.")]
    StakeHistory,
}

/// High level SVM errors
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum HPSVMError {
    /// Invalid sysvar data error
    #[error("{0}")]
    InvalidSysvarData(#[from] InvalidSysvarDataError),
    /// Sysvar serialization failure
    #[error("failed to serialize sysvar {sysvar}: {reason}")]
    SysvarSerialization { sysvar: &'static str, reason: String },
    /// Instruction error
    #[error("{0}")]
    Instruction(#[from] InstructionError),
    /// Invalid path error
    #[error("{0}")]
    InvalidPath(#[from] std::io::Error),
    /// Runtime environment refresh failure
    #[error("failed to refresh runtime environment {version}: {reason}")]
    RuntimeEnvironment { version: &'static str, reason: String },
    /// Custom syscall registration failure
    #[error("failed to register custom syscall {name} in {runtime}: {reason}")]
    CustomSyscallRegistration { name: String, runtime: &'static str, reason: String },
    /// Invalid loader error
    #[error("unsupported loader {loader_id} for program {program_id}")]
    InvalidLoader { program_id: Address, loader_id: Address },
    /// Required runtime component was not materialized.
    #[error("missing runtime component: {component}")]
    MissingRuntimeComponent { component: &'static str },
    /// External account source failure.
    #[error("account source failed while loading {pubkey}: {source}")]
    AccountSource {
        /// Account address that triggered the source read.
        pubkey: Address,
        /// Underlying account source error.
        source: AccountSourceError,
    },
    /// Program load failure.
    #[error("failed to load program: {0}")]
    ProgramLoad(String),
}

impl From<Box<dyn std::error::Error>> for HPSVMError {
    fn from(err: Box<dyn std::error::Error>) -> Self {
        Self::ProgramLoad(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use solana_instruction::error::InstructionError;

    use super::*;
    use crate::account_source::AccountSourceErrorKind;

    /// Every `InvalidSysvarDataError` variant must render a message that names
    /// the offending sysvar, and the `#[from]` conversion must preserve it.
    #[test]
    fn invalid_sysvar_data_error_variants_render_and_convert() {
        let cases: [(InvalidSysvarDataError, &str); 9] = [
            (InvalidSysvarDataError::Clock, "Invalid Clock sysvar data."),
            (InvalidSysvarDataError::EpochRewards, "Invalid EpochRewards sysvar data."),
            (InvalidSysvarDataError::EpochSchedule, "Invalid EpochSchedule sysvar data."),
            (InvalidSysvarDataError::Fees, "Invalid Fees sysvar data."),
            (InvalidSysvarDataError::LastRestartSlot, "Invalid LastRestartSlot sysvar data."),
            (InvalidSysvarDataError::RecentBlockhashes, "Invalid RecentBlockhashes sysvar data."),
            (InvalidSysvarDataError::Rent, "Invalid Rent sysvar data."),
            (InvalidSysvarDataError::SlotHashes, "Invalid SlotHashes sysvar data."),
            (InvalidSysvarDataError::StakeHistory, "Invalid StakeHistory sysvar data."),
        ];

        for (error, expected) in cases {
            assert_eq!(error.to_string(), expected);
            // `HPSVMError::InvalidSysvarData` delegates rendering to the inner error.
            let wrapped = HPSVMError::from(error);
            assert!(matches!(wrapped, HPSVMError::InvalidSysvarData(_)));
            assert_eq!(wrapped.to_string(), expected);
        }
    }

    #[test]
    fn sysvar_serialization_error_renders_sysvar_and_reason() {
        let error = HPSVMError::SysvarSerialization {
            sysvar: "EpochSchedule",
            reason: "slot index out of range".to_string(),
        };
        assert_eq!(
            error.to_string(),
            "failed to serialize sysvar EpochSchedule: slot index out of range"
        );
    }

    #[test]
    fn instruction_error_conversion_preserves_payload() {
        let error = HPSVMError::from(InstructionError::InvalidArgument);
        match &error {
            HPSVMError::Instruction(InstructionError::InvalidArgument) => {}
            other => panic!("expected HPSVMError::Instruction, got {other:?}"),
        }
        assert_eq!(error.to_string(), InstructionError::InvalidArgument.to_string());
    }

    #[test]
    fn invalid_path_error_converts_from_io_error() {
        let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "missing program.so");
        let error = HPSVMError::from(io_error);
        match &error {
            HPSVMError::InvalidPath(io) => assert_eq!(io.kind(), std::io::ErrorKind::NotFound),
            other => panic!("expected HPSVMError::InvalidPath, got {other:?}"),
        }
        assert_eq!(error.to_string(), "missing program.so");
    }

    #[test]
    fn runtime_environment_error_renders_version_and_reason() {
        let error = HPSVMError::RuntimeEnvironment {
            version: "v1",
            reason: "custom syscall replay failed".to_string(),
        };
        assert_eq!(
            error.to_string(),
            "failed to refresh runtime environment v1: custom syscall replay failed"
        );
    }

    #[test]
    fn custom_syscall_registration_error_renders_name_runtime_and_reason() {
        let error = HPSVMError::CustomSyscallRegistration {
            name: "sol_alloc_free_".to_string(),
            runtime: "runtime",
            reason: "duplicate registration".to_string(),
        };
        assert_eq!(
            error.to_string(),
            "failed to register custom syscall sol_alloc_free_ in runtime: duplicate registration"
        );
    }

    #[test]
    fn invalid_loader_error_renders_program_and_loader_ids() {
        let program_id = Address::new_unique();
        let loader_id = Address::new_unique();
        let error = HPSVMError::InvalidLoader { program_id, loader_id };
        assert_eq!(
            error.to_string(),
            format!("unsupported loader {loader_id} for program {program_id}")
        );
    }

    #[test]
    fn missing_runtime_component_error_renders_component() {
        let error = HPSVMError::MissingRuntimeComponent { component: "loader_cache" };
        assert_eq!(error.to_string(), "missing runtime component: loader_cache");
    }

    #[test]
    fn account_source_error_renders_pubkey_and_underlying_source() {
        let pubkey = Address::new_unique();
        let source = AccountSourceError::new("rpc endpoint unreachable");
        let error = HPSVMError::AccountSource { pubkey, source };

        assert_eq!(
            error.to_string(),
            format!(
                "account source failed while loading {pubkey}: Unavailable: rpc endpoint unreachable"
            )
        );
    }

    #[test]
    fn program_load_error_renders_message() {
        let error = HPSVMError::ProgramLoad("ELF header is truncated".to_string());
        assert_eq!(error.to_string(), "failed to load program: ELF header is truncated");
    }

    #[test]
    fn boxed_error_converts_to_program_load() {
        let boxed: Box<dyn std::error::Error> = Box::new(std::io::Error::other("bad magic"));
        let error = HPSVMError::from(boxed);
        match &error {
            HPSVMError::ProgramLoad(message) => assert_eq!(message, "bad magic"),
            other => panic!("expected HPSVMError::ProgramLoad, got {other:?}"),
        }
    }

    #[test]
    fn account_source_error_is_classifiable_through_hpsvm_error() {
        // `HPSVMError::AccountSource` must not erase the source error's kind.
        let source = AccountSourceError::with_kind(
            AccountSourceErrorKind::InvalidResponse,
            "malformed json",
        );
        let error = HPSVMError::AccountSource { pubkey: Address::new_unique(), source };

        match error {
            HPSVMError::AccountSource { source, .. } => {
                assert_eq!(source.kind(), AccountSourceErrorKind::InvalidResponse);
            }
            other => panic!("expected HPSVMError::AccountSource, got {other:?}"),
        }
    }
}
