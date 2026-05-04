use solana_address::Address;
use solana_transaction::sanitized::SanitizedTransaction;

use crate::HPSVM;

/// Origin of a transaction observed by an [`Inspector`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransactionOrigin {
    /// A caller-submitted transaction or instruction helper.
    User,
    /// The internal transfer used by [`HPSVM::airdrop`].
    InternalAirdrop,
    /// A transaction executed as part of a batch stage.
    Batch {
        /// Zero-based stage index in the computed batch plan.
        stage_index: usize,
        /// Index in the caller-provided transaction list.
        transaction_index: usize,
    },
}

impl TransactionOrigin {
    const fn is_user(self) -> bool {
        matches!(self, Self::User)
    }
}

/// Observes transaction execution without mutating VM state.
pub trait Inspector: Send + Sync {
    /// Called immediately before top-level instruction processing begins.
    fn on_transaction_start(&self, _svm: &HPSVM, _tx: &SanitizedTransaction) {}

    /// Called immediately before top-level instruction processing begins, with origin context.
    fn on_transaction_start_with_origin(
        &self,
        origin: TransactionOrigin,
        svm: &HPSVM,
        tx: &SanitizedTransaction,
    ) {
        if origin.is_user() {
            self.on_transaction_start(svm, tx);
        }
    }

    /// Called before each top-level instruction is executed.
    fn on_instruction(&self, _svm: &HPSVM, _index: usize, _program_id: &Address) {}

    /// Called before each top-level instruction is executed, with origin context.
    fn on_instruction_with_origin(
        &self,
        origin: TransactionOrigin,
        svm: &HPSVM,
        index: usize,
        program_id: &Address,
    ) {
        if origin.is_user() {
            self.on_instruction(svm, index, program_id);
        }
    }

    /// Called after top-level instruction processing completes.
    fn on_transaction_end(
        &self,
        _svm: &HPSVM,
        _result: &solana_transaction_error::TransactionResult<()>,
    ) {
    }

    /// Called after top-level instruction processing completes, with origin context.
    fn on_transaction_end_with_origin(
        &self,
        origin: TransactionOrigin,
        svm: &HPSVM,
        result: &solana_transaction_error::TransactionResult<()>,
    ) {
        if origin.is_user() {
            self.on_transaction_end(svm, result);
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct NoopInspector;

impl Inspector for NoopInspector {}
