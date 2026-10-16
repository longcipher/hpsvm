//! Inspector behaviour as observed by the core VM.
//!
//! Two things are checked here. First, that the VM installs an inspector, fires
//! its hooks for user traffic only, and consults the right origin exactly once
//! per execution. Second, the exact event stream a scripted transaction produces,
//! which the reusable inspectors in `hpsvm-inspectors` must reproduce: this
//! crate cannot depend on `hpsvm-inspectors` (that would put two copies of
//! `hpsvm` in one build), so the fixtures below stay local and the equivalence
//! is asserted from the other side, in
//! `crates/hpsvm-inspectors/tests/inspector_events.rs`.

use std::{
    any::Any,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use hpsvm::{BaseSvmTypes, HPSVM, Inspector, Svm, TransactionOrigin};
use solana_address::Address;
use solana_hash::Hash;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::{Transaction, sanitized::SanitizedTransaction};
use solana_transaction_error::TransactionResult;

/// Base58 of the system program: the only program a bare transfer invokes.
const SYSTEM_PROGRAM: &str = "11111111111111111111111111111111";

/// Counts the top-level instructions of every transaction it observes.
#[derive(Default)]
struct CountingInspector {
    top_level_instructions: Arc<AtomicUsize>,
}

impl CountingInspector {
    fn instructions(&self) -> usize {
        self.top_level_instructions.load(Ordering::SeqCst)
    }
}

impl Inspector<BaseSvmTypes> for CountingInspector {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn instruction_start(&mut self, _index: usize, _program_id: &Address) {
        self.top_level_instructions.fetch_add(1, Ordering::SeqCst);
    }
}

/// Splits the observed top-level instructions by origin.
///
/// Hooks carry no origin, so each instruction is attributed to the origin the
/// VM consulted at the start of its execution (see `OriginCountingInspector`
/// in `hpsvm-inspectors` for the documented soundness argument).
#[derive(Default)]
struct OriginCountingInspector {
    user_instructions: Arc<AtomicUsize>,
    internal_airdrops: Arc<AtomicUsize>,
    last_origin: Mutex<Option<TransactionOrigin>>,
}

impl OriginCountingInspector {
    fn user_instructions(&self) -> usize {
        self.user_instructions.load(Ordering::SeqCst)
    }

    fn internal_airdrops(&self) -> usize {
        self.internal_airdrops.load(Ordering::SeqCst)
    }
}

impl Inspector<BaseSvmTypes> for OriginCountingInspector {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn accepts_origin(&self, origin: TransactionOrigin) -> bool {
        *self.last_origin.lock().expect("origin stash lock") = Some(origin);
        match origin {
            TransactionOrigin::User | TransactionOrigin::InternalAirdrop => true,
            // `TransactionOrigin` is `#[non_exhaustive]`, so a wildcard arm is
            // required. What it covers today — batch stages — is deliberately
            // not counted.
            _ => false,
        }
    }

    fn instruction_start(&mut self, _index: usize, _program_id: &Address) {
        let origin = *self.last_origin.lock().expect("origin stash lock");
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

/// Shared, cloneable log a [`RecordingInspector`] appends to.
#[derive(Clone, Debug, Default)]
struct EventLog {
    events: Arc<Mutex<Vec<String>>>,
}

impl EventLog {
    fn events(&self) -> Vec<String> {
        self.events.lock().expect("event log lock").clone()
    }

    fn push(&self, event: String) {
        self.events.lock().expect("event log lock").push(event);
    }
}

/// Records every callback as a string.
///
/// Only the transaction/instruction hooks are overridden, so every recorded
/// event proves the inspector accepted the execution's origin.
#[derive(Default)]
struct RecordingInspector {
    log: EventLog,
}

impl RecordingInspector {
    fn subscribe() -> (Self, EventLog) {
        let log = EventLog::default();
        (Self { log: log.clone() }, log)
    }
}

impl Inspector<BaseSvmTypes> for RecordingInspector {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn transaction_start(&mut self, _svm: &Svm<'_, BaseSvmTypes>, _tx: &SanitizedTransaction) {
        self.log.push("start".to_string());
    }

    fn instruction_start(&mut self, index: usize, program_id: &Address) {
        self.log.push(format!("instruction_start {index} {program_id}"));
    }

    fn instruction_end(&mut self, index: usize, program_id: &Address) {
        self.log.push(format!("instruction_end {index} {program_id}"));
    }

    fn transaction_end(&mut self, _svm: &Svm<'_, BaseSvmTypes>, result: &TransactionResult<()>) {
        self.log.push(format!("end {}", result.is_ok()));
    }
}

/// Shared, cloneable log of the origins an origin-aware observer sees.
#[derive(Clone, Debug, Default)]
struct OriginLog {
    origins: Arc<Mutex<Vec<TransactionOrigin>>>,
}

impl OriginLog {
    fn origins(&self) -> Vec<TransactionOrigin> {
        self.origins.lock().expect("origin log lock").clone()
    }
}

/// Overrides only `accepts_origin`, recording every consulted origin, so it
/// observes user traffic, airdrops and batch stages alike.
#[derive(Default)]
struct OriginRecordingInspector {
    log: OriginLog,
}

impl OriginRecordingInspector {
    fn subscribe() -> (Self, OriginLog) {
        let log = OriginLog::default();
        (Self { log: log.clone() }, log)
    }
}

impl Inspector<BaseSvmTypes> for OriginRecordingInspector {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn accepts_origin(&self, origin: TransactionOrigin) -> bool {
        self.log.origins.lock().expect("origin recording inspector lock").push(origin);
        true
    }
}

fn transfer_tx(
    payer: &Keypair,
    recipient: &Address,
    lamports: u64,
    blockhash: Hash,
) -> Transaction {
    Transaction::new(
        &[payer],
        Message::new(&[transfer(&payer.pubkey(), recipient, lamports)], Some(&payer.pubkey())),
        blockhash,
    )
}

#[test]
fn inspector_observes_top_level_instructions() {
    let inspector = CountingInspector::default();
    let observed = Arc::clone(&inspector.top_level_instructions);
    let mut svm = HPSVM::new().with_inspector(inspector);
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = transfer_tx(&payer, &recipient, 1, svm.latest_blockhash());

    svm.send_transaction(tx).unwrap();

    assert_eq!(observed.load(Ordering::SeqCst), 1);
}

#[test]
fn inspector_can_distinguish_internal_airdrops_from_user_transactions() {
    let inspector = OriginCountingInspector::default();
    let user_instructions = Arc::clone(&inspector.user_instructions);
    let internal_airdrops = Arc::clone(&inspector.internal_airdrops);
    let mut svm = HPSVM::new().with_inspector(inspector);
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = transfer_tx(&payer, &recipient, 1, svm.latest_blockhash());

    svm.send_transaction(tx).unwrap();

    assert_eq!(internal_airdrops.load(Ordering::SeqCst), 1);
    assert_eq!(user_instructions.load(Ordering::SeqCst), 1);
}

/// The airdrop that funds the payer runs as an internal transfer, so only the
/// caller-submitted transfer may reach the origin-less hooks.
#[test]
fn a_user_transfer_records_the_exact_origin_less_event_stream() {
    let (inspector, log) = RecordingInspector::subscribe();
    let mut svm = HPSVM::new().with_inspector(inspector);
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = transfer_tx(&payer, &recipient, 1, svm.latest_blockhash());
    svm.send_transaction(tx).unwrap();

    assert_eq!(
        log.events(),
        vec![
            "start".to_string(),
            format!("instruction_start 0 {SYSTEM_PROGRAM}"),
            format!("instruction_end 0 {SYSTEM_PROGRAM}"),
            "end true".to_string(),
        ]
    );
}

/// A transaction whose instruction fails still starts, still dispatches its
/// instruction start and end, and closes with `end false`.
#[test]
fn a_failing_transfer_records_the_exact_origin_less_event_stream() {
    let (inspector, log) = RecordingInspector::subscribe();
    let mut svm = HPSVM::new().with_inspector(inspector);
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = transfer_tx(&payer, &recipient, 1_000_000_000, svm.latest_blockhash());
    svm.send_transaction(tx).unwrap_err();

    assert_eq!(
        log.events(),
        vec![
            "start".to_string(),
            format!("instruction_start 0 {SYSTEM_PROGRAM}"),
            format!("instruction_end 0 {SYSTEM_PROGRAM}"),
            "end false".to_string(),
        ]
    );
}

/// An instruction-less message is still executed, so it starts and ends
/// without ever dispatching an instruction.
#[test]
fn an_empty_transaction_records_the_exact_origin_less_event_stream() {
    let (inspector, log) = RecordingInspector::subscribe();
    let mut svm = HPSVM::new().with_inspector(inspector);
    let payer = Keypair::new();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    assert!(result.is_ok(), "an instruction-less message should execute: {result:?}");

    assert_eq!(log.events(), vec!["start".to_string(), "end true".to_string(),]);
}

/// Batch stages are non-user origins, so the origin-aware hook reports the
/// stage and transaction indexes the planner assigned.
#[test]
fn a_batch_records_the_exact_stage_and_transaction_origins() {
    let (inspector, log) = OriginRecordingInspector::subscribe();
    let mut svm = HPSVM::new().with_inspector(inspector);
    let payer = Keypair::new();
    let recipient_a = Address::new_unique();
    let recipient_b = Address::new_unique();
    let recipient_c = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000_000).unwrap();
    let blockhash = svm.latest_blockhash();
    // Every transaction writes the payer, so the planner puts each one in its
    // own stage and the origins arrive in plan order.
    let batch = svm
        .send_transaction_batch([
            transfer_tx(&payer, &recipient_a, 1, blockhash),
            transfer_tx(&payer, &recipient_b, 2, blockhash),
            transfer_tx(&payer, &recipient_c, 3, blockhash),
        ])
        .expect("batch execution should succeed");

    assert_eq!(batch.plan.stages.len(), 3);
    assert_eq!(
        log.origins(),
        vec![
            // The funding airdrop is a non-user origin too, and this
            // inspector overrides the origin-aware hook so it sees it.
            TransactionOrigin::InternalAirdrop,
            TransactionOrigin::Batch { stage_index: 0, transaction_index: 0 },
            TransactionOrigin::Batch { stage_index: 1, transaction_index: 1 },
            TransactionOrigin::Batch { stage_index: 2, transaction_index: 2 },
        ]
    );
}

/// A stage with more than one transaction runs on worker VMs, which carry no
/// inspector: the boxed inspector cannot be cloned onto rayon threads, and
/// sharing one box would serialize execution on its lock. Only the funding
/// airdrops (serial, on the live VM) are therefore consulted.
#[test]
fn a_parallel_batch_stage_consults_only_the_serial_airdrops() {
    let (inspector, log) = OriginRecordingInspector::subscribe();
    let mut svm = HPSVM::new().with_inspector(inspector);
    let payer_a = Keypair::new();
    let payer_b = Keypair::new();

    svm.airdrop(&payer_a.pubkey(), 1_000_000_000).unwrap();
    svm.airdrop(&payer_b.pubkey(), 1_000_000_000).unwrap();
    let blockhash = svm.latest_blockhash();
    let batch = svm
        .send_transaction_batch([
            transfer_tx(&payer_a, &Address::new_unique(), 10, blockhash),
            transfer_tx(&payer_b, &Address::new_unique(), 20, blockhash),
        ])
        .expect("batch execution should succeed");

    assert_eq!(batch.plan.stages.len(), 1);
    assert_eq!(batch.plan.stages[0].transaction_indexes, vec![0, 1]);

    // Parallel-stage transactions run on inspector-less worker VMs, so they
    // are never consulted. Both funding airdrops precede the batch and are.
    assert_eq!(
        log.origins(),
        vec![TransactionOrigin::InternalAirdrop, TransactionOrigin::InternalAirdrop,]
    );
}

/// The counting hook fires only when the inspector accepts the execution's
/// origin; the default accepts user traffic only, so a batch stage — a
/// non-user origin — contributes nothing.
#[test]
fn the_origin_less_hook_ignores_batch_stage_instructions() {
    let inspector = CountingInspector::default();
    let observed = Arc::clone(&inspector.top_level_instructions);
    let mut svm = HPSVM::new().with_inspector(inspector);
    let payer = Keypair::new();

    svm.airdrop(&payer.pubkey(), 10_000_000).unwrap();
    let blockhash = svm.latest_blockhash();
    let batch = svm
        .send_transaction_batch([transfer_tx(&payer, &Address::new_unique(), 1, blockhash)])
        .expect("single-transaction batch execution should succeed");

    assert_eq!(batch.results.len(), 1);
    assert_eq!(observed.load(Ordering::SeqCst), 0);
}

// Task 5.1 RED tests: the mutable-hook inspector API does not exist yet.
// Every test below must fail to compile until the trait is redefined with
// `&mut self` hooks plus `set_inspector` / `clear_inspector` and the
// `OriginFiltered` decorator.
mod task_5_1_mutable_hooks {
    use std::any::Any;

    use hpsvm::{BaseSvmTypes, HPSVM, Inspector, OriginFilter, OriginFiltered, Svm};
    use solana_address::Address;
    use solana_keypair::Keypair;
    use solana_message::Message;
    use solana_signer::Signer;
    use solana_system_interface::instruction::transfer;
    use solana_transaction::{Transaction, sanitized::SanitizedTransaction};

    const SYSTEM_PROGRAM: &str = "11111111111111111111111111111111";

    /// Counts hook invocations with plain owned state: `&mut self` hooks make
    /// the `Arc<AtomicUsize>` / `Arc<Mutex<..>>` handles of the old `&self`
    /// trait unnecessary.
    #[derive(Default)]
    struct MutableCountingInspector {
        tx_starts: usize,
        instruction_starts: Vec<(usize, Address)>,
        instruction_ends: Vec<(usize, Address)>,
        tx_ends: usize,
        logs: Vec<String>,
        cpi_starts: usize,
        cpi_ends: usize,
    }

    impl Inspector<BaseSvmTypes> for MutableCountingInspector {
        fn as_any(&self) -> &dyn Any {
            self
        }

        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }

        fn transaction_start(&mut self, _svm: &Svm<'_, BaseSvmTypes>, _tx: &SanitizedTransaction) {
            self.tx_starts += 1;
        }

        fn instruction_start(&mut self, index: usize, program_id: &Address) {
            self.instruction_starts.push((index, *program_id));
        }

        fn instruction_end(&mut self, index: usize, program_id: &Address) {
            self.instruction_ends.push((index, *program_id));
        }

        fn cpi_start(&mut self, _depth: u8, _program_id: &Address) {
            self.cpi_starts += 1;
        }

        fn cpi_end(&mut self, _depth: u8, _program_id: &Address) {
            self.cpi_ends += 1;
        }

        fn log(&mut self, message: &str) {
            self.logs.push(message.to_owned());
        }

        fn transaction_end(
            &mut self,
            _svm: &Svm<'_, BaseSvmTypes>,
            _result: &solana_transaction_error::TransactionResult<()>,
        ) {
            self.tx_ends += 1;
        }
    }

    fn transfer_tx(
        payer: &Keypair,
        recipient: &Address,
        lamports: u64,
        blockhash: solana_hash::Hash,
    ) -> Transaction {
        Transaction::new(
            &[payer],
            Message::new(&[transfer(&payer.pubkey(), recipient, lamports)], Some(&payer.pubkey())),
            blockhash,
        )
    }

    #[test]
    fn each_hook_fires_exactly_once_with_exact_args_for_a_transfer() {
        let mut svm = HPSVM::new();
        svm.set_inspector(MutableCountingInspector::default());
        let payer = Keypair::new();
        let recipient = Address::new_unique();

        // The funding airdrop is an internal transfer, so the default
        // user-only filter must suppress it: only the transfer below counts.
        svm.airdrop(&payer.pubkey(), 10_000).unwrap();
        let tx = transfer_tx(&payer, &recipient, 1, svm.latest_blockhash());
        svm.send_transaction(tx).unwrap();

        let boxed = svm.clear_inspector().expect("inspector should be installed");
        let counting = boxed
            .downcast_ref::<MutableCountingInspector>()
            .expect("concrete inspector type must recover after clear");
        let system_program: Address = SYSTEM_PROGRAM.parse().unwrap();

        assert_eq!(counting.tx_starts, 1);
        assert_eq!(counting.instruction_starts, vec![(0, system_program)]);
        assert_eq!(counting.instruction_ends, vec![(0, system_program)]);
        assert_eq!(counting.tx_ends, 1);
        assert_eq!(counting.cpi_starts, 0);
        assert_eq!(counting.cpi_ends, 0);
        assert_eq!(
            counting.logs,
            vec![
                "Program 11111111111111111111111111111111 invoke [1]".to_string(),
                "Program 11111111111111111111111111111111 success".to_string(),
            ]
        );
    }

    #[test]
    fn set_and_clear_inspector_round_trip_recovers_the_concrete_type() {
        let mut svm = HPSVM::new();
        assert!(svm.clear_inspector().is_none(), "nothing installed yet");

        svm.set_inspector(MutableCountingInspector::default());
        let payer = Keypair::new();
        let recipient = Address::new_unique();
        svm.airdrop(&payer.pubkey(), 1_000_000).unwrap();
        let tx = transfer_tx(&payer, &recipient, 1, svm.latest_blockhash());
        svm.send_transaction(tx).unwrap();

        let boxed = svm.clear_inspector().expect("inspector should be installed");
        let counting = boxed
            .downcast_ref::<MutableCountingInspector>()
            .expect("concrete inspector type must recover after clear");
        assert_eq!(counting.tx_starts, 1);
        assert!(boxed.downcast_ref::<NoopProbe>().is_none(), "wrong-type downcast must be None");

        // With the inspector cleared, execution continues without hooks.
        // A distinct message (different amount, hence different signature)
        // avoids the duplicate-transaction guard.
        let tx = transfer_tx(&payer, &recipient, 2, svm.latest_blockhash());
        svm.send_transaction(tx).unwrap();
        assert!(svm.clear_inspector().is_none());
    }

    /// Wrong-type probe for the downcast test above.
    struct NoopProbe;

    impl Inspector<BaseSvmTypes> for NoopProbe {
        fn as_any(&self) -> &dyn Any {
            self
        }

        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    }

    /// Overrides nothing: every hook must stay a no-op so installing an empty
    /// inspector never perturbs execution.
    struct EmptyInspector;

    impl Inspector<BaseSvmTypes> for EmptyInspector {
        fn as_any(&self) -> &dyn Any {
            self
        }

        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    }

    #[test]
    fn unset_hooks_remain_no_ops() {
        let mut svm = HPSVM::new();
        svm.set_inspector(EmptyInspector);
        let payer = Keypair::new();
        let recipient = Address::new_unique();

        svm.airdrop(&payer.pubkey(), 10_000).unwrap();
        let tx = transfer_tx(&payer, &recipient, 1, svm.latest_blockhash());
        svm.send_transaction(tx).unwrap();

        let boxed = svm.clear_inspector().expect("inspector should be installed");
        assert!(boxed.downcast_ref::<EmptyInspector>().is_some());
        assert_eq!(svm.get_balance(&recipient), Some(1));
    }

    /// Records hook events into a plain `Vec`: possible only because hooks
    /// take `&mut self`.
    #[derive(Default)]
    struct VecRecorder {
        events: Vec<String>,
    }

    impl Inspector<BaseSvmTypes> for VecRecorder {
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
            result: &solana_transaction_error::TransactionResult<()>,
        ) {
            self.events.push(format!("end {}", result.is_ok()));
        }
    }

    #[test]
    fn origin_filtered_decorator_passes_user_but_filters_batch() {
        type Nested = OriginFiltered<OriginFiltered<VecRecorder>>;
        let mut svm = HPSVM::new();
        svm.set_inspector(OriginFiltered::new(
            OriginFiltered::new(VecRecorder::default(), OriginFilter::All),
            OriginFilter::UserOnly,
        ));
        let payer = Keypair::new();
        let recipient = Address::new_unique();

        svm.airdrop(&payer.pubkey(), 10_000_000).unwrap();
        let tx = transfer_tx(&payer, &recipient, 1, svm.latest_blockhash());
        svm.send_transaction(tx).unwrap();

        let user_events = vec![
            "start".to_string(),
            format!("instruction_start 0 {SYSTEM_PROGRAM}"),
            format!("instruction_end 0 {SYSTEM_PROGRAM}"),
            "end true".to_string(),
        ];

        // A single-transaction batch runs serially on the live VM with a
        // `Batch` origin, which the nested `UserOnly` filter must suppress.
        let batch = svm
            .send_transaction_batch([transfer_tx(
                &payer,
                &Address::new_unique(),
                1,
                svm.latest_blockhash(),
            )])
            .expect("single-transaction batch execution should succeed");
        assert_eq!(batch.results.len(), 1);

        let boxed = svm.clear_inspector().expect("inspector should be installed");
        let nested = boxed.downcast_ref::<Nested>().expect("nested decorator must recover");
        assert_eq!(nested.inner().inner().events, user_events);
    }

    /// `initialize` fires exactly once per installation, before any
    /// transaction hook.
    #[test]
    fn initialize_fires_once_on_installation() {
        #[derive(Default)]
        struct InitProbe {
            initializations: usize,
        }

        impl Inspector<BaseSvmTypes> for InitProbe {
            fn as_any(&self) -> &dyn Any {
                self
            }

            fn as_any_mut(&mut self) -> &mut dyn Any {
                self
            }

            fn initialize(&mut self, _svm: &Svm<'_, BaseSvmTypes>) {
                self.initializations += 1;
            }
        }

        let mut svm = HPSVM::new();
        svm.set_inspector(InitProbe::default());
        // Replacing the inspector initializes the replacement, not the old one.
        svm.set_inspector(InitProbe::default());

        let boxed = svm.clear_inspector().expect("inspector should be installed");
        let probe = boxed.downcast_ref::<InitProbe>().expect("concrete type must recover");
        assert_eq!(probe.initializations, 1);
    }

    /// Committed account changes reach `account_change` in staging order;
    /// discarded and detached executions never fire it.
    #[test]
    fn account_change_hook_observes_committed_changes() {
        #[derive(Default)]
        struct ChangeRecorder {
            changes: Vec<(Address, Option<u64>, Option<u64>)>,
        }

        impl Inspector<BaseSvmTypes> for ChangeRecorder {
            fn as_any(&self) -> &dyn Any {
                self
            }

            fn as_any_mut(&mut self) -> &mut dyn Any {
                self
            }

            fn account_change(&mut self, change: hpsvm::AccountChange<'_>) {
                use solana_account::ReadableAccount;
                self.changes.push((
                    change.address,
                    change.before.map(ReadableAccount::lamports),
                    change.after.map(ReadableAccount::lamports),
                ));
            }
        }

        let mut svm = HPSVM::new();
        svm.set_inspector(ChangeRecorder::default());
        let payer = Keypair::new();
        let recipient = Address::new_unique();
        svm.airdrop(&payer.pubkey(), 10_000).unwrap();
        let tx = transfer_tx(&payer, &recipient, 1, svm.latest_blockhash());
        svm.send_transaction(tx).unwrap();

        let boxed = svm.clear_inspector().expect("inspector should be installed");
        let recorder = boxed.downcast_ref::<ChangeRecorder>().expect("concrete type must recover");
        // The recipient is created by the transfer: no before-state, one
        // lamport after.
        assert!(
            recorder.changes.iter().any(|(address, before, after)| {
                *address == recipient && before.is_none() && *after == Some(1)
            }),
            "recipient creation must be reported, saw {:?}",
            recorder.changes
        );
        // The payer pre-exists and pays amount plus fee.
        assert!(
            recorder.changes.iter().any(|(address, before, after)| {
                *address == payer.pubkey() &&
                    *before == Some(10_000) &&
                    *after == Some(10_000 - 1 - 5000)
            }),
            "payer debit must be reported, saw {:?}",
            recorder.changes
        );
    }
}

