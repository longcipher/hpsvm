// adapted from agave 4.x
use solana_program_runtime::invoke_context::InvokeContext;
use solana_svm_timings::ExecuteTimings;
// The agave-side copy of the message traits: `AgaveMessage` implements these,
// and `InvokeContext` (agave 4.5) is typed against them.
use solana_svm_transaction_agave::svm_message::SVMStaticMessage;
use solana_transaction_context::IndexOfAccount;
use solana_transaction_error::TransactionError;

use crate::{InspectMode, Svm, SvmTypesHost, agave_compat::AgaveMessage};

#[cfg_attr(feature = "hotpath", hotpath::measure)]
pub(crate) fn process_message<'ix_data, const MODE: InspectMode, T: SvmTypesHost>(
    svm: &Svm<'_, T>,
    // The agave-4.5 view of the sanitized message: `InvokeContext` is typed
    // against the older interface crates, so its `prepare_top_level_instructions`
    // needs this bridge (see `agave_compat`). The borrow must outlive the
    // transaction context the invoke context writes instruction data into.
    message: &'ix_data AgaveMessage<'ix_data>,
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
        svm.on_instruction_start::<MODE>(top_level_instruction_index, program_id);

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

        // The end hook fires for failed instructions too, before the error
        // propagates to the transaction result.
        svm.on_instruction_end::<MODE>(top_level_instruction_index, program_id);

        result.map_err(|err| {
            TransactionError::InstructionError(top_level_instruction_index as u8, err)
        })?;
    }
    Ok(())
}
