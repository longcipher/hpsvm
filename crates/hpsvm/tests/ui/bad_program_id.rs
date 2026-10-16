//! Rejection: `program_id` is not valid base58.

#![allow(unused_imports)]

use hpsvm::SvmInstructionCtx;
use hpsvm_macros::svm_instruction;
use solana_transaction::InstructionError;

#[svm_instruction(program_id = "not-a-valid-program-id!!", compute_units = 150)]
fn bad_id(ctx: &mut SvmInstructionCtx) -> Result<(), InstructionError> {
    let _ = ctx;
    Ok(())
}

fn main() {}
