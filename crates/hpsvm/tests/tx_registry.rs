//! Task 6.1 (TxRegistry) acceptance tests.
//!
//! * A transaction whose tag matches a registered handler executes that handler.
//! * Dispatching an unregistered tag produces a typed error.
//! * Registering a handler for the Legacy tag intercepts legacy transfers routed through the normal
//!   transaction entry points.

use std::sync::{Arc, Mutex};

use hpsvm::{
    BaseSvmTypes, DetachedTx, ExecutionError, Hpsvm, Svm, SvmTypesHost, TxError, TxHandler,
    TxRegistry, TxTag, TxValidationError, handler,
};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::{Message, VersionedMessage};
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::versioned::VersionedTransaction;

/// A handler that records `validate`/`execute` invocations in a shared log
/// and then delegates to the default execution path.
struct RecordingHandler {
    log: Arc<Mutex<Vec<&'static str>>>,
}

impl<T: SvmTypesHost> TxHandler<T, VersionedTransaction, DetachedTx> for RecordingHandler {
    fn validate(
        &self,
        _tx: &VersionedTransaction,
        _host: &Svm<'_, T>,
    ) -> Result<(), TxValidationError> {
        self.log.lock().expect("log lock").push("validate");
        Ok(())
    }

    fn execute(
        &self,
        tx: VersionedTransaction,
        host: &Svm<'_, T>,
    ) -> Result<DetachedTx, ExecutionError> {
        self.log.lock().expect("log lock").push("execute");
        hpsvm::DefaultExecutionHandler.execute(tx, host)
    }
}

fn funded_transfer_tx(
    svm: &Hpsvm,
    payer: &Keypair,
    recipient: &Address,
    lamports: u64,
) -> VersionedTransaction {
    let message = Message::new_with_blockhash(
        &[transfer(&payer.pubkey(), recipient, lamports)],
        Some(&payer.pubkey()),
        &svm.latest_blockhash(),
    );
    VersionedTransaction::try_new(VersionedMessage::Legacy(message), &[payer])
        .expect("transfer transaction should build")
}

#[test]
fn custom_tag_dispatches_to_the_registered_handler() {
    let mut svm = Hpsvm::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();
    svm.airdrop(&payer.pubkey(), 1_000_000).expect("airdrop should succeed");
    let tx = funded_transfer_tx(&svm, &payer, &recipient, 64);

    let log = Arc::new(Mutex::new(Vec::new()));
    let mut registry = TxRegistry::<BaseSvmTypes, DetachedTx>::new();
    registry.register(TxTag::Custom(7), RecordingHandler { log: Arc::clone(&log) });

    let detached =
        registry.dispatch(TxTag::Custom(7), tx, &svm).expect("registered tag should dispatch");
    assert!(detached.status().is_ok(), "delegated execution should succeed");

    assert_eq!(*log.lock().expect("log lock"), vec!["validate", "execute"]);
}

#[test]
fn unregistered_tag_produces_a_typed_error() {
    let mut svm = Hpsvm::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();
    svm.airdrop(&payer.pubkey(), 1_000_000).expect("airdrop should succeed");
    let tx = funded_transfer_tx(&svm, &payer, &recipient, 64);

    let registry = TxRegistry::<BaseSvmTypes, DetachedTx>::new();
    let error =
        registry.dispatch(TxTag::Custom(9), tx, &svm).expect_err("unregistered tag should fail");

    assert!(
        matches!(error, TxError::UnknownTag(TxTag::Custom(9))),
        "expected UnknownTag(Custom(9)), got {error:?}"
    );
}

#[test]
fn handler_adapter_registers_two_closures_as_one_handler() {
    let mut svm = Hpsvm::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();
    svm.airdrop(&payer.pubkey(), 1_000_000).expect("airdrop should succeed");
    let tx = funded_transfer_tx(&svm, &payer, &recipient, 64);

    let log = Arc::new(Mutex::new(Vec::new()));
    let validate_log = Arc::clone(&log);
    let execute_log = Arc::clone(&log);
    let adapted = handler(
        move |_tx: &VersionedTransaction, _host: &Svm<'_, BaseSvmTypes>| {
            validate_log.lock().expect("log lock").push("validate");
            Ok(())
        },
        move |tx: VersionedTransaction, host: &Svm<'_, BaseSvmTypes>| {
            execute_log.lock().expect("log lock").push("execute");
            hpsvm::DefaultExecutionHandler.execute(tx, host)
        },
    );

    let mut registry = TxRegistry::<BaseSvmTypes, DetachedTx>::new();
    registry.register(TxTag::Custom(7), adapted);
    registry.dispatch(TxTag::Custom(7), tx, &svm).expect("registered tag should dispatch");

    assert_eq!(*log.lock().expect("log lock"), vec!["validate", "execute"]);
}

#[test]
fn legacy_handler_registration_intercepts_legacy_transfers() {
    let mut svm = Hpsvm::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();
    svm.airdrop(&payer.pubkey(), 1_000_000).expect("airdrop should succeed");

    let log = Arc::new(Mutex::new(Vec::new()));
    svm.register_tx_handler(TxTag::Legacy, RecordingHandler { log: Arc::clone(&log) });

    let tx = funded_transfer_tx(&svm, &payer, &recipient, 64);
    let fee = 5000u64;
    svm.send_transaction(tx).expect("intercepted transfer should succeed");

    assert_eq!(*log.lock().expect("log lock"), vec!["validate", "execute"]);
    assert_eq!(svm.get_balance(&recipient), Some(64));
    assert_eq!(svm.get_balance(&payer.pubkey()), Some(1_000_000 - 64 - fee));
}
