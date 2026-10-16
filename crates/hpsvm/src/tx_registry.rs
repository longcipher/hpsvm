//! Typed transaction handlers behind an erased registry.
//!
//! Every transaction envelope maps to a [`TxTag`] via [`tag_of`]; the
//! [`Svm`](crate::Svm) routes each envelope through the [`TxRegistry`] entry
//! for its tag instead of a hardcoded execution path. Custom tags
//! ([`TxTag::Custom`]) let tests and forks intercept or replace execution for
//! synthetic envelopes, while the [`DefaultExecutionHandler`] registered under
//! [`TxTag::Legacy`], [`TxTag::V0`] and [`TxTag::V1`] preserves the existing
//! behaviour for real transaction kinds.
//!
//! # Deferred pieces
//!
//! * The design (§3.8) names a dedicated `TxError`. The taxonomy now exists as
//!   [`TxError`](crate::error::TxError), and [`TxRegistryError::into_tx_error`] maps this boundary
//!   onto it — but handlers still bind to [`HPSVMError`](crate::error::HPSVMError) and dispatch
//!   failures still surface as [`TxRegistryError`], which converts into `HPSVMError` losslessly.
//!   Switching the trait and `dispatch` signatures belongs to a future registry task.
//! * The design (§3.8) takes `host: &mut T::Host<'_>`. The host is pinned to the VM itself by the
//!   blanket marker (a family can only satisfy it by pointing its host at the VM), so handlers take
//!   `&Svm<'_, T>` directly and avoid higher-ranked lifetime friction. The reference is shared
//!   rather than exclusive because two of the three entry points (`transact_detached`,
//!   `try_transact`) run on `&self`: readonly execution only ever needed `&self`
//!   (`execute_readonly`), so no handler can observe a difference.
//! * Only `VersionedTransaction` envelopes are implemented today. The erased [`TxHandler`] trait
//!   stays generic over `Tx` so non-`VersionedTransaction` envelopes can be added later without
//!   changing the registry shape.

use std::{collections::HashMap, sync::Arc};

use solana_message::VersionedMessage;
use solana_transaction::versioned::VersionedTransaction;
use solana_transaction_error::TransactionError;
use thiserror::Error;

use crate::{
    DetachedTx, Svm, SvmTypesHost,
    error::{AnyError, ExecutionError, HPSVMError, TxError, TxValidationError},
};

/// Classifies a transaction envelope for handler dispatch.
///
/// [`tag_of`] derives this from real envelopes, so [`TxTag::Legacy`],
/// [`TxTag::V0`] and [`TxTag::V1`] arise from the transaction entry points.
/// [`TxTag::Custom`] tags exist for synthetic envelopes in tests and forks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TxTag {
    /// A legacy `VersionedMessage::Legacy` envelope.
    Legacy,
    /// A versioned `VersionedMessage::V0` envelope.
    V0,
    /// A versioned `VersionedMessage::V1` envelope.
    V1,
    /// A synthetic tag with no on-chain envelope counterpart.
    Custom(u8),
}

/// Derives the dispatch tag for a transaction envelope.
///
/// Total over every envelope the entry points accept: legacy messages map to
/// [`TxTag::Legacy`], v0 messages to [`TxTag::V0`], v1 messages to
/// [`TxTag::V1`].
pub fn tag_of(tx: &VersionedTransaction) -> TxTag {
    match tx.message {
        VersionedMessage::Legacy(_) => TxTag::Legacy,
        VersionedMessage::V0(_) => TxTag::V0,
        VersionedMessage::V1(_) => TxTag::V1,
    }
}

