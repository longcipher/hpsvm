use solana_account::{Account, AccountSharedData};
use solana_address::Address;
use solana_instruction::{Instruction, account_meta::AccountMeta, error::InstructionError};
use solana_message::inner_instruction::InnerInstructionsList;
use solana_program_error::ProgramError;
use solana_signature::Signature;
use solana_transaction_context::TransactionReturnData;
use solana_transaction_error::{TransactionError, TransactionResult as Result};

use crate::format_logs::format_logs;

#[expect(missing_docs)]
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TransactionMetadata {
    #[cfg_attr(feature = "serde", serde(with = "crate::utils::serde_with_str"))]
    pub signature: Signature,
    pub logs: Vec<String>,
    pub inner_instructions: InnerInstructionsList,
    pub compute_units_consumed: u64,
    pub return_data: TransactionReturnData,
    pub fee: u64,
    #[cfg_attr(feature = "serde", serde(default))]
    pub diagnostics: ExecutionDiagnostics,
}

impl TransactionMetadata {
    #[expect(missing_docs)]
    pub fn pretty_logs(&self) -> String {
        format_logs(&self.logs)
    }
}

/// Structured execution details captured alongside transaction metadata.
#[expect(missing_docs)]
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ExecutionDiagnostics {
    pub pre_balances: Vec<u64>,
    pub post_balances: Vec<u64>,
    pub account_diffs: Vec<AccountDiff>,
    pub pre_token_balances: Vec<TokenBalance>,
    pub post_token_balances: Vec<TokenBalance>,
    pub execution_trace: ExecutionTrace,
}

/// Pre/post account state for a writable account touched by execution.
#[expect(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct AccountDiff {
    pub address: Address,
    pub pre: Option<Account>,
    pub post: Option<Account>,
}

/// SPL token balance metadata for a token account present in execution diagnostics.
#[expect(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct TokenBalance {
    pub account_index: usize,
    pub address: Address,
    pub mint: Address,
    pub owner: Address,
    pub amount: u64,
    pub decimals: Option<u8>,
}

/// Instruction trace frames captured directly from the Solana transaction context.
#[expect(missing_docs)]
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ExecutionTrace {
    pub instructions: Vec<ExecutedInstruction>,
}

/// One executed top-level or CPI instruction.
#[expect(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ExecutedInstruction {
    pub stack_height: u8,
    pub program_id: Address,
    pub accounts: Vec<AccountMeta>,
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
}

#[expect(missing_docs)]
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SimulatedTransactionInfo {
    #[expect(missing_docs)]
    pub meta: TransactionMetadata,
    #[expect(missing_docs)]
    pub post_accounts: Vec<(Address, AccountSharedData)>,
}

#[expect(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FailedTransactionMetadata {
    #[expect(missing_docs)]
    pub err: TransactionError,
    #[expect(missing_docs)]
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

#[expect(missing_docs)]
pub type TransactionResult = std::result::Result<TransactionMetadata, FailedTransactionMetadata>;

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
        }
    }
}
