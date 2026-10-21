//! Rejection: the handler context is not `hpsvm::SvmInstructionCtx`.

#![allow(unused_imports)]

use hpsvm_macros::svm_instruction;
use solana_transaction::InstructionError;

/// A look-alike context type that is not the one the macro expands against.
mod foreign {
    pub struct SvmInstructionCtx;
}

#[svm_instruction(
    program_id = "84LtecYhx229KabnaiMixiuD5Tey4bBo6N4yzZ81Vzpz",
    compute_units = 150
)]
fn look_alike(ctx: &mut foreign::SvmInstructionCtx) -> Result<(), InstructionError> {
    let _ = ctx;
    Ok(())
}

fn main() {}