/// A typed transaction handler: validation plus execution against the host.
///
/// Handlers run against a shared host reference (see the module docs for why
/// the design's `&mut T::Host` is a shared `&Svm` here) and must be
/// thread-shareable because batch workers clone the registry onto Rayon
/// threads.
pub trait TxHandler<T: SvmTypesHost, Tx, Output>: Send + Sync {
    /// Rejects transactions the handler refuses to run.
    ///
    /// The registry calls this before [`execute`](TxHandler::execute); a
    /// failure short-circuits execution and surfaces as
    /// [`TxRegistryError::HandlerFailed`].
    fn validate(&self, tx: &Tx, host: &Svm<'_, T>) -> Result<(), HPSVMError>;

    /// Runs the transaction against the host without staging any state.
    ///
    /// Staging stays with the caller (`transact_core`): handlers return the
    /// owned readonly outcome only.
    fn execute(&self, tx: Tx, host: &Svm<'_, T>) -> Result<Output, HPSVMError>;
}

/// Object-safe erased handler over `VersionedTransaction` envelopes.
///
/// Kept private: callers name [`TxHandler`], never this trait. The blanket
/// implementation below covers every typed handler, and the generic `Tx`
/// parameter on [`TxHandler`] stays open for future non-`VersionedTransaction`
/// envelopes.
trait AnyTxHandler<T: SvmTypesHost, Output>: Send + Sync {
    fn validate_erased(
        &self,
        tx: &VersionedTransaction,
        host: &Svm<'_, T>,
    ) -> Result<(), HPSVMError>;

    fn execute_erased(
        &self,
        tx: VersionedTransaction,
        host: &Svm<'_, T>,
    ) -> Result<Output, HPSVMError>;
}

impl<T, Output, H> AnyTxHandler<T, Output> for H
where
    T: SvmTypesHost,
    H: TxHandler<T, VersionedTransaction, Output>,
{
    fn validate_erased(
        &self,
        tx: &VersionedTransaction,
        host: &Svm<'_, T>,
    ) -> Result<(), HPSVMError> {
        self.validate(tx, host)
    }

    fn execute_erased(
        &self,
        tx: VersionedTransaction,
        host: &Svm<'_, T>,
    ) -> Result<Output, HPSVMError> {
        self.execute(tx, host)
    }
}

/// Dispatch failure: either no handler owns the tag, or the owning handler
/// declined the transaction.
#[derive(Debug, Error)]
pub enum TxRegistryError {
    /// No handler was registered for the transaction's tag.
    #[error("no transaction handler registered for tag {0:?}")]
    UnknownTag(TxTag),
    /// The owning handler's `validate` or `execute` failed.
    #[error(transparent)]
    HandlerFailed(#[from] HPSVMError),
}

impl TxRegistryError {
    /// Maps a dispatch failure onto the layered taxonomy without changing
    /// the `dispatch` signature.
    ///
    /// An unroutable envelope is a validation rejection
    /// ([`UnsupportedVersion`](TransactionError::UnsupportedVersion),
    /// mirroring `TxRegistryError::transaction_error`);
    /// a handler failure is an erased fatal execution error whose concrete
    /// [`HPSVMError`] stays recoverable by downcast.
    pub fn into_tx_error(self) -> TxError {
        match self {
            Self::UnknownTag(_) => TxError::Validation(TxValidationError::Invalid(
                TransactionError::UnsupportedVersion,
            )),
            Self::HandlerFailed(error) => {
                TxError::Execution(ExecutionError::Fatal(Box::new(AnyError::new(error))))
            }
        }
    }

    /// Maps a dispatch failure to the execution status a detached outcome
    /// carries when the failure cannot propagate as an error.
    ///
    /// An unroutable envelope looks, from the chain's perspective, like an
    /// unsupported version; a handler that declines a transaction rejects it
    /// before execution, like a failed sanity check.
    pub(crate) fn transaction_error(&self) -> TransactionError {
        match self {
            Self::UnknownTag(_) => TransactionError::UnsupportedVersion,
            Self::HandlerFailed(_) => TransactionError::SanitizeFailure,
        }
    }
}

/// Tag-to-handler map consulted by every transaction entry point.
///
/// Handlers are held behind `Arc`, so cloning a registry (as batch workers
/// and scratch VMs do) bumps reference counts instead of copying handlers.
/// Registering a tag twice replaces the previous handler.
pub struct TxRegistry<T, Output> {
    handlers: HashMap<TxTag, Arc<dyn AnyTxHandler<T, Output>>>,
}

impl<T, Output> TxRegistry<T, Output> {
    /// Creates an empty registry with no handlers.
    pub fn new() -> Self {
        Self { handlers: HashMap::new() }
    }

    /// Registers `handle` for `tag`, replacing any previous entry.
    pub fn register<H>(&mut self, tag: TxTag, handle: H)
    where
        T: SvmTypesHost,
        H: TxHandler<T, VersionedTransaction, Output> + 'static,
    {
        self.handlers.insert(tag, Arc::new(handle));
    }

    /// Runs the handler for `tag`: `validate` first, then `execute`.
    ///
    /// An unregistered tag yields [`TxRegistryError::UnknownTag`]; a
    /// validation or execution failure yields
    /// [`TxRegistryError::HandlerFailed`].
    pub fn dispatch(
        &self,
        tag: TxTag,
        tx: VersionedTransaction,
        host: &Svm<'_, T>,
    ) -> Result<Output, TxRegistryError>
    where
        T: SvmTypesHost,
    {
        let handle = self.handlers.get(&tag).ok_or(TxRegistryError::UnknownTag(tag))?;
        handle.validate_erased(&tx, host).map_err(TxRegistryError::HandlerFailed)?;
        handle.execute_erased(tx, host).map_err(TxRegistryError::HandlerFailed)
    }

    /// Lists the tags that currently own a handler.
    pub fn tags(&self) -> Vec<TxTag> {
        let mut tags: Vec<TxTag> = self.handlers.keys().copied().collect();
        tags.sort_by_key(|tag| match tag {
            TxTag::Legacy => (0, 0),
            TxTag::V0 => (1, 0),
            TxTag::V1 => (2, 0),
            TxTag::Custom(id) => (3, *id),
        });
        tags
    }
}

impl<T, Output> Default for TxRegistry<T, Output> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, Output> Clone for TxRegistry<T, Output> {
    fn clone(&self) -> Self {
        Self {
            handlers: self
                .handlers
                .iter()
                .map(|(tag, handle)| (*tag, Arc::clone(handle)))
                .collect(),
        }
    }
}

impl<T, Output> std::fmt::Debug for TxRegistry<T, Output> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TxRegistry").field("tags", &self.tags()).finish()
    }
}

impl<T: SvmTypesHost> TxRegistry<T, DetachedTx> {
    /// Creates a registry with the default execution handler under the
    /// [`TxTag::Legacy`], [`TxTag::V0`] and [`TxTag::V1`] tags.
    ///
    /// Every [`Svm`](crate::Svm) starts from this so existing transaction
    /// kinds behave exactly as before the registry existed.
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.register(TxTag::Legacy, DefaultExecutionHandler);
        registry.register(TxTag::V0, DefaultExecutionHandler);
        registry.register(TxTag::V1, DefaultExecutionHandler);
        registry
    }
}

