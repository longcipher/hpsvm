use solana_account::{Account, AccountSharedData};
use solana_address::Address;
use solana_instruction::{Instruction, account_meta::AccountMeta, error::InstructionError};
use solana_message::inner_instruction::InnerInstructionsList;
use solana_program_error::ProgramError;
use solana_signature::Signature;
use solana_transaction_context::TransactionReturnData;
use solana_transaction_error::{TransactionError, TransactionResult as Result};

use crate::{error::HPSVMError, format_logs::format_logs};

/// Transaction metadata captured during execution.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TransactionMetadata {
    /// The transaction signature.
    #[cfg_attr(feature = "serde", serde(with = "crate::utils::serde_with_str"))]
    pub signature: Signature,
    /// Transaction log messages.
    pub logs: Vec<String>,
    /// Inner (CPI) instructions executed during the transaction.
    pub inner_instructions: InnerInstructionsList,
    /// Compute units consumed by the transaction.
    pub compute_units_consumed: u64,
    /// Data returned by programs during execution.
    pub return_data: TransactionReturnData,
    /// Transaction fee in lamports.
    pub fee: u64,
    /// Execution diagnostics including balance diffs, token balances, and trace.
    #[cfg_attr(feature = "serde", serde(default))]
    pub diagnostics: ExecutionDiagnostics,
}

impl TransactionMetadata {
    /// Returns the transaction logs formatted with ANSI color codes.
    pub fn pretty_logs(&self) -> String {
        format_logs(&self.logs)
    }
}

/// Structured execution details captured alongside transaction metadata.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ExecutionDiagnostics {
    /// Account lamport balances before execution.
    pub pre_balances: Vec<u64>,
    /// Account lamport balances after execution.
    pub post_balances: Vec<u64>,
    /// Per-account state diffs for writable accounts.
    pub account_diffs: Vec<AccountDiff>,
    /// Account source failures observed during execution preparation.
    pub account_source_failures: Vec<AccountSourceFailure>,
    /// SPL token balances before execution.
    pub pre_token_balances: Vec<TokenBalance>,
    /// SPL token balances after execution.
    pub post_token_balances: Vec<TokenBalance>,
    /// Execution trace of CPI instruction frames.
    pub execution_trace: ExecutionTrace,
}

/// External account source failures observed while preparing execution.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct AccountSourceFailure {
    /// The account address that failed.
    pub pubkey: Address,
    /// Description of the failure.
    pub error: String,
}

/// Pre/post account state for a writable account touched by execution.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct AccountDiff {
    /// The account address.
    pub address: Address,
    /// Account state before execution, if the account existed.
    pub pre: Option<Account>,
    /// Account state after execution.
    pub post: Option<Account>,
}

/// SPL token balance metadata for a token account present in execution diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct TokenBalance {
    /// Index of the token account in the transaction.
    pub account_index: usize,
    /// Address of the token account.
    pub address: Address,
    /// Address of the token mint.
    pub mint: Address,
    /// Address of the token account owner.
    pub owner: Address,
    /// Token balance amount.
    pub amount: u64,
    /// Token decimals, if known.
    pub decimals: Option<u8>,
}

/// Instruction trace frames captured directly from the Solana transaction context.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ExecutionTrace {
    /// The executed instructions in order.
    pub instructions: Vec<ExecutedInstruction>,
}

/// One executed top-level or CPI instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ExecutedInstruction {
    /// Call stack depth (0 for top-level instructions).
    pub stack_height: u8,
    /// Address of the program that was invoked.
    pub program_id: Address,
    /// Account metas passed to the instruction.
    pub accounts: Vec<AccountMeta>,
    /// Raw instruction data.
    pub data: Vec<u8>,
}

impl ExecutedInstruction {
    /// Returns this trace frame as a normal Solana instruction.
    #[must_use]
    pub fn instruction(&self) -> Instruction {
        Instruction {
            program_id: self.program_id,
            accounts: self.accounts.clone(),
            data: self.data.clone(),
        }
    }
}

