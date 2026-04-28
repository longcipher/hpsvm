use solana_address::Address;
use solana_transaction::sanitized::SanitizedTransaction;

use crate::HPSVM;

/// Observes transaction execution without mutating VM state.
pub trait Inspector: Send + Sync {
    /// Called immediately before top-level instruction processing begins.
    fn on_transaction_start(&self, _svm: &HPSVM, _tx: &SanitizedTransaction) {}

    /// Called before each top-level instruction is executed.
    fn on_instruction(&self, _svm: &HPSVM, _index: usize, _program_id: &Address) {}

    /// Called after top-level instruction processing completes.
    fn on_transaction_end(
        &self,
        _svm: &HPSVM,
        _result: &solana_transaction_error::TransactionResult<()>,
    ) {
    }
}

#[derive(Debug, Default)]
pub(crate) struct NoopInspector;

impl Inspector for NoopInspector {}
