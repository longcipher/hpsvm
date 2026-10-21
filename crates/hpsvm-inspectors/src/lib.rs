//! Reusable execution inspectors for `hpsvm`.
//!
//! The [`Inspector`] trait, [`TransactionOrigin`], [`OriginFilter`] and
//! [`OriginFiltered`] stay in [`hpsvm`] because they are part of the VM's
//! contract, and this crate depends on `hpsvm`, so the reverse edge would be a
//! dependency cycle. All four are re-exported here so an implementor needs a
//! single import to write and install an inspector.
//!
//! Every hook takes `&mut self`, so an inspector keeps plain owned state and
//! hands out a shared handle (`subscribe` / `counter`) that stays readable
//! after the inspector itself has moved into the VM. The VM stores the
//! inspector boxed and takes it out per dispatch, which is why installing the
//! same inspector on two VMs is impossible — subscribe a second recorder
//! instead.
//!
//! # Example
//!
//! ```
//! use hpsvm::HPSVM;
//! use hpsvm_inspectors::CountingInspector;
//!
//! let counter = CountingInspector::default();
//! // The handle stays live after the inspector moves into the VM.
//! let observed = counter.counter();
//! let _svm = HPSVM::new().with_inspector(counter);
//!
//! // Nothing has run yet, so nothing has been observed.
//! assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 0);
//! ```

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

pub use hpsvm::{InspectMode, Inspector, OriginFilter, OriginFiltered, TransactionOrigin};
use hpsvm::{Svm, SvmTypesHost};
use parking_lot::Mutex;
use solana_address::Address;
use solana_transaction::sanitized::SanitizedTransaction;
use solana_transaction_error::TransactionResult;

/// An inspector that accepts every callback and records nothing.
///
/// This is the reusable "no inspection" inspector: install it to state the
/// choice explicitly, or as a base to decorate. It produces no observable
/// events.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoopInspector;

impl<T: SvmTypesHost> Inspector<T> for NoopInspector {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// A shared, cloneable log that a [`RecordingInspector`] appends its events to.
///
/// [`hpsvm::HPSVM::with_inspector`] takes the inspector by value, so a test keeps this
/// handle instead. Cloning it is cheap and every clone observes the same buffer.
#[derive(Clone, Debug, Default)]
pub struct EventLog {
    events: Arc<Mutex<Vec<String>>>,
}

impl EventLog {
    /// Returns a snapshot of the recorded events, in the order they happened.
    pub fn events(&self) -> Vec<String> {
        self.lock().clone()
    }

    fn push(&self, event: String) {
        self.lock().push(event);
    }

    fn lock(&self) -> parking_lot::MutexGuard<'_, Vec<String>> {
        self.events.lock()
    }
}

/// Records transaction and instruction callbacks as strings, in order.
///
/// Only the transaction/instruction hooks are overridden, so every recorded
/// event proves the inspector accepted the execution's origin: an internal
/// airdrop or a batch stage produces no events at all unless wrapped in
/// [`OriginFiltered`] with a wider [`OriginFilter`].
///
/// Use [`RecordingInspector::subscribe`] to obtain a recorder together with the
/// [`EventLog`] its events can be read from once the recorder has been moved
/// into the VM.
#[derive(Debug, Default)]
pub struct RecordingInspector {
    log: EventLog,
}

impl RecordingInspector {
    /// Creates a recorder and the live log it appends to.
    #[must_use]
    pub fn subscribe() -> (Self, EventLog) {
        let log = EventLog::default();
        (Self { log: log.clone() }, log)
    }

    fn record(&self, event: String) {
        self.log.push(event);
    }
}

impl<T: SvmTypesHost> Inspector<T> for RecordingInspector {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn transaction_start(&mut self, _svm: &Svm<'_, T>, _tx: &SanitizedTransaction) {
        self.record("start".to_string());
    }

    fn instruction_start(&mut self, index: usize, program_id: &Address) {
        self.record(format!("instruction_start {index} {program_id}"));
    }

    fn instruction_end(&mut self, index: usize, program_id: &Address) {
        self.record(format!("instruction_end {index} {program_id}"));
    }

    fn transaction_end(&mut self, _svm: &Svm<'_, T>, result: &TransactionResult<()>) {
        self.record(format!("end {}", result.is_ok()));
    }
}