/// Result of [`crate::HPSVM::transact`].
///
/// Each outcome is tied to the VM instance and state version that produced it.
/// [`crate::HPSVM::commit_transaction`] only accepts outcomes from that same
/// instance before any intervening state or config mutation. Otherwise commit
/// returns `ResanitizationNeeded`.
///
/// The provenance fields stay internal and are skipped from serialization, so
/// serialized outcomes are observational only and cannot be committed on a
/// foreign or later-mutated VM.
#[must_use = "call HPSVM::commit_transaction to apply this outcome to the VM"]
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ExecutionOutcome {
    pub(crate) meta: TransactionMetadata,
    pub(crate) post_accounts: Vec<(Address, AccountSharedData)>,
    pub(crate) status: Result<()>,
    pub(crate) included: bool,
    #[cfg_attr(feature = "serde", serde(skip_serializing))]
    pub(crate) origin_vm_instance_id: u64,
    #[cfg_attr(feature = "serde", serde(skip_serializing))]
    pub(crate) origin_state_version: u64,
    #[cfg_attr(feature = "serde", serde(skip_serializing, skip_deserializing))]
    pub(crate) fee_payer: Option<Address>,
}

impl ExecutionOutcome {
    /// Returns the transaction metadata captured during execution.
    pub fn meta(&self) -> &TransactionMetadata {
        &self.meta
    }

    /// Returns the writable post-execution account snapshot captured by `transact`.
    pub fn post_accounts(&self) -> &[(Address, AccountSharedData)] {
        &self.post_accounts
    }

    /// Returns the execution status that `commit_transaction` will commit if
    /// the outcome provenance still matches the target VM.
    pub fn status(&self) -> &Result<()> {
        &self.status
    }

    /// Returns whether this outcome is eligible for commit-time side effects.
    pub fn included(&self) -> bool {
        self.included
    }

    /// Returns the fee payer address, if one was validated for this transaction.
    pub fn fee_payer(&self) -> Option<Address> {
        self.fee_payer
    }
}

/// Simulated transaction information including metadata and post-execution accounts.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SimulatedTransactionInfo {
    /// Transaction metadata.
    pub meta: TransactionMetadata,
    /// Post-execution writable account snapshot.
    pub post_accounts: Vec<(Address, AccountSharedData)>,
}

/// Metadata for a failed transaction including the error and logs.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FailedTransactionMetadata {
    /// The transaction error.
    pub err: TransactionError,
    /// Transaction metadata captured before failure.
    pub meta: TransactionMetadata,
}

impl From<ProgramError> for FailedTransactionMetadata {
    fn from(value: ProgramError) -> Self {
        Self {
            err: TransactionError::InstructionError(
                0,
                InstructionError::Custom(u64::from(value) as u32),
            ),
            meta: Default::default(),
        }
    }
}

/// A result type that either holds successful transaction metadata or failure metadata.
pub type TransactionResult = std::result::Result<TransactionMetadata, FailedTransactionMetadata>;

#[derive(Debug)]
pub(crate) struct ExecutionResult {
    pub(crate) post_accounts: Vec<(Address, AccountSharedData)>,
    pub(crate) tx_result: Result<()>,
    pub(crate) signature: Signature,
    pub(crate) compute_units_consumed: u64,
    pub(crate) inner_instructions: InnerInstructionsList,
    pub(crate) return_data: TransactionReturnData,
    pub(crate) execution_trace: ExecutionTrace,
    /// Whether the transaction can be included in a block
    pub(crate) included: bool,
    pub(crate) fee: u64,
    pub(crate) fee_payer: Option<Address>,
    pub(crate) account_source_failures: Vec<AccountSourceFailure>,
    pub(crate) fatal_error: Option<HPSVMError>,
}

impl Default for ExecutionResult {
    fn default() -> Self {
        Self {
            post_accounts: Default::default(),
            tx_result: Err(TransactionError::UnsupportedVersion),
            signature: Default::default(),
            compute_units_consumed: Default::default(),
            inner_instructions: Default::default(),
            return_data: Default::default(),
            execution_trace: Default::default(),
            included: false,
            fee: 0,
            fee_payer: None,
            account_source_failures: Default::default(),
            fatal_error: None,
        }
    }
}
