#![cfg(feature = "register-tracing")]

use std::path::PathBuf;

use hpsvm::{HPSVM, register_tracing::TraceMetricsCollector};
use solana_account::Account;
use solana_address::{Address, address};
use solana_instruction::{Instruction, account_meta::AccountMeta};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_transaction::Transaction;

fn read_counter_program() -> Vec<u8> {
    let mut so_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    so_path.push("test_programs/target/deploy/counter.so");
    std::fs::read(so_path).unwrap()
}

fn make_tx(
    program_id: Address,
    counter_address: Address,
    payer_pk: &Address,
    blockhash: solana_hash::Hash,
    payer_kp: &Keypair,
    deduper: u8,
) -> Transaction {
    let msg = Message::new_with_blockhash(
        &[Instruction {
            program_id,
            accounts: vec![AccountMeta::new(counter_address, false)],
            data: vec![0, deduper],
        }],
        Some(payer_pk),
        &blockhash,
    );
    Transaction::new(&[payer_kp], msg, blockhash)
}

#[test]
fn trace_metrics_collector_records_counter_program_execution() {
    let mut svm = HPSVM::new_debuggable(true);
    let collector = TraceMetricsCollector::default();
    svm.set_invocation_inspect_callback(collector.clone());

    let payer_kp = Keypair::new();
    let payer_pk = payer_kp.pubkey();
    let program_id = address!("GtdambwDgHWrDJdVPBkEHGhCwokqgAoch162teUjJse2");
    let counter_address = address!("J39wvrFY2AkoAUCke5347RMNk3ditxZfVidoZ7U6Fguf");

    svm.add_program(program_id, &read_counter_program()).unwrap();
    svm.airdrop(&payer_pk, 1_000_000_000).unwrap();
    svm.set_account(
        counter_address,
        Account {
            lamports: 5,
            data: vec![0_u8; std::mem::size_of::<u32>()],
            owner: program_id,
            ..Default::default()
        },
    )
    .unwrap();

    let tx = make_tx(program_id, counter_address, &payer_pk, svm.latest_blockhash(), &payer_kp, 0);
    svm.send_transaction(tx).unwrap();

    let metrics = collector.snapshot();
    let counter_metrics = metrics
        .iter()
        .find(|metrics| metrics.program_id == program_id)
        .expect("counter program metrics should be collected");

    assert_eq!(counter_metrics.invocations, 1);
    assert!(counter_metrics.total_register_frames > 0);
    assert!(counter_metrics.max_register_frames > 0);
    assert_eq!(counter_metrics.max_stack_height, 1);
    assert_eq!(counter_metrics.max_instruction_accounts, 1);
}

#[test]
fn trace_metrics_collector_accumulates_repeated_counter_invocations() {
    let mut svm = HPSVM::new_debuggable(true);
    let collector = TraceMetricsCollector::default();
    svm.set_invocation_inspect_callback(collector.clone());

    let payer_kp = Keypair::new();
    let payer_pk = payer_kp.pubkey();
    let program_id = address!("GtdambwDgHWrDJdVPBkEHGhCwokqgAoch162teUjJse2");
    let counter_address = address!("J39wvrFY2AkoAUCke5347RMNk3ditxZfVidoZ7U6Fguf");

    svm.add_program(program_id, &read_counter_program()).unwrap();
    svm.airdrop(&payer_pk, 1_000_000_000).unwrap();
    svm.set_account(
        counter_address,
        Account {
            lamports: 5,
            data: vec![0_u8; std::mem::size_of::<u32>()],
            owner: program_id,
            ..Default::default()
        },
    )
    .unwrap();

    for deduper in 0..3 {
        let tx = make_tx(
            program_id,
            counter_address,
            &payer_pk,
            svm.latest_blockhash(),
            &payer_kp,
            deduper,
        );
        svm.send_transaction(tx).unwrap();
        svm.expire_blockhash();
    }

    let metrics = collector.snapshot();
    let counter_metrics = metrics
        .iter()
        .find(|metrics| metrics.program_id == program_id)
        .expect("counter program metrics should be collected");

    assert_eq!(counter_metrics.invocations, 3);
    assert_eq!(counter_metrics.cpi_invocations, 0);
    assert!(counter_metrics.average_register_frames() > 0.0);
    assert_eq!(counter_metrics.average_instruction_accounts(), 1.0);
}
