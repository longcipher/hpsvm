//! `trybuild` rejection tests for `#[svm_instruction]`.
//!
//! Each fixture under `tests/ui/` must fail to compile with an actionable
//! `compile_error!`. Run with `TRYBUILD=overwrite` to re-bless the expected
//! stderr after intentionally changing rejection messages.

#[test]
fn svm_instruction_rejections() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/*.rs");
}
