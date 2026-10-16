//! Equivalence suite for the reusable inspectors that `hpsvm-inspectors`
//! provides.
//!
//! Every expectation here is a copy of the value pinned in
//! `crates/hpsvm/tests/inspector.rs` against the core-crate implementations
//! that were moved into this crate, so a behavioural drift in any moved
//! inspector fails this file rather than silently changing the observable event
//! stream. Each inspector is exercised with a real [`HPSVM`]: a successful user
//! transfer, a failing transfer, an instruction-less transaction, a single
//! transaction stage and a parallel stage.

use std::sync::atomic::Ordering;

use hpsvm::{BaseSvmTypes, HPSVM};
use hpsvm_inspectors::{
    CountingInspector, Inspector, NoopInspector, OriginCountingInspector, OriginFilter,
    OriginFiltered, OriginRecordingInspector, RecordingInspector, TransactionOrigin,
};
use solana_address::Address;
use solana_hash::Hash;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::Transaction;

/// Base58 of the system program: the only program a bare transfer invokes.
const SYSTEM_PROGRAM: &str = "11111111111111111111111111111111";

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

/// Compile-time proof that a type is installable through
/// [`HPSVM::with_inspector`].
fn assert_installable<I: Inspector<BaseSvmTypes> + Send + Sync + 'static>() {}

#[test]
fn every_shipped_inspector_is_installable() {
    assert_installable::<NoopInspector>();
    assert_installable::<RecordingInspector>();
    assert_installable::<OriginRecordingInspector>();
    assert_installable::<CountingInspector>();
    assert_installable::<OriginCountingInspector>();
}

#[test]
fn a_user_transfer_records_the_exact_origin_less_event_stream() {
    let (inspector, log) = RecordingInspector::subscribe();
    let mut svm = HPSVM::new().with_inspector(inspector);
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    // The airdrop that funds the payer is an internal transfer, so its
    // callbacks must not reach the hooks.
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

#[test]
fn counting_inspector_observes_one_top_level_instruction_per_transfer() {
    let inspector = CountingInspector::default();
    let observed = inspector.counter();
    let mut svm = HPSVM::new().with_inspector(inspector);
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = transfer_tx(&payer, &recipient, 1, svm.latest_blockhash());
    svm.send_transaction(tx).unwrap();

    assert_eq!(observed.load(Ordering::SeqCst), 1);
}

/// `CountingInspector` only overrides the instruction hook, which fires for
/// user traffic only.
#[test]
fn origin_counting_inspector_separates_internal_airdrops_from_user_transactions() {
    let inspector = OriginCountingInspector::default();
    let user_instructions = inspector.user_instructions();
    let internal_airdrops = inspector.internal_airdrops();
    let mut svm = HPSVM::new().with_inspector(inspector);
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = transfer_tx(&payer, &recipient, 1, svm.latest_blockhash());
    svm.send_transaction(tx).unwrap();

    assert_eq!(internal_airdrops.load(Ordering::SeqCst), 1);
    assert_eq!(user_instructions.load(Ordering::SeqCst), 1);
}

/// Batch stages are non-user origins, so the origin observer reports the
/// stage and transaction indexes the planner assigned. Single-transaction
/// stages run serially on the live VM, which is why their origins arrive in
/// plan order.
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
            // inspector accepts every origin so it sees it.
            TransactionOrigin::InternalAirdrop,
            TransactionOrigin::Batch { stage_index: 0, transaction_index: 0 },
            TransactionOrigin::Batch { stage_index: 1, transaction_index: 1 },
            TransactionOrigin::Batch { stage_index: 2, transaction_index: 2 },
        ]
    );
}

/// A stage with more than one transaction runs on worker VMs, which carry no
/// inspector: the boxed inspector cannot be cloned onto rayon threads. Only
/// the funding airdrops (serial, on the live VM) are therefore observed.
#[test]
fn a_parallel_batch_stage_observes_only_the_serial_airdrops() {
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
    // are never observed. Both funding airdrops precede the batch and are.
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
    let observed = inspector.counter();
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

/// Funds `payer` and transfers one lamport to `recipient`, returning the
/// committed transaction metadata.
fn funded_transfer(
    svm: &mut HPSVM,
    payer: &Keypair,
    recipient: &Address,
) -> hpsvm::types::TransactionResult {
    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = transfer_tx(payer, recipient, 1, svm.latest_blockhash());
    svm.send_transaction(tx)
}

/// `NoopInspector` accepts every callback and records nothing. Installing it
/// must leave the VM observably identical to an uninstrumented VM — same
/// metadata, same balances.
#[test]
fn noop_inspector_is_observationally_transparent() {
    let payer = Keypair::new();
    let recipient = Address::new_unique();
    let mut instrumented = HPSVM::new().with_inspector(NoopInspector);
    let mut uninstrumented = HPSVM::new();

    let instrumented_result = funded_transfer(&mut instrumented, &payer, &recipient);
    let uninstrumented_result = funded_transfer(&mut uninstrumented, &payer, &recipient);

    assert!(
        instrumented_result.is_ok(),
        "the instrumented transfer should succeed: {instrumented_result:?}"
    );
    assert_eq!(instrumented_result, uninstrumented_result);
    assert_eq!(instrumented.get_balance(&recipient), uninstrumented.get_balance(&recipient));
    assert_eq!(instrumented.get_balance(&recipient), Some(1));
    assert_eq!(
        instrumented.get_balance(&payer.pubkey()),
        uninstrumented.get_balance(&payer.pubkey())
    );
}

/// The re-exported trait, origin, filter and decorator must be the very items
/// the VM hands to the hooks, not look-alikes defined in this crate.
#[test]
fn the_reexported_trait_and_origin_are_the_core_crate_items() {
    fn same_inspector<T: Inspector<BaseSvmTypes>>() {}
    fn same_origin(_: TransactionOrigin) {}

    same_inspector::<NoopInspector>();
    same_origin(TransactionOrigin::Batch { stage_index: 2, transaction_index: 5 });

    let decorator = OriginFiltered::new(NoopInspector, OriginFilter::All);
    assert_eq!(decorator.filter(), OriginFilter::All);
    assert!(Inspector::<BaseSvmTypes>::accepts_origin(&decorator, TransactionOrigin::User));
    assert!(!Inspector::<BaseSvmTypes>::accepts_origin(
        &OriginFiltered::new(NoopInspector, OriginFilter::UserOnly),
        TransactionOrigin::InternalAirdrop
    ));
}
