use hpsvm::types::{ExecutionOutcome, FailedTransactionMetadata, SimulatedTransactionInfo};
use solana_account::ReadableAccount;
use solana_address::Address;
use solana_message::inner_instruction::InnerInstructionsList;
use solana_transaction_context::TransactionReturnData;
use solana_transaction_error::{TransactionError, TransactionResult};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ExecutionSnapshot {
    pub status: ExecutionStatus,
    pub included: bool,
    pub compute_units_consumed: u64,
    pub fee: u64,
    pub logs: Vec<String>,
    pub return_data: Option<ReturnDataSnapshot>,
    pub inner_instructions: Vec<InnerInstructionSnapshot>,
    pub post_accounts: Vec<AccountSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionSnapshotFields {
    pub status: ExecutionStatus,
    pub included: bool,
    pub compute_units_consumed: u64,
    pub fee: u64,
    pub logs: Vec<String>,
    pub return_data: Option<ReturnDataSnapshot>,
    pub inner_instructions: Vec<InnerInstructionSnapshot>,
    pub post_accounts: Vec<AccountSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum ExecutionStatus {
    Success,
    Failure { kind: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct AccountSnapshot {
    pub address: Address,
    pub lamports: u64,
    pub owner: Address,
    pub executable: bool,
    pub rent_epoch: u64,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ReturnDataSnapshot {
    pub program_id: Address,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct InnerInstructionSnapshot {
    pub stack_height: u8,
    pub program_id_index: u8,
    pub accounts: Vec<u8>,
    pub data: Vec<u8>,
    pub outer_instruction_index: usize,
}

impl ExecutionSnapshot {
    pub fn from_fields(fields: ExecutionSnapshotFields) -> Self {
        let ExecutionSnapshotFields {
            status,
            included,
            compute_units_consumed,
            fee,
            logs,
            return_data,
            inner_instructions,
            post_accounts,
        } = fields;

        Self {
            status,
            included,
            compute_units_consumed,
            fee,
            logs,
            return_data,
            inner_instructions,
            post_accounts,
        }
    }

    pub fn from_outcome(outcome: &ExecutionOutcome) -> Self {
        Self {
            status: ExecutionStatus::from_result(outcome.status()),
            included: outcome.included(),
            compute_units_consumed: outcome.meta().compute_units_consumed,
            fee: outcome.meta().fee,
            logs: outcome.meta().logs.clone(),
            return_data: ReturnDataSnapshot::from_meta(&outcome.meta().return_data),
            inner_instructions: flatten_inner_instructions(&outcome.meta().inner_instructions),
            post_accounts: outcome
                .post_accounts()
                .iter()
                .map(|(address, account)| AccountSnapshot::from_readable(*address, account))
                .collect(),
        }
    }

    pub fn from_simulation(result: &SimulatedTransactionInfo) -> Self {
        Self {
            status: ExecutionStatus::Success,
            included: true,
            compute_units_consumed: result.meta.compute_units_consumed,
            fee: result.meta.fee,
            logs: result.meta.logs.clone(),
            return_data: ReturnDataSnapshot::from_meta(&result.meta.return_data),
            inner_instructions: flatten_inner_instructions(&result.meta.inner_instructions),
            post_accounts: result
                .post_accounts
                .iter()
                .map(|(address, account)| AccountSnapshot::from_readable(*address, account))
                .collect(),
        }
    }

    pub fn from_failed_simulation(error: &FailedTransactionMetadata) -> Self {
        Self {
            status: ExecutionStatus::from_error(&error.err),
            included: false,
            compute_units_consumed: error.meta.compute_units_consumed,
            fee: error.meta.fee,
            logs: error.meta.logs.clone(),
            return_data: ReturnDataSnapshot::from_meta(&error.meta.return_data),
            inner_instructions: flatten_inner_instructions(&error.meta.inner_instructions),
            post_accounts: Vec::new(),
        }
    }
}

impl ExecutionStatus {
    fn from_result(result: &TransactionResult<()>) -> Self {
        match result {
            Ok(()) => Self::Success,
            Err(error) => Self::from_error(error),
        }
    }

    fn from_error(error: &TransactionError) -> Self {
        Self::Failure { kind: format!("{error:?}"), message: error.to_string() }
    }
}

impl AccountSnapshot {
    pub fn new(
        address: Address,
        lamports: u64,
        owner: Address,
        executable: bool,
        rent_epoch: u64,
        data: Vec<u8>,
    ) -> Self {
        Self { address, lamports, owner, executable, rent_epoch, data }
    }

    pub fn from_readable(address: Address, account: &impl ReadableAccount) -> Self {
        Self {
            address,
            lamports: account.lamports(),
            owner: *account.owner(),
            executable: account.executable(),
            rent_epoch: account.rent_epoch(),
            data: account.data().to_vec(),
        }
    }
}

impl ReturnDataSnapshot {
    pub fn new(program_id: Address, data: Vec<u8>) -> Self {
        Self { program_id, data }
    }

    fn from_meta(return_data: &TransactionReturnData) -> Option<Self> {
        if return_data.data.is_empty() {
            None
        } else {
            Some(Self { program_id: return_data.program_id, data: return_data.data.clone() })
        }
    }
}

impl InnerInstructionSnapshot {
    pub fn new(
        stack_height: u8,
        program_id_index: u8,
        accounts: Vec<u8>,
        data: Vec<u8>,
        outer_instruction_index: usize,
    ) -> Self {
        Self { stack_height, program_id_index, accounts, data, outer_instruction_index }
    }
}

fn flatten_inner_instructions(groups: &InnerInstructionsList) -> Vec<InnerInstructionSnapshot> {
    groups
        .iter()
        .enumerate()
        .flat_map(|(outer_instruction_index, group)| {
            group.iter().map(move |inner| InnerInstructionSnapshot {
                stack_height: inner.stack_height,
                program_id_index: inner.instruction.program_id_index,
                accounts: inner.instruction.accounts.clone(),
                data: inner.instruction.data.clone(),
                outer_instruction_index,
            })
        })
        .collect()
}