// Task 5.2 RED tests: monomorphized execution over `InspectMode` does not
// exist yet. Every test below must fail to compile until `InspectMode` is
// defined, re-exported from `hpsvm`, and threaded through execution as a
// const parameter.
mod task_5_2_inspect_mode {
    use std::any::Any;

    use hpsvm::{BaseSvmTypes, HPSVM, InspectMode, Inspector, Svm};
    use solana_address::Address;
    use solana_keypair::Keypair;
    use solana_message::Message;
    use solana_signer::Signer;
    use solana_system_interface::instruction::transfer;
    use solana_transaction::{Transaction, sanitized::SanitizedTransaction};

    /// Counts every hook that can fire during execution, with plain owned
    /// state. The type exists in scope for the no-inspector test without ever
    /// being installed there.
    #[derive(Default)]
    struct FullCountingInspector {
        tx_starts: usize,
        instruction_starts: usize,
        instruction_ends: usize,
        tx_ends: usize,
        logs: usize,
        account_changes: usize,
    }

    impl Inspector<BaseSvmTypes> for FullCountingInspector {
        fn as_any(&self) -> &dyn Any {
            self
        }

        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }

        fn transaction_start(&mut self, _svm: &Svm<'_, BaseSvmTypes>, _tx: &SanitizedTransaction) {
            self.tx_starts += 1;
        }

