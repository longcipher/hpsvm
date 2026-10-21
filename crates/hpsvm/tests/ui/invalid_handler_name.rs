//! Rejection: the handler name does not derive a valid type name.

#![allow(unused_imports)]

use hpsvm::SvmInstructionCtx;
use hpsvm_macros::svm_instruction;
use solana_transaction::InstructionError;

#[svm_instruction(
    program_id = "84LtecYhx229KabnaiMixiuD5Tey4bBo6N4yzZ81Vzpz",
    compute_units = 150
)]
fn r#type(ctx: &mut SvmInstructionCtx) -> Result<(), InstructionError> {
    let _ = ctx;
    Ok(())
}

fn main() {}
