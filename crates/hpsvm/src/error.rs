use solana_instruction::error::InstructionError;
use thiserror::Error;

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
    /// Instruction error
    #[error("{0}")]
    Instruction(#[from] InstructionError),
    /// Invalid path error
    #[error("{0}")]
    InvalidPath(#[from] std::io::Error),
    /// Invalid loader error
    #[error("{0}")]
    InvalidLoader(String),
}
