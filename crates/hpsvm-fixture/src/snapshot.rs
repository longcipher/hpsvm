use hpsvm::types::{ExecutionOutcome, FailedTransactionMetadata, SimulatedTransactionInfo};
use solana_account::ReadableAccount;
use solana_address::Address;
use solana_message::inner_instruction::InnerInstructionsList;
use solana_transaction_context::transaction::TransactionReturnData;
use solana_transaction_error::{TransactionError, TransactionResult};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bin-codec", derive(wincode::SchemaWrite, wincode::SchemaRead))]
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
#[cfg_attr(feature = "bin-codec", derive(wincode::SchemaWrite, wincode::SchemaRead))]
#[non_exhaustive]
pub enum ExecutionStatus {
    Success,
    Failure { kind: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bin-codec", derive(wincode::SchemaWrite, wincode::SchemaRead))]
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
#[cfg_attr(feature = "bin-codec", derive(wincode::SchemaWrite, wincode::SchemaRead))]
#[non_exhaustive]
pub struct ReturnDataSnapshot {
    pub program_id: Address,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bin-codec", derive(wincode::SchemaWrite, wincode::SchemaRead))]
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

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use solana_message::{
        compiled_instruction::CompiledInstruction, inner_instruction::InnerInstruction,
    };

    use super::*;

    fn address(seed: u8) -> Address {
        Address::new_from_array([seed; 32])
    }

    fn metadata(logs: &[&str], compute_units: u64, fee: u64) -> hpsvm::types::TransactionMetadata {
        hpsvm::types::TransactionMetadata {
            logs: logs.iter().map(|line| (*line).to_string()).collect(),
            compute_units_consumed: compute_units,
            fee,
            ..Default::default()
        }
    }

    fn inner(program_id_index: u8, data: Vec<u8>, stack_height: u8) -> InnerInstruction {
        InnerInstruction {
            instruction: CompiledInstruction::new_from_raw_parts(
                program_id_index,
                data,
                vec![0, 1],
            ),
            stack_height,
        }
    }

    #[test]
    fn from_fields_is_a_field_for_field_copy() {
        let fields = ExecutionSnapshotFields {
            status: ExecutionStatus::Success,
            included: true,
            compute_units_consumed: 10,
            fee: 20,
            logs: vec![String::from("a")],
            return_data: Some(ReturnDataSnapshot::new(address(1), vec![2])),
            inner_instructions: Vec::new(),
            post_accounts: Vec::new(),
        };
        let snapshot = ExecutionSnapshot::from_fields(fields.clone());

        assert_eq!(snapshot.status, ExecutionStatus::Success);
        assert!(snapshot.included);
        assert_eq!(snapshot.compute_units_consumed, 10);
        assert_eq!(snapshot.fee, 20);
        assert_eq!(snapshot.logs, vec![String::from("a")]);
        assert_eq!(snapshot.return_data, Some(ReturnDataSnapshot::new(address(1), vec![2])));
        assert_eq!(
            ExecutionSnapshot::from_fields(fields),
            snapshot,
            "from_fields must be deterministic"
        );
    }

    #[test]
    fn from_failed_simulation_records_the_failure_and_drops_post_accounts() {
        let error = FailedTransactionMetadata {
            err: TransactionError::AccountNotFound,
            meta: metadata(&["Program log: partial"], 123, 45),
        };
        let snapshot = ExecutionSnapshot::from_failed_simulation(&error);

        assert!(!snapshot.included);
        assert_eq!(snapshot.compute_units_consumed, 123);
        assert_eq!(snapshot.fee, 45);
        assert_eq!(snapshot.logs, vec![String::from("Program log: partial")]);
        assert!(snapshot.post_accounts.is_empty());
        match &snapshot.status {
            ExecutionStatus::Failure { kind, message } => {
                assert_eq!(kind, &format!("{:?}", TransactionError::AccountNotFound));
                assert_eq!(message, &TransactionError::AccountNotFound.to_string());
            }
            other => panic!("expected ExecutionStatus::Failure, got {other:?}"),
        }
    }

    #[test]
    fn from_failed_simulation_of_an_empty_failure_is_still_a_failure() {
        let error = FailedTransactionMetadata {
            err: TransactionError::BlockhashNotFound,
            meta: Default::default(),
        };
        let snapshot = ExecutionSnapshot::from_failed_simulation(&error);

        assert!(!snapshot.included);
        assert_eq!(snapshot.compute_units_consumed, 0);
        assert!(snapshot.logs.is_empty());
        assert!(snapshot.return_data.is_none());
        assert!(snapshot.inner_instructions.is_empty());
        assert!(snapshot.post_accounts.is_empty());
        assert!(matches!(snapshot.status, ExecutionStatus::Failure { .. }));
    }

    /// `from_failed_simulation` must surface the error kind for every kind of
    /// transaction error, not just account-not-found.
    #[test]
    fn from_failed_simulation_records_the_debug_kind_for_each_error() {
        let errors = [
            TransactionError::AlreadyProcessed,
            TransactionError::AccountInUse,
            TransactionError::BlockhashNotFound,
            TransactionError::InsufficientFundsForRent { account_index: 2 },
            TransactionError::UnsupportedVersion,
        ];
        for err in errors {
            // `kind` is the `Debug` rendering and `message` the `Display`
            // rendering; capture both before the value is moved.
            let expected_kind = format!("{err:?}");
            let expected_message = err.to_string();
            let error = FailedTransactionMetadata { err, meta: Default::default() };
            let snapshot = ExecutionSnapshot::from_failed_simulation(&error);
            match snapshot.status {
                ExecutionStatus::Failure { kind, message } => {
                    assert_eq!(kind, expected_kind);
                    assert_eq!(message, expected_message);
                }
                other => panic!("expected ExecutionStatus::Failure, got {other:?}"),
            }
        }
    }

    #[test]
    fn from_simulation_always_reports_success_and_inclusion() {
        let result = SimulatedTransactionInfo {
            meta: metadata(&["Program log: ok"], 77, 5_000),
            post_accounts: Vec::new(),
        };
        let snapshot = ExecutionSnapshot::from_simulation(&result);

        assert_eq!(snapshot.status, ExecutionStatus::Success);
        assert!(snapshot.included);
        assert_eq!(snapshot.compute_units_consumed, 77);
        assert_eq!(snapshot.fee, 5_000);
    }

    #[test]
    fn account_snapshot_new_stores_every_field() {
        let snapshot = AccountSnapshot::new(address(1), 42, address(2), true, 7, vec![9, 9]);
        assert_eq!(snapshot.address, address(1));
        assert_eq!(snapshot.lamports, 42);
        assert_eq!(snapshot.owner, address(2));
        assert!(snapshot.executable);
        assert_eq!(snapshot.rent_epoch, 7);
        assert_eq!(snapshot.data, vec![9, 9]);
    }

    #[test]
    fn return_data_snapshot_new_stores_the_program_and_payload() {
        let snapshot = ReturnDataSnapshot::new(address(3), vec![1, 2]);
        assert_eq!(snapshot.program_id, address(3));
        assert_eq!(snapshot.data, vec![1, 2]);
    }

    #[test]
    fn return_data_from_meta_is_none_for_empty_data() {
        let return_data = TransactionReturnData { program_id: address(1), data: Vec::new() };
        assert!(ReturnDataSnapshot::from_meta(&return_data).is_none());
    }

    #[test]
    fn return_data_from_meta_keeps_non_empty_data_with_its_program() {
        let return_data = TransactionReturnData { program_id: address(1), data: vec![4, 5] };
        assert_eq!(
            ReturnDataSnapshot::from_meta(&return_data),
            Some(ReturnDataSnapshot::new(address(1), vec![4, 5]))
        );
    }

    #[test]
    fn inner_instruction_snapshot_new_stores_every_field() {
        let snapshot = InnerInstructionSnapshot::new(3, 1, vec![0, 1], vec![7], 4);
        assert_eq!(snapshot.stack_height, 3);
        assert_eq!(snapshot.program_id_index, 1);
        assert_eq!(snapshot.accounts, vec![0, 1]);
        assert_eq!(snapshot.data, vec![7]);
        assert_eq!(snapshot.outer_instruction_index, 4);
    }

    #[test]
    fn flatten_inner_instructions_of_an_empty_list_is_empty() {
        assert!(flatten_inner_instructions(&InnerInstructionsList::new()).is_empty());
    }

    /// Multiple inner instructions in one group all carry the same
    /// `outer_instruction_index`, and groups keep their positions.
    #[test]
    fn flatten_inner_instructions_groups_by_outer_index() {
        let list: InnerInstructionsList = vec![
            vec![inner(1, vec![10], 2), inner(2, vec![20], 3)],
            vec![],
            vec![inner(3, vec![30], 2)],
        ];
        let flattened = flatten_inner_instructions(&list);

        assert_eq!(flattened.len(), 3);
        assert_eq!(flattened[0].outer_instruction_index, 0);
        assert_eq!(flattened[0].data, vec![10]);
        assert_eq!(flattened[1].outer_instruction_index, 0);
        assert_eq!(flattened[1].data, vec![20]);
        assert_eq!(flattened[2].outer_instruction_index, 2);
        assert_eq!(flattened[2].data, vec![30]);
        assert_eq!(flattened[2].stack_height, 2);
    }

    // Property: flattening preserves the total instruction count and never
    // drops or duplicates an outer index.
    proptest! {
        #[test]
        fn flatten_inner_instructions_preserves_counts_and_indices(
            groups in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..4), 0..8),
        ) {
            let list: InnerInstructionsList = groups
                .iter()
                .map(|group| {
                    group.iter().map(|data| inner(1, vec![*data], 2)).collect::<Vec<InnerInstruction>>()
                })
                .collect();

            let flattened = flatten_inner_instructions(&list);

            prop_assert_eq!(flattened.len(), groups.iter().map(Vec::len).sum::<usize>());
            for (index, group) in groups.iter().enumerate() {
                prop_assert_eq!(
                    flattened.iter().filter(|inner| inner.outer_instruction_index == index).count(),
                    group.len()
                );
            }
        }
    }

    // Property: an empty return-data payload always yields `None`, whatever the
    // program id.
    proptest! {
        #[test]
        fn empty_return_data_never_produces_a_snapshot(program in any::<[u8; 32]>()) {
            let return_data =
                TransactionReturnData { program_id: Address::new_from_array(program), data: Vec::new() };
            prop_assert!(ReturnDataSnapshot::from_meta(&return_data).is_none());
        }
    }

    // Property: a non-empty payload is preserved verbatim with its program id.
    proptest! {
        #[test]
        fn non_empty_return_data_is_preserved_verbatim(
            program in any::<[u8; 32]>(),
            payload in prop::collection::vec(any::<u8>(), 1..16),
        ) {
            let return_data =
                TransactionReturnData { program_id: Address::new_from_array(program), data: payload.clone() };
            let snapshot = ReturnDataSnapshot::from_meta(&return_data).unwrap();
            prop_assert_eq!(snapshot.program_id, Address::new_from_array(program));
            prop_assert_eq!(snapshot.data, payload);
        }
    }
}