/// A shared, cloneable log of the origins an origin-aware observer sees.
///
/// Like [`EventLog`], this is the handle to keep once the inspector itself has
/// been moved into the VM.
#[derive(Clone, Debug, Default)]
pub struct OriginLog {
    origins: Arc<Mutex<Vec<TransactionOrigin>>>,
}

impl OriginLog {
    /// Returns a snapshot of the observed origins, in the order they happened.
    pub fn origins(&self) -> Vec<TransactionOrigin> {
        self.lock().clone()
    }

    fn push(&self, origin: TransactionOrigin) {
        self.lock().push(origin);
    }

    fn lock(&self) -> parking_lot::MutexGuard<'_, Vec<TransactionOrigin>> {
        self.origins.lock()
    }
}

/// Records the origin of every execution the VM consults it for, including
/// the non-user ones.
///
/// Only [`Inspector::accepts_origin`] is overridden — it records the origin
/// and returns `true` — so this observer sees every origin exactly once per
/// execution: the VM consults once, before any hook fires. All hooks stay
/// no-ops.
#[derive(Debug, Default)]
pub struct OriginRecordingInspector {
    log: OriginLog,
}

impl OriginRecordingInspector {
    /// Creates a recorder and the live log it appends to.
    #[must_use]
    pub fn subscribe() -> (Self, OriginLog) {
        let log = OriginLog::default();
        (Self { log: log.clone() }, log)
    }
}

impl<T: SvmTypesHost> Inspector<T> for OriginRecordingInspector {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn accepts_origin(&self, origin: TransactionOrigin) -> bool {
        self.log.push(origin);
        true
    }
}

/// Counts the top-level instructions of every transaction it observes.
///
/// Only the instruction hook is overridden and the origin filter is the
/// default, so the count covers user traffic: the internal airdrop
/// transaction and batch stages never reach it.
#[derive(Debug, Default)]
pub struct CountingInspector {
    top_level_instructions: Arc<AtomicUsize>,
}

impl CountingInspector {
    /// Returns a live handle to the instruction counter.
    ///
    /// The handle stays valid after the inspector has been moved into the VM.
    #[must_use]
    pub fn counter(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.top_level_instructions)
    }
}

impl<T: SvmTypesHost> Inspector<T> for CountingInspector {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn instruction_start(&mut self, _index: usize, _program_id: &Address) {
        self.top_level_instructions.fetch_add(1, Ordering::SeqCst);
    }
}

/// Splits the observed top-level instructions by origin.
///
/// Hooks carry no origin, so each instruction is attributed to the origin the
/// VM consulted at the start of its execution: `accepts_origin` stashes the
/// consulted origin (accepting user traffic and internal airdrops, declining
/// batch stages) and `instruction_start` counts against the stash. This is
/// sound because the VM consults exactly once per execution, before any
/// instruction hook fires, and dispatches each VM's hooks serially.
#[derive(Debug, Default)]
pub struct OriginCountingInspector {
    user_instructions: Arc<AtomicUsize>,
    internal_airdrops: Arc<AtomicUsize>,
    last_origin: Mutex<Option<TransactionOrigin>>,
}

impl OriginCountingInspector {
    /// Returns a live handle to the user-instruction counter.
    #[must_use]
    pub fn user_instructions(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.user_instructions)
    }

    /// Returns a live handle to the internal-airdrop counter.
    #[must_use]
    pub fn internal_airdrops(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.internal_airdrops)
    }
}

impl<T: SvmTypesHost> Inspector<T> for OriginCountingInspector {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn accepts_origin(&self, origin: TransactionOrigin) -> bool {
        *self.last_origin.lock() = Some(origin);
        match origin {
            TransactionOrigin::User | TransactionOrigin::InternalAirdrop => true,
            // `TransactionOrigin` is `#[non_exhaustive]`, so a wildcard arm is
            // required. What it covers today — batch stages — is deliberately
            // not counted.
            _ => false,
        }
    }

    fn instruction_start(&mut self, _index: usize, _program_id: &Address) {
        let origin = *self.last_origin.lock();
        match origin {
            Some(TransactionOrigin::User) => {
                self.user_instructions.fetch_add(1, Ordering::SeqCst);
            }
            Some(TransactionOrigin::InternalAirdrop) => {
                self.internal_airdrops.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        }
    }
}
