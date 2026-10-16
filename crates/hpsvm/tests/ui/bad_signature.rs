//! Rejection: handler takes no context argument.

#![allow(unused_imports)]

use hpsvm::SvmInstructionCtx;
use hpsvm_macros::svm_instruction;
use solana_transaction::InstructionError;

#[svm_instruction(program_id = "84LtecYhx229KabnaiMixiuD5Tey4bBo6N4yzZ81Vzpz", compute_units = 150)]
fn missing_context() -> Result<(), InstructionError> {
    Ok(())
}

fn main() {
    let _ = SvmInstructionCtx {
        program_id: solana_address::Address::default(),
        accounts: Vec::new(),
        data: Vec::new(),
    };
    let _ = missing_context;
}
