//! Transaction inspection hooks.
//!
//! [`Inspector`] observes execution through `&mut self` hooks, so an
//! implementation accumulates state in plain owned fields instead of wrapping
//! every counter in an `Arc<AtomicUsize>` handle. The VM stores the installed
//! inspector as an `Option<Box<dyn Inspector<T>>>` and takes it out for each
//! dispatch: a hook that (against the documentation) re-enters execution
//! observes no inspector rather than deadlocking or aliasing it.
//!
//! Origin-aware behaviour lives in the [`OriginFiltered`] decorator, not in
//! the trait: every hook is origin-less, and the VM consults
//! [`Inspector::accepts_origin`] once per execution to decide whether the
//! hooks fire at all. Reusable implementations of the trait live in the
//! `hpsvm-inspectors` crate, which re-exports [`Inspector`],
//! [`TransactionOrigin`], [`OriginFilter`] and [`OriginFiltered`] so
//! downstream code needs a single import.

use std::{any::Any, marker::ConstParamTy};

use auto_impl::auto_impl;
use solana_address::Address;
use solana_transaction::sanitized::SanitizedTransaction;

use crate::{Svm, SvmTypesHost, state_change_sink::AccountChange};

/// Origin of a transaction observed by an [`Inspector`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransactionOrigin {
    /// A caller-submitted transaction or instruction helper.
    User,
    /// The internal transfer used by [`Svm::airdrop`].
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
    /// Returns `true` for caller-submitted traffic.
    pub const fn is_user(self) -> bool {
        matches!(self, Self::User)
    }
}

/// Which execution path the VM monomorphizes for inspector dispatch.
///
/// The public entries select the mode once per execution (at the internal
/// readonly execution entry point): an installed
/// inspector that accepts the current origin selects [`DynInspector`](Self::DynInspector),
/// everything else selects [`NoInspector`](Self::NoInspector). Inside the
/// `NoInspector` instantiation every hook dispatcher matches on `MODE` and
/// resolves to the empty arm at compile time, so the no-inspector path
/// contains no hook-branch code and never monomorphizes a `fire_inspector`
/// call.
///
/// HONEST LIMIT: mode selection itself still performs the single
/// `inspector_live()` mutex lock per execution — the installed-or-not fact
/// lives behind the lock, and avoiding it would need an atomic flag with its
/// own sync risk (out of scope). The win is everything after the selection:
/// zero hook-branch code in the `NoInspector` instantiation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ConstParamTy)]
pub enum InspectMode {
    /// No inspector accepted this execution: all hook dispatch compiles away.
    NoInspector,
    /// An installed inspector accepted this execution: hooks fire via dynamic
    /// dispatch, exactly as before.
    DynInspector,
}

impl InspectMode {
    /// Returns `true` when hooks fire under this mode.
    ///
    /// A `const fn` of a const-generic argument so call sites fold to a
    /// literal `true`/`false` per monomorphization.
    pub const fn is_live(self) -> bool {
        match self {
            Self::DynInspector => true,
            Self::NoInspector => false,
        }
    }
}

/// Observes transaction execution and accumulates inspection state.
///
/// The trait is parameterized by the VM's type family, so a hook receives the
/// very host the VM is instantiated with: an extension slot on a custom family
/// is reachable here without a downcast.
///
/// Every hook takes `&mut self` with a default no-op body, so an
/// implementation overrides only the hooks it cares about and keeps plain
/// owned state (`usize` counters, `Vec` logs) instead of shared atomic
/// handles. The hooks receive a shared `&Svm` — never `&mut Svm` — so firing
/// them from the readonly execution paths (`try_transact`,
/// `transact_detached`, `simulate_*`) needs no signature changes there, and a
/// hook cannot alias the VM mid-execution.
///
/// [`Inspector::accepts_origin`] is consulted once per execution, before any
/// hook fires. Its default accepts user traffic only, which preserves the
/// historical rule that the internal airdrop transfer and batch stages never
/// reach the hooks. Override it (directly, or via [`OriginFiltered`]) to
/// observe other origins.
///
/// [`Inspector::as_any`] and [`Inspector::as_any_mut`] exist so a boxed
/// inspector recovered through [`Svm::clear_inspector`] can be downcast back
/// to its concrete type with `downcast_ref`.
/// They are the only required methods; every hook defaults to a no-op. They
/// also bind every implementor to `'static` ([`Any`](std::any::Any) is
/// `'static`), so a non-`'static` type cannot implement this trait even
/// though the VM's storage is lifetime-generic.
#[auto_impl(Box, &mut)]
pub trait Inspector<T: SvmTypesHost>: Send + Sync {
    /// Decides whether any hook fires for a transaction of `origin`.
    ///
    /// The default accepts [`TransactionOrigin::User`] only.
    fn accepts_origin(&self, origin: TransactionOrigin) -> bool {
        origin.is_user()
    }