/// The default execution handler: the pre-registry readonly execution path.
///
/// `validate` accepts everything; `execute` runs `execute_readonly` plus the
/// detached-outcome conversion and never fails on its own — fatal execution
/// errors travel inside the returned [`DetachedTx`] (see its `fatal_error`
/// transport) so every entry point keeps its exact legacy convention for
/// them. Registering a handler for a tag this owns replaces it, which is how
/// interception works.
#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultExecutionHandler;

impl<T: SvmTypesHost> TxHandler<T, VersionedTransaction, DetachedTx> for DefaultExecutionHandler {
    fn validate(&self, _tx: &VersionedTransaction, _host: &Svm<'_, T>) -> Result<(), HPSVMError> {
        Ok(())
    }

    fn execute(
        &self,
        tx: VersionedTransaction,
        host: &Svm<'_, T>,
    ) -> Result<DetachedTx, HPSVMError> {
        Ok(host.default_execute_detached(tx))
    }
}

/// A pair of closures acting as one [`TxHandler`].
///
/// The parameters are named `validate`/`execute` to match the trait
/// vocabulary.
pub struct HandlerFn<F, G> {
    validate: F,
    execute: G,
}

/// Adapts a `validate` closure and an `execute` closure into one handler.
pub fn handler<F, G>(validate: F, execute: G) -> HandlerFn<F, G> {
    HandlerFn { validate, execute }
}

