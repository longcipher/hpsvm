// adapted from agave 4.x
use solana_program_runtime::invoke_context::InvokeContext;
use solana_svm_timings::ExecuteTimings;
use solana_svm_transaction::svm_message::SVMMessage;
use solana_transaction_context::IndexOfAccount;
use solana_transaction_error::TransactionError;

use crate::HPSVM;

#[cfg_attr(feature = "hotpath", hotpath::measure)]
pub(crate) fn process_message<'ix_data>(
    svm: &HPSVM,
    message: &'ix_data impl SVMMessage,
    program_indices: &[IndexOfAccount],
    invoke_context: &mut InvokeContext<'_, 'ix_data>,
    execute_timings: &mut ExecuteTimings,
    accumulated_consumed_units: &mut u64,
) -> Result<(), TransactionError> {
    debug_assert_eq!(program_indices.len(), message.num_instructions());

    invoke_context
        .prepare_top_level_instructions(message)
        .map_err(|(index, err)| TransactionError::InstructionError(index, err))?;

    for (top_level_instruction_index, ((program_id, _instruction), _program_account_index)) in
        message.program_instructions_iter().zip(program_indices.iter()).enumerate()
    {
        svm.on_instruction(top_level_instruction_index, program_id);

        let mut compute_units_consumed = 0;
        let result = if invoke_context.is_precompile(program_id) {
            crate::hotpath_block!("hpsvm::process_message::execute_precompile", {
                invoke_context.process_precompile(
                    program_id,
                    _instruction.data,
                    message.instructions_iter().map(|ix| ix.data),
                )
            })
        } else {
            crate::hotpath_block!("hpsvm::process_message::execute_instruction", {
                invoke_context.process_instruction(&mut compute_units_consumed, execute_timings)
            })
        };

        crate::hotpath_block!("hpsvm::process_message::account_consumed_units", {
            *accumulated_consumed_units =
                accumulated_consumed_units.saturating_add(compute_units_consumed);
        });

        result.map_err(|err| {
            TransactionError::InstructionError(top_level_instruction_index as u8, err)
        })?;
    }
    Ok(())
}
