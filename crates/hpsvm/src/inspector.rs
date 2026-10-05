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

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use solana_keypair::Keypair;
    use solana_message::{Message, VersionedMessage};
    use solana_signer::Signer;
    use solana_system_interface::instruction::transfer;
    use solana_transaction::versioned::VersionedTransaction;
    use solana_transaction_error::{TransactionError, TransactionResult};

    use super::*;

    /// Builds a minimal sanitized transfer transaction; the inspector hooks
    /// only ever receive it as an opaque payload.
    fn sanitized_transfer(svm: &HPSVM) -> SanitizedTransaction {
        let payer = Keypair::new();
        let recipient = Address::new_unique();
        let message =
            Message::new(&[transfer(&payer.pubkey(), &recipient, 1)], Some(&payer.pubkey()));
        let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(message), &[&payer])
            .expect("unsigned transfer should sanitize");
        svm.sanitize_transaction_no_verify(tx).expect("sanitize should succeed")
    }

    /// Records every callback the VM fires so origin-based suppression can be
    /// asserted without needing a real [`HPSVM`].
    #[derive(Default)]
    struct RecordingInspector {
        events: Mutex<Vec<String>>,
    }

    impl RecordingInspector {
        fn record(&self, event: String) {
            self.events.lock().expect("recording inspector lock").push(event);
        }

        fn events(&self) -> Vec<String> {
            self.events.lock().expect("recording inspector lock").clone()
        }
    }

    /// Only overrides the origin-less hooks, so every observed event proves the
    /// default `*_with_origin` wrapper decided the origin was `User`.
    impl Inspector for RecordingInspector {
        fn on_transaction_start(&self, _svm: &HPSVM, _tx: &SanitizedTransaction) {
            self.record("start".to_string());
        }

        fn on_instruction(&self, _svm: &HPSVM, index: usize, program_id: &Address) {
            self.record(format!("instruction {index} {program_id}"));
        }

        fn on_transaction_end(&self, _svm: &HPSVM, result: &TransactionResult<()>) {
            self.record(format!("end {}", result.is_ok()));
        }
    }

    /// Overrides only the origin-aware hooks, so it observes every origin.
    struct OriginInspector {
        events: Mutex<Vec<TransactionOrigin>>,
    }

    impl Inspector for OriginInspector {
        fn on_transaction_start_with_origin(
            &self,
            origin: TransactionOrigin,
            _svm: &HPSVM,
            _tx: &SanitizedTransaction,
        ) {
            self.events.lock().expect("origin inspector lock").push(origin);
        }
    }

    fn origins() -> [TransactionOrigin; 4] {
        [
            TransactionOrigin::User,
            TransactionOrigin::InternalAirdrop,
            TransactionOrigin::Batch { stage_index: 0, transaction_index: 0 },
            TransactionOrigin::Batch { stage_index: 3, transaction_index: 7 },
        ]
    }

    #[test]
    fn only_the_user_origin_is_a_user_origin() {
        assert!(TransactionOrigin::User.is_user());
        assert!(!TransactionOrigin::InternalAirdrop.is_user());
        assert!(!TransactionOrigin::Batch { stage_index: 0, transaction_index: 0 }.is_user());
        assert!(!TransactionOrigin::Batch { stage_index: 9, transaction_index: 9 }.is_user());
    }

    /// The default wrappers must forward user traffic to the origin-less hooks…
    #[test]
    fn default_wrappers_forward_user_traffic_to_the_origin_less_hooks() {
        for origin in origins().into_iter().filter(|origin| origin.is_user()) {
            let inspector = RecordingInspector::default();
            let svm = HPSVM::new();
            let tx = sanitized_transfer(&svm);
            let program_id = Address::new_unique();
            let result: TransactionResult<()> = Ok(());

            inspector.on_transaction_start_with_origin(origin, &svm, &tx);
            inspector.on_instruction_with_origin(origin, &svm, 2, &program_id);
            inspector.on_transaction_end_with_origin(origin, &svm, &result);

            let expected = vec![
                "start".to_string(),
                format!("instruction 2 {program_id}"),
                "end true".to_string(),
            ];
            assert_eq!(inspector.events(), expected, "origin {origin:?}");
        }
    }

    /// …and must drop every non-user origin, including airdrops and batch stages.
    #[test]
    fn default_wrappers_suppress_non_user_origins() {
        for origin in origins().into_iter().filter(|origin| !origin.is_user()) {
            let inspector = RecordingInspector::default();
            let svm = HPSVM::new();
            let tx = sanitized_transfer(&svm);
            let result: TransactionResult<()> = Err(TransactionError::AlreadyProcessed);

            inspector.on_transaction_start_with_origin(origin, &svm, &tx);
            inspector.on_instruction_with_origin(origin, &svm, 0, &Address::new_unique());
            inspector.on_transaction_end_with_origin(origin, &svm, &result);

            assert!(
                inspector.events().is_empty(),
                "origin {origin:?} must not reach the origin-less hooks, saw {:?}",
                inspector.events()
            );
        }
    }

    /// An implementor that overrides the origin-aware hook sees every origin,
    /// so the wrapper must not filter before dispatching to it.
    #[test]
    fn origin_aware_override_observes_every_origin() {
        let inspector = OriginInspector { events: Mutex::new(Vec::new()) };
        let svm = HPSVM::new();
        let tx = sanitized_transfer(&svm);

        for origin in origins() {
            inspector.on_transaction_start_with_origin(origin, &svm, &tx);
        }

        assert_eq!(inspector.events.lock().expect("origin inspector lock").clone(), origins());
    }

    #[test]
    fn noop_inspector_accepts_every_callback() {
        let inspector = NoopInspector;
        let svm = HPSVM::new();
        let tx = sanitized_transfer(&svm);
        let result: TransactionResult<()> = Ok(());

        for origin in origins() {
            inspector.on_transaction_start(&svm, &tx);
            inspector.on_transaction_start_with_origin(origin, &svm, &tx);
            inspector.on_instruction(&svm, 0, &Address::new_unique());
            inspector.on_instruction_with_origin(origin, &svm, 0, &Address::new_unique());
            inspector.on_transaction_end(&svm, &result);
            inspector.on_transaction_end_with_origin(origin, &svm, &result);
        }
    }
}