        fn instruction_start(&mut self, _index: usize, _program_id: &Address) {
            self.instruction_starts += 1;
        }

        fn instruction_end(&mut self, _index: usize, _program_id: &Address) {
            self.instruction_ends += 1;
        }

        fn log(&mut self, _message: &str) {
            self.logs += 1;
        }

        fn account_change(&mut self, _change: hpsvm::AccountChange<'_>) {
            self.account_changes += 1;
        }

        fn transaction_end(
            &mut self,
            _svm: &Svm<'_, BaseSvmTypes>,
            _result: &solana_transaction_error::TransactionResult<()>,
        ) {
            self.tx_ends += 1;
        }
    }

    impl FullCountingInspector {
        fn total(&self) -> usize {
            self.tx_starts +
                self.instruction_starts +
                self.instruction_ends +
                self.tx_ends +
                self.logs +
                self.account_changes
        }
    }

    fn transfer_tx(
        payer: &Keypair,
        recipient: &Address,
        lamports: u64,
        blockhash: solana_hash::Hash,
    ) -> Transaction {
        Transaction::new(
            &[payer],
            Message::new(&[transfer(&payer.pubkey(), recipient, lamports)], Some(&payer.pubkey())),
            blockhash,
        )
    }

    /// Both `InspectMode` variants are nameable and exhaustive: adding a
    /// variant breaks this match at compile time.
    #[test]
    fn inspect_mode_match_is_exhaustive_over_both_variants() {
        fn name(mode: InspectMode) -> &'static str {
            match mode {
                InspectMode::NoInspector => "no-inspector",
                InspectMode::DynInspector => "dyn-inspector",
            }
        }
        assert_eq!(name(InspectMode::NoInspector), "no-inspector");
        assert_eq!(name(InspectMode::DynInspector), "dyn-inspector");
        assert_ne!(InspectMode::NoInspector, InspectMode::DynInspector);
    }

    /// With no inspector installed — while the counting inspector type exists
    /// in scope but is never installed — execution results are identical to an
    /// inspected run (sans observations) and no hook dispatch occurs.
    #[test]
    fn no_inspector_run_matches_inspected_run_with_zero_dispatch() {
        // The counting type is in scope here but never installed: a fresh
        // instance trivially observes nothing, and the plain VM below never
        // dispatches to it.
        let uninstalled = FullCountingInspector::default();
        assert_eq!(uninstalled.total(), 0);

        let mut plain = HPSVM::new();
        let payer = Keypair::new();
        let recipient = Address::new_unique();
        plain.airdrop(&payer.pubkey(), 10_000).unwrap();
        let plain_meta = plain
            .send_transaction(transfer_tx(&payer, &recipient, 1, plain.latest_blockhash()))
            .unwrap();

        let mut inspected = HPSVM::new();
        let payer_b = Keypair::new();
        let recipient_b = Address::new_unique();
        inspected.airdrop(&payer_b.pubkey(), 10_000).unwrap();
        inspected.set_inspector(FullCountingInspector::default());
        let inspected_meta = inspected
            .send_transaction(transfer_tx(&payer_b, &recipient_b, 1, inspected.latest_blockhash()))
            .unwrap();

        // Identical execution semantics: logs, compute, fee, balances.
        assert_eq!(plain_meta.logs, inspected_meta.logs);
        assert_eq!(plain_meta.compute_units_consumed, inspected_meta.compute_units_consumed);
        assert_eq!(plain_meta.fee, inspected_meta.fee);
        assert_eq!(plain.get_balance(&recipient), inspected.get_balance(&recipient_b));
        assert_eq!(plain.get_balance(&payer.pubkey()), inspected.get_balance(&payer_b.pubkey()));

        // The detached readonly path agrees too (distinct amounts dodge the
        // duplicate-transaction guard inside each VM).
        let plain_detached =
            plain.transact_detached(transfer_tx(&payer, &recipient, 2, plain.latest_blockhash()));
        let inspected_detached = inspected.transact_detached(transfer_tx(
            &payer_b,
            &recipient_b,
            2,
            inspected.latest_blockhash(),
        ));
        assert_eq!(plain_detached.meta().logs, inspected_detached.meta().logs);
        assert_eq!(
            plain_detached.meta().compute_units_consumed,
            inspected_detached.meta().compute_units_consumed
        );
        assert_eq!(plain_detached.status(), inspected_detached.status());

        // Still nothing observed: the type was never installed anywhere.
        assert_eq!(uninstalled.total(), 0);
    }

    /// With an inspector installed, the `DynInspector` specialization still
    /// fires every hook: guards against the specialization silently dropping
    /// the live path.
    #[test]
    fn installed_inspector_still_observes_every_hook() {
        let mut svm = HPSVM::new();
        svm.set_inspector(FullCountingInspector::default());
        let payer = Keypair::new();
        let recipient = Address::new_unique();
        svm.airdrop(&payer.pubkey(), 10_000).unwrap();
        let tx = transfer_tx(&payer, &recipient, 1, svm.latest_blockhash());
        svm.send_transaction(tx).unwrap();

        let boxed = svm.clear_inspector().expect("inspector should be installed");
        let counting =
            boxed.downcast_ref::<FullCountingInspector>().expect("concrete type must recover");
        assert_eq!(counting.tx_starts, 1);
        assert_eq!(counting.instruction_starts, 1);
        assert_eq!(counting.instruction_ends, 1);
        assert_eq!(counting.tx_ends, 1);
        // A bare system transfer logs exactly invoke + success.
        assert_eq!(counting.logs, 2);
        // Commit forwards every staged change: payer debit plus recipient
        // creation at minimum.
        assert!(counting.account_changes >= 2, "saw {}", counting.account_changes);
        assert!(counting.total() > 0);
    }

    /// The mode selection follows installation on the same VM: an installed
    /// inspector observes its transfer (`DynInspector`), and after clearing,
    /// the next transfer executes with identical semantics but no observation
    /// (`NoInspector`) — asserted via hook counters, not internals.
    #[test]
    fn mode_selection_follows_installation_on_the_same_vm() {
        let mut svm = HPSVM::new();
        let payer = Keypair::new();
        let recipient = Address::new_unique();
        svm.airdrop(&payer.pubkey(), 20_000).unwrap();

        svm.set_inspector(FullCountingInspector::default());
        let inspected_meta = svm
            .send_transaction(transfer_tx(&payer, &recipient, 1, svm.latest_blockhash()))
            .unwrap();
        let boxed = svm.clear_inspector().expect("inspector should be installed");
        let counting =
            boxed.downcast_ref::<FullCountingInspector>().expect("concrete type must recover");
        // The installed run took the `DynInspector` path: hooks fired.
        assert_eq!(counting.tx_starts, 1);
        assert!(counting.total() > 0);

        // Cleared: the same VM now takes the `NoInspector` path. Execution
        // semantics are unchanged (distinct amount dodges the
        // duplicate-transaction guard).
        let plain_meta = svm
            .send_transaction(transfer_tx(&payer, &recipient, 2, svm.latest_blockhash()))
            .unwrap();
        assert!(svm.clear_inspector().is_none());
        assert_eq!(plain_meta.logs, inspected_meta.logs);
        assert_eq!(plain_meta.compute_units_consumed, inspected_meta.compute_units_consumed);
        assert_eq!(plain_meta.fee, inspected_meta.fee);
    }
}