    /// Exposes the concrete type for `downcast_ref`.
    fn as_any(&self) -> &dyn Any;

    /// Exposes the concrete type for `downcast_mut`.
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Called once when the inspector is installed on a VM.
    fn initialize(&mut self, _svm: &Svm<'_, T>) {}

    /// Called immediately before top-level instruction processing begins.
    fn transaction_start(&mut self, _svm: &Svm<'_, T>, _tx: &SanitizedTransaction) {}

    /// Called before each top-level instruction is executed.
    fn instruction_start(&mut self, _index: usize, _program_id: &Address) {}

    /// Called after each top-level instruction is executed, including a
    /// failed one: the hook fires before the error propagates.
    fn instruction_end(&mut self, _index: usize, _program_id: &Address) {}

    /// Called once per collected log message, after instruction processing
    /// and before `transaction_end`.
    fn log(&mut self, _message: &str) {}

    /// Called once per committed account change, in staging order.
    ///
    /// This fires when a staged transaction commits
    /// ([`ExecutedTx::commit`](crate::ExecutedTx::commit) and
    /// [`commit_with`](crate::ExecutedTx::commit_with)), not on discard and
    /// not on the detached readonly paths.
    fn account_change(&mut self, _change: AccountChange<'_>) {}

    /// Called after top-level instruction processing completes, carrying the
    /// execution outcome.
    ///
    /// The payload is the raw outcome (`Ok(())` vs `Err(..)`), not the
    /// metadata-level [`TransactionResult`](crate::types::TransactionResult):
    /// metadata is built during commit-time conversion, which the readonly
    /// paths never perform, while the raw outcome exists on every path.
    fn transaction_end(
        &mut self,
        _svm: &Svm<'_, T>,
        _result: &solana_transaction_error::TransactionResult<()>,
    ) {
    }
}

impl<T: SvmTypesHost> dyn Inspector<T> + '_ {
    /// Returns the concrete inspector when it is `U`, or `None` otherwise.
    pub fn downcast_ref<U: 'static>(&self) -> Option<&U> {
        self.as_any().downcast_ref::<U>()
    }

    /// Returns the concrete inspector mutably when it is `U`, or `None`
    /// otherwise.
    pub fn downcast_mut<U: 'static>(&mut self) -> Option<&mut U> {
        self.as_any_mut().downcast_mut::<U>()
    }
}

/// Which transaction origins an [`OriginFiltered`] decorator forwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OriginFilter {
    /// Caller-submitted traffic only (the trait default).
    UserOnly,
    /// Every origin, including airdrops and batch stages.
    All,
    /// Batch stages only.
    BatchStages,
    /// The internal airdrop transfer only.
    InternalAirdrop,
    /// Every non-user origin (airdrops and batch stages).
    NonUser,
}

impl OriginFilter {
    /// Returns `true` when a transaction of `origin` passes the filter.
    pub const fn allows(self, origin: TransactionOrigin) -> bool {
        match (self, origin) {
            (Self::All, _) => true,
            (Self::UserOnly, TransactionOrigin::User) => true,
            (Self::BatchStages, TransactionOrigin::Batch { .. }) => true,
            (Self::InternalAirdrop, TransactionOrigin::InternalAirdrop) => true,
            (Self::NonUser, TransactionOrigin::User) => false,
            (Self::NonUser, _) => true,
            _ => false,
        }
    }
}

/// Origin-aware decorator for an [`Inspector`].
///
/// The VM consults only the outermost inspector's
/// [`accepts_origin`](Inspector::accepts_origin), so wrapping an inspector in
/// `OriginFiltered` replaces origin filtering without touching the trait's
/// hooks: set the filter to [`OriginFilter::All`] to observe every origin
/// (including the internal airdrop and batch stages the default suppresses),
/// or to [`OriginFilter::UserOnly`] to keep exactly the default. The inner
/// inspector's own `accepts_origin` is AND-combined, so decorators nest:
/// `OriginFiltered<OriginFiltered<I>>` forwards only origins both filters
/// allow.
///
/// The inner inspector must be `'static`: downcasting runs through
/// [`Any`](std::any::Any), which is `'static` by construction.
#[derive(Debug, Clone)]
pub struct OriginFiltered<I> {
    inner: I,
    filter: OriginFilter,
}

