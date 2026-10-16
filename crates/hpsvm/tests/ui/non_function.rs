//! Rejection: the attribute decorates a non-function item.

use hpsvm_macros::svm_instruction;

#[svm_instruction(program_id = "84LtecYhx229KabnaiMixiuD5Tey4bBo6N4yzZ81Vzpz", compute_units = 150)]
struct NotAFunction;

fn main() {}
