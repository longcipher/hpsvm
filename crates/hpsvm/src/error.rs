use solana_address::Address;
use solana_instruction::error::InstructionError;
use thiserror::Error;

use crate::AccountSourceError;

/// Errors related to invalid sysvar data
#[derive(Error, Debug)]
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
}