impl<T, Tx, Output, F, G> TxHandler<T, Tx, Output> for HandlerFn<F, G>
where
    T: SvmTypesHost,
    F: Fn(&Tx, &Svm<'_, T>) -> Result<(), HPSVMError> + Send + Sync,
    G: Fn(Tx, &Svm<'_, T>) -> Result<Output, HPSVMError> + Send + Sync,
{
    fn validate(&self, tx: &Tx, host: &Svm<'_, T>) -> Result<(), HPSVMError> {
        (self.validate)(tx, host)
    }

    fn execute(&self, tx: Tx, host: &Svm<'_, T>) -> Result<Output, HPSVMError> {
        (self.execute)(tx, host)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use solana_address::Address;
    use solana_keypair::Keypair;
    use solana_message::{
        Message, MessageHeader, VersionedMessage,
        compiled_instruction::CompiledInstruction,
        v0::Message as MessageV0,
        v1::{Message as MessageV1, TransactionConfig as TransactionConfigV1},
    };
    use solana_signer::Signer;
    use solana_system_interface::instruction::transfer;

    use super::*;
    use crate::{BaseSvmTypes, Hpsvm, types::TransactionMetadata};

    /// A handler that logs `validate`/`execute` calls and then either fails
    /// validation on demand or returns a canned outcome.
    struct CannedHandler {
        log: Arc<Mutex<Vec<&'static str>>>,
        fail_validate: bool,
    }

    fn canned_detached() -> DetachedTx {
        DetachedTx {
            meta: TransactionMetadata::default(),
            post_accounts: Vec::new(),
            status: Ok(()),
            included: false,
            fee_payer: None,
            fatal_error: None,
            inspector_live: false,
        }
    }

    impl<T: SvmTypesHost> TxHandler<T, VersionedTransaction, DetachedTx> for CannedHandler {
        fn validate(
            &self,
            _tx: &VersionedTransaction,
            _host: &Svm<'_, T>,
        ) -> Result<(), HPSVMError> {
            self.log.lock().expect("log lock").push("validate");
            if self.fail_validate {
                return Err(HPSVMError::ProgramLoad("validation declined".to_string()));
            }
            Ok(())
        }

        fn execute(
            &self,
            _tx: VersionedTransaction,
            _host: &Svm<'_, T>,
        ) -> Result<DetachedTx, HPSVMError> {
            self.log.lock().expect("log lock").push("execute");
            Ok(canned_detached())
        }
    }

    fn legacy_tx() -> VersionedTransaction {
        let payer = Keypair::new();
        let message = Message::new_with_blockhash(
            &[transfer(&payer.pubkey(), &Address::new_unique(), 1)],
            Some(&payer.pubkey()),
            &solana_hash::Hash::default(),
        );
        VersionedTransaction::try_new(VersionedMessage::Legacy(message), &[&payer])
            .expect("legacy transaction should build")
    }

    fn v0_tx() -> VersionedTransaction {
        let payer = Keypair::new();
        let message = MessageV0::try_compile(
            &payer.pubkey(),
            &[transfer(&payer.pubkey(), &Address::new_unique(), 1)],
            &[],
            solana_hash::Hash::default(),
        )
        .expect("v0 transaction should build");
        VersionedTransaction::try_new(VersionedMessage::V0(message), &[&payer])
            .expect("v0 transaction should build")
    }

    fn v1_tx() -> VersionedTransaction {
        let message = MessageV1::new(
            MessageHeader {
                num_required_signatures: 1,
                num_readonly_signed_accounts: 0,
                num_readonly_unsigned_accounts: 1,
            },
            TransactionConfigV1::default(),
            solana_hash::Hash::new_from_array([7; 32]),
            vec![Address::new_unique(), Address::new_unique(), Address::new_unique()],
            vec![CompiledInstruction {
                program_id_index: 1,
                accounts: vec![0, 2],
                data: vec![1, 2, 3, 4],
            }],
        );
        VersionedTransaction {
            signatures: vec![solana_signature::Signature::default()],
            message: VersionedMessage::V1(message),
        }
    }

    #[test]
    fn tag_of_covers_every_envelope_version() {
        assert_eq!(tag_of(&legacy_tx()), TxTag::Legacy);
        assert_eq!(tag_of(&v0_tx()), TxTag::V0);
        assert_eq!(tag_of(&v1_tx()), TxTag::V1);
    }

    #[test]
    fn dispatch_runs_validate_before_execute() {
        let svm = Hpsvm::new();
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut registry = TxRegistry::<BaseSvmTypes, DetachedTx>::new();
        registry.register(
            TxTag::Custom(7),
            CannedHandler { log: Arc::clone(&log), fail_validate: false },
        );

        let detached = registry
            .dispatch(TxTag::Custom(7), legacy_tx(), &svm)
            .expect("dispatch should succeed");

        assert!(detached.status().is_ok());
        assert_eq!(*log.lock().expect("log lock"), vec!["validate", "execute"]);
    }

    #[test]
    fn dispatch_to_an_unregistered_tag_yields_the_typed_error() {
        let svm = Hpsvm::new();
        let registry = TxRegistry::<BaseSvmTypes, DetachedTx>::new();

        let error = registry
            .dispatch(TxTag::Custom(9), legacy_tx(), &svm)
            .expect_err("dispatch should fail");

        assert!(
            matches!(error, TxRegistryError::UnknownTag(TxTag::Custom(9))),
            "expected UnknownTag(Custom(9)), got {error:?}"
        );
    }

    #[test]
    fn validate_failure_short_circuits_execute() {
        let svm = Hpsvm::new();
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut registry = TxRegistry::<BaseSvmTypes, DetachedTx>::new();
        registry.register(
            TxTag::Custom(7),
            CannedHandler { log: Arc::clone(&log), fail_validate: true },
        );

        let error = registry
            .dispatch(TxTag::Custom(7), legacy_tx(), &svm)
            .expect_err("dispatch should fail");

        assert!(
            matches!(error, TxRegistryError::HandlerFailed(_)),
            "expected HandlerFailed, got {error:?}"
        );
        assert_eq!(*log.lock().expect("log lock"), vec!["validate"]);
    }

    #[test]
    fn a_registry_with_defaults_owns_every_envelope_tag() {
        let registry = TxRegistry::<BaseSvmTypes, DetachedTx>::with_defaults();
        assert_eq!(registry.tags(), vec![TxTag::Legacy, TxTag::V0, TxTag::V1]);
    }

    #[test]
    fn cloning_a_registry_shares_handlers() {
        let mut registry = TxRegistry::<BaseSvmTypes, DetachedTx>::with_defaults();
        registry.register(
            TxTag::Custom(7),
            CannedHandler { log: Arc::new(Mutex::new(Vec::new())), fail_validate: false },
        );
        let cloned = registry.clone();
        assert_eq!(cloned.tags(), vec![TxTag::Legacy, TxTag::V0, TxTag::V1, TxTag::Custom(7)]);
    }

    #[test]
    fn unknown_tag_renders_its_tag() {
        let error = TxRegistryError::UnknownTag(TxTag::Custom(9));
        assert_eq!(error.to_string(), "no transaction handler registered for tag Custom(9)");
    }

    #[test]
    fn task71_into_tx_error_maps_unknown_tag_to_validation() {
        let error = TxRegistryError::UnknownTag(TxTag::Custom(9)).into_tx_error();
        match &error {
            TxError::Validation(TxValidationError::Invalid(
                solana_transaction_error::TransactionError::UnsupportedVersion,
            )) => {}
            other => panic!("expected TxError::Validation(UnsupportedVersion), got {other:?}"),
        }
        assert!(!error.is_fatal());
    }

    #[test]
    fn task71_into_tx_error_erases_handler_failures_recoverably() {
        let error = TxRegistryError::HandlerFailed(HPSVMError::ProgramLoad("boom".to_string()))
            .into_tx_error();
        match &error {
            TxError::Execution(ExecutionError::Fatal(erased)) => {
                let recovered = erased
                    .downcast_ref::<HPSVMError>()
                    .expect("handler failure must stay recoverable by downcast");
                match recovered {
                    HPSVMError::ProgramLoad(message) => assert_eq!(message, "boom"),
                    other => panic!("expected HPSVMError::ProgramLoad, got {other:?}"),
                }
            }
            other => panic!("expected TxError::Execution(Fatal), got {other:?}"),
        }
        assert!(error.is_fatal());
    }
}
