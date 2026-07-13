use solana_address::Address;
use solana_instruction::account_meta::AccountMeta;
use solana_message::{
    compiled_instruction::CompiledInstruction,
    inner_instruction::{InnerInstruction, InnerInstructionsList},
};
use solana_transaction_context::transaction::TransactionContext;

use crate::types::{ExecutedInstruction, ExecutionTrace};

pub(crate) fn extract_instruction_trace_data(
    transaction_context: &mut TransactionContext<'_>,
) -> (InnerInstructionsList, ExecutionTrace) {
    // Collect account keys before consuming the trace
    let num_accounts = transaction_context.get_number_of_accounts();
    let account_keys: Vec<Address> = (0..num_accounts)
        .map(|i| {
            transaction_context.get_key_of_account_at_index(i).ok().copied().unwrap_or_default()
        })
        .collect();

    let (frames, accounts, data) = transaction_context.take_instruction_trace();

    let num_top_level =
        frames.iter().take_while(|frame| frame.index_of_caller_instruction == u16::MAX).count();

    let mut all_inner_instructions: Vec<Vec<InnerInstruction>> =
        (0..num_top_level).map(|_| Vec::new()).collect();

    let mut execution_instructions: Vec<ExecutedInstruction> = Vec::new();

    for (i, frame) in frames.iter().enumerate() {
        let stack_height = frame.nesting_level.saturating_add(1);

        let program_account_index = frame.program_account_index_in_tx as usize;
        let program_id = account_keys.get(program_account_index).copied().unwrap_or_default();

        let ix_accounts: Vec<AccountMeta> = accounts[i]
            .iter()
            .filter_map(|acc| {
                let pubkey = account_keys.get(acc.index_in_transaction as usize).copied()?;
                Some(AccountMeta {
                    pubkey,
                    is_signer: acc.is_signer(),
                    is_writable: acc.is_writable(),
                })
            })
            .collect();

        execution_instructions.push(ExecutedInstruction {
            stack_height: stack_height as u8,
            program_id,
            accounts: ix_accounts,
            data: data[i].to_vec(),
        });

        if i >= num_top_level {
            let mut ancestor = i;
            while ancestor >= num_top_level {
                ancestor = frames[ancestor].index_of_caller_instruction as usize;
            }

            let inner_instruction = InnerInstruction {
                instruction: CompiledInstruction::new_from_raw_parts(
                    frame.program_account_index_in_tx as u8,
                    data[i].to_vec(),
                    accounts[i].iter().map(|acc| acc.index_in_transaction as u8).collect(),
                ),
                stack_height: stack_height as u8,
            };
            all_inner_instructions[ancestor].push(inner_instruction);
        }
    }

    let execution_trace = ExecutionTrace { instructions: execution_instructions };
    (all_inner_instructions, execution_trace)
}