impl<I> OriginFiltered<I> {
    /// Wraps `inner` so only origins accepted by `filter` reach its hooks.
    pub const fn new(inner: I, filter: OriginFilter) -> Self {
        Self { inner, filter }
    }

    /// Returns the wrapped inspector.
    pub const fn inner(&self) -> &I {
        &self.inner
    }

    /// Returns the wrapped inspector mutably.
    pub fn inner_mut(&mut self) -> &mut I {
        &mut self.inner
    }

    /// Returns the configured filter.
    pub const fn filter(&self) -> OriginFilter {
        self.filter
    }

    /// Unwraps the decorator.
    pub fn into_inner(self) -> I {
        self.inner
    }
}

impl<T: SvmTypesHost, I: Inspector<T> + 'static> Inspector<T> for OriginFiltered<I> {
    fn accepts_origin(&self, origin: TransactionOrigin) -> bool {
        self.filter.allows(origin) && self.inner.accepts_origin(origin)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn initialize(&mut self, svm: &Svm<'_, T>) {
        self.inner.initialize(svm);
    }

    fn transaction_start(&mut self, svm: &Svm<'_, T>, tx: &SanitizedTransaction) {
        self.inner.transaction_start(svm, tx);
    }

    fn instruction_start(&mut self, index: usize, program_id: &Address) {
        self.inner.instruction_start(index, program_id);
    }

    fn instruction_end(&mut self, index: usize, program_id: &Address) {
        self.inner.instruction_end(index, program_id);
    }

    fn log(&mut self, message: &str) {
        self.inner.log(message);
    }

    fn account_change(&mut self, change: AccountChange<'_>) {
        self.inner.account_change(change);
    }

    fn transaction_end(
        &mut self,
        svm: &Svm<'_, T>,
        result: &solana_transaction_error::TransactionResult<()>,
    ) {
        self.inner.transaction_end(svm, result);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use solana_keypair::Keypair;
    use solana_message::{Message, VersionedMessage};
    use solana_signer::Signer;
    use solana_system_interface::instruction::transfer;
    use solana_transaction::versioned::VersionedTransaction;
    use solana_transaction_error::TransactionResult;

    use super::*;
    use crate::{BaseSvmTypes, Hpsvm, Svm};

    /// Builds a minimal sanitized transfer transaction; the inspector hooks
    /// only ever receive it as an opaque payload.
    fn sanitized_transfer(svm: &Svm<'_, BaseSvmTypes>) -> SanitizedTransaction {
        let payer = Keypair::new();
        let recipient = Address::new_unique();
        let message =
            Message::new(&[transfer(&payer.pubkey(), &recipient, 1)], Some(&payer.pubkey()));
        let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(message), &[&payer])
            .expect("unsigned transfer should sanitize");
        svm.sanitize_transaction_no_verify(tx).expect("sanitize should succeed")
    }

    /// Records hook events into a plain `Vec`: `&mut self` hooks need no
    /// shared handle.
    ///
    /// This is a fixture for the trait's own tests, not a reusable inspector:
    /// the reusable implementations live in the `hpsvm-inspectors` crate, which
    /// depends on `hpsvm`, so this module cannot import them without putting two
    /// copies of `hpsvm` in one build.
    #[derive(Default)]
    struct RecordingInspector {
        events: Vec<String>,
    }

    impl Inspector<BaseSvmTypes> for RecordingInspector {
        fn as_any(&self) -> &dyn Any {
            self
        }

        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }

        fn transaction_start(&mut self, _svm: &Svm<'_, BaseSvmTypes>, _tx: &SanitizedTransaction) {
            self.events.push("start".to_string());
        }

        fn instruction_start(&mut self, index: usize, program_id: &Address) {
            self.events.push(format!("instruction_start {index} {program_id}"));
        }

        fn instruction_end(&mut self, index: usize, program_id: &Address) {
            self.events.push(format!("instruction_end {index} {program_id}"));
        }

        fn transaction_end(
            &mut self,
            _svm: &Svm<'_, BaseSvmTypes>,
            result: &TransactionResult<()>,
        ) {
            self.events.push(format!("end {}", result.is_ok()));
        }
    }

    /// Records every consulted origin: `accepts_origin` is the only
    /// origin-touching method, so observing origins means recording there.
    /// It takes `&self`, hence the lock; everything else stays a no-op.
    struct OriginRecorder {
        origins: Mutex<Vec<TransactionOrigin>>,
    }

    impl OriginRecorder {
        fn origins(&self) -> Vec<TransactionOrigin> {
            self.origins.lock().expect("origin recorder lock").clone()
        }
    }

    impl Inspector<BaseSvmTypes> for OriginRecorder {
        fn as_any(&self) -> &dyn Any {
            self
        }

        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }

        fn accepts_origin(&self, origin: TransactionOrigin) -> bool {
            self.origins.lock().expect("origin recorder lock").push(origin);
            true
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

    /// The default filter accepts user traffic and drops everything else.
    #[test]
    fn default_accepts_origin_forwards_user_traffic_only() {
        let inspector = RecordingInspector::default();
        assert!(inspector.accepts_origin(TransactionOrigin::User));
        for origin in origins().into_iter().filter(|origin| !origin.is_user()) {
            assert!(!inspector.accepts_origin(origin), "origin {origin:?}");
        }
    }

    #[test]
    fn origin_filter_allows_exactly_its_documented_origins() {
        let batch = TransactionOrigin::Batch { stage_index: 1, transaction_index: 2 };
        let cases = [
            (OriginFilter::UserOnly, TransactionOrigin::User, true),
            (OriginFilter::UserOnly, TransactionOrigin::InternalAirdrop, false),
            (OriginFilter::UserOnly, batch, false),
            (OriginFilter::All, TransactionOrigin::User, true),
            (OriginFilter::All, TransactionOrigin::InternalAirdrop, true),
            (OriginFilter::All, batch, true),
            (OriginFilter::BatchStages, batch, true),
            (OriginFilter::BatchStages, TransactionOrigin::User, false),
            (OriginFilter::BatchStages, TransactionOrigin::InternalAirdrop, false),
            (OriginFilter::InternalAirdrop, TransactionOrigin::InternalAirdrop, true),
            (OriginFilter::InternalAirdrop, TransactionOrigin::User, false),
            (OriginFilter::InternalAirdrop, batch, false),
            (OriginFilter::NonUser, TransactionOrigin::User, false),
            (OriginFilter::NonUser, TransactionOrigin::InternalAirdrop, true),
            (OriginFilter::NonUser, batch, true),
        ];
        for (filter, origin, expected) in cases {
            assert_eq!(filter.allows(origin), expected, "{filter:?} on {origin:?}");
        }
    }

    /// The decorator forwards hooks when its filter passes and suppresses
    /// them otherwise.
    #[test]
    fn origin_filtered_forwards_hooks_only_for_allowed_origins() {
        let svm = Hpsvm::new();
        let tx = sanitized_transfer(&svm);
        let program_id = Address::new_unique();

        let mut allowed =
            OriginFiltered::new(RecordingInspector::default(), OriginFilter::UserOnly);
        assert!(allowed.accepts_origin(TransactionOrigin::User));
        allowed.transaction_start(&svm, &tx);
        allowed.instruction_start(0, &program_id);
        allowed.instruction_end(0, &program_id);
        assert_eq!(
            allowed.inner().events,
            vec![
                "start".to_string(),
                format!("instruction_start 0 {program_id}"),
                format!("instruction_end 0 {program_id}"),
            ]
        );

        let blocked = OriginFiltered::new(RecordingInspector::default(), OriginFilter::BatchStages);
        assert!(!blocked.accepts_origin(TransactionOrigin::User));
        assert!(blocked.inner().events.is_empty());
    }

    /// Nested decorators AND-combine their filters: the inner `All` would
    /// pass the batch origin, but the outer `UserOnly` stops it.
    #[test]
    fn nested_origin_filtered_decorators_combine_their_filters() {
        type Nested = OriginFiltered<OriginFiltered<RecordingInspector>>;
        let batch = TransactionOrigin::Batch { stage_index: 0, transaction_index: 0 };
        let nested = Nested::new(
            OriginFiltered::new(RecordingInspector::default(), OriginFilter::All),
            OriginFilter::UserOnly,
        );
        assert!(nested.accepts_origin(TransactionOrigin::User));
        assert!(!nested.accepts_origin(batch));
        assert!(!nested.accepts_origin(TransactionOrigin::InternalAirdrop));
    }

    /// A boxed inspector downcasts back to its concrete type, and to nothing
    /// else — including through the `Box` blanket impl.
    #[test]
    fn boxed_inspector_downcasts_to_its_concrete_type_only() {
        let boxed: Box<dyn Inspector<BaseSvmTypes>> = Box::new(RecordingInspector::default());
        assert!(boxed.downcast_ref::<RecordingInspector>().is_some());
        assert!(boxed.downcast_ref::<OriginRecorder>().is_none());

        let mut reboxed: Box<dyn Inspector<BaseSvmTypes>> =
            Box::new(OriginFiltered::new(RecordingInspector::default(), OriginFilter::All));
        assert!(reboxed.downcast_ref::<OriginFiltered<RecordingInspector>>().is_some());
        assert!(reboxed.downcast_mut::<OriginFiltered<RecordingInspector>>().is_some());
        assert!(reboxed.downcast_ref::<RecordingInspector>().is_none());
    }

    /// A `&mut` borrow of an inspector is itself an inspector, so hooks can
    /// be driven through a reborrow without moving the value.
    #[test]
    fn mutably_borrowed_inspector_forwards_every_hook() {
        let svm = Hpsvm::new();
        let tx = sanitized_transfer(&svm);
        let program_id = Address::new_unique();
        let mut inspector = RecordingInspector::default();
        let borrowed: &mut dyn Inspector<BaseSvmTypes> = &mut inspector;
        borrowed.transaction_start(&svm, &tx);
        borrowed.instruction_start(2, &program_id);
        borrowed.instruction_end(2, &program_id);
        assert_eq!(
            inspector.events,
            vec![
                "start".to_string(),
                format!("instruction_start 2 {program_id}"),
                format!("instruction_end 2 {program_id}"),
            ]
        );
    }

    /// Every hook with no execution behind it fires by direct call: CPI,
    /// log, account-change and initialize hooks have no other coverage until
    /// their firing sites exist.
    #[test]
    fn every_hook_fires_by_direct_call() {
        let svm = Hpsvm::new();
        let tx = sanitized_transfer(&svm);
        let program_id = Address::new_unique();
        let mut inspector = RecordingInspector::default();

        inspector.initialize(&svm);
        inspector.transaction_start(&svm, &tx);
        inspector.instruction_start(0, &program_id);
        inspector.instruction_end(0, &program_id);
        inspector.log("hello");
        inspector.account_change(AccountChange { address: program_id, before: None, after: None });

        // `RecordingInspector` overrides only the transaction/instruction
        // hooks, so exactly those record events; the rest stay no-ops.
        assert_eq!(
            inspector.events,
            vec![
                "start".to_string(),
                format!("instruction_start 0 {program_id}"),
                format!("instruction_end 0 {program_id}"),
            ]
        );
    }

    /// An origin-aware override observes every consulted origin, so the
    /// recorder must not filter before reporting it.
    #[test]
    fn origin_recorder_observes_every_consulted_origin() {
        let recorder = OriginRecorder { origins: Mutex::new(Vec::new()) };
        for origin in origins() {
            assert!(recorder.accepts_origin(origin));
        }
        assert_eq!(recorder.origins(), origins());
    }

    /// An empty implementation is a no-op on every hook, so installing one
    /// never perturbs execution.
    #[test]
    fn empty_implementation_accepts_every_hook_as_a_no_op() {
        struct Empty;

        impl Inspector<BaseSvmTypes> for Empty {
            fn as_any(&self) -> &dyn Any {
                self
            }

            fn as_any_mut(&mut self) -> &mut dyn Any {
                self
            }
        }

        let svm = Hpsvm::new();
        let tx = sanitized_transfer(&svm);
        let program_id = Address::new_unique();
        let mut inspector = Empty;

        assert!(inspector.accepts_origin(TransactionOrigin::User));
        inspector.initialize(&svm);
        inspector.transaction_start(&svm, &tx);
        inspector.instruction_start(0, &program_id);
        inspector.instruction_end(0, &program_id);
        inspector.log("hello");
        inspector.account_change(AccountChange { address: program_id, before: None, after: None });
    }
}
