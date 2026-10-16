//! Task 4.3: `prewarm` driven by the batch read-set.
//!
//! A batch stage must perform a single batched source load instead of
//! per-transaction loads, while producing results identical to serial
//! execution.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use hpsvm::{AccountSource, AccountSourceError, HPSVM, batch::TransactionBatchError};
use solana_account::AccountSharedData;
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_sdk_ids::system_program;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::{Transaction, versioned::VersionedTransaction};

/// Map-backed source counting batched vs single loads separately.
///
/// `get_accounts` serves from the map directly (one batched round-trip) and
/// does NOT delegate to `get_account`, so the two counters distinguish a
/// single batched load from per-key fall-through reads.
#[derive(Clone, Default)]
struct CountingMapSource {
    accounts: Arc<HashMap<Address, AccountSharedData>>,
    batch_calls: Arc<AtomicUsize>,
    single_calls: Arc<AtomicUsize>,
}

impl CountingMapSource {
    fn with_accounts(accounts: HashMap<Address, AccountSharedData>) -> Self {
        Self { accounts: Arc::new(accounts), ..Self::default() }
    }

    fn batch_calls(&self) -> usize {
        self.batch_calls.load(Ordering::SeqCst)
    }

    fn single_calls(&self) -> usize {
        self.single_calls.load(Ordering::SeqCst)
    }
}

impl AccountSource for CountingMapSource {
    fn get_account(
        &self,
        pubkey: &Address,
    ) -> Result<Option<AccountSharedData>, AccountSourceError> {
        self.single_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.accounts.get(pubkey).cloned())
    }

    fn get_accounts(
        &self,
        pubkeys: &[Address],
    ) -> Result<Vec<Option<AccountSharedData>>, AccountSourceError> {
        self.batch_calls.fetch_add(1, Ordering::SeqCst);
        Ok(pubkeys.iter().map(|key| self.accounts.get(key).cloned()).collect())
    }
}

/// Source whose batched path always fails while single reads succeed.
///
/// Planning sanitizes legacy transactions without source reads, so a batch
/// over source-backed accounts plans fine and then exercises the prewarm
/// error mapping.
#[derive(Clone)]
struct BatchFailingSource {
    inner: CountingMapSource,
}

impl AccountSource for BatchFailingSource {
    fn get_account(
        &self,
        pubkey: &Address,
    ) -> Result<Option<AccountSharedData>, AccountSourceError> {
        self.inner.get_account(pubkey)
    }

    fn get_accounts(
        &self,
        _pubkeys: &[Address],
    ) -> Result<Vec<Option<AccountSharedData>>, AccountSourceError> {
        Err(AccountSourceError::new("batch endpoint unavailable"))
    }
}

fn funded_system_account(lamports: u64) -> AccountSharedData {
    AccountSharedData::new(lamports, 0, &system_program::id())
}

fn transfer_versioned(
    payer: &Keypair,
    recipient: &Address,
    lamports: u64,
    svm: &HPSVM,
) -> VersionedTransaction {
    VersionedTransaction::from(Transaction::new(
        &[payer],
        Message::new(&[transfer(&payer.pubkey(), recipient, lamports)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    ))
}

#[test]
fn batch_stage_performs_a_single_batched_source_load_with_identical_results() {
    let payer_a = Keypair::new();
    let payer_b = Keypair::new();
    let recipient_a = Address::new_unique();
    let recipient_b = Address::new_unique();
    let funded = HashMap::from([
        (payer_a.pubkey(), funded_system_account(1_000_000_000)),
        (payer_b.pubkey(), funded_system_account(1_000_000_000)),
        (recipient_a, funded_system_account(1_000)),
        (recipient_b, funded_system_account(1_000)),
    ]);

    let batch_source = CountingMapSource::with_accounts(funded.clone());
    let serial_source = CountingMapSource::with_accounts(funded);
    let mut batch_vm = HPSVM::builder()
        .with_program_test_defaults()
        .with_account_source(batch_source.clone())
        .build()
        .unwrap();
    let mut serial_vm = HPSVM::builder()
        .with_program_test_defaults()
        .with_account_source(serial_source)
        .build()
        .unwrap();

    // Distinct payers/recipients are conflict-free: both land in one stage.
    let batch_txs = vec![
        transfer_versioned(&payer_a, &recipient_a, 10, &batch_vm),
        transfer_versioned(&payer_b, &recipient_b, 20, &batch_vm),
    ];
    let batch_outcome = batch_vm.send_transaction_batch(batch_txs).unwrap();

    assert_eq!(batch_outcome.plan.stages.len(), 1);
    assert_eq!(batch_outcome.plan.stages[0].transaction_indexes, vec![0, 1]);
    assert!(batch_outcome.results.iter().all(Result::is_ok));

    // Exactly one batched source round-trip for the stage, and no per-key
    // fall-through reads: every stage key was served from the warmed overlay.
    assert_eq!(batch_source.batch_calls(), 1);
    assert_eq!(batch_source.single_calls(), 0);

    let serial_txs = vec![
        transfer_versioned(&payer_a, &recipient_a, 10, &serial_vm),
        transfer_versioned(&payer_b, &recipient_b, 20, &serial_vm),
    ];
    let mut serial_results = Vec::new();
    for tx in serial_txs {
        serial_results.push(serial_vm.send_transaction(tx));
    }

    assert!(serial_results.iter().all(Result::is_ok));
    assert_eq!(batch_outcome.results, serial_results);
    for (vm, name) in [(&batch_vm, "batch"), (&serial_vm, "serial")] {
        assert_eq!(vm.get_balance(&recipient_a), Some(1_010), "{name} recipient_a");
        assert_eq!(vm.get_balance(&recipient_b), Some(1_020), "{name} recipient_b");
        assert_eq!(
            vm.get_balance(&payer_a.pubkey()),
            serial_vm.get_balance(&payer_a.pubkey()),
            "{name} payer_a matches serial"
        );
    }
}

#[test]
fn batch_stage_maps_a_prewarm_source_failure_to_the_stage_head() {
    let payer_a = Keypair::new();
    let payer_b = Keypair::new();
    let recipient_a = Address::new_unique();
    let recipient_b = Address::new_unique();
    let source = BatchFailingSource {
        inner: CountingMapSource::with_accounts(HashMap::from([
            (payer_a.pubkey(), funded_system_account(1_000_000_000)),
            (payer_b.pubkey(), funded_system_account(1_000_000_000)),
            (recipient_a, funded_system_account(1_000)),
            (recipient_b, funded_system_account(1_000)),
        ])),
    };
    let mut svm =
        HPSVM::builder().with_program_test_defaults().with_account_source(source).build().unwrap();

    let txs = vec![
        transfer_versioned(&payer_a, &recipient_a, 10, &svm),
        transfer_versioned(&payer_b, &recipient_b, 20, &svm),
    ];
    let error = svm.send_transaction_batch(txs).expect_err("prewarm must propagate source errors");

    // The batched load cannot name the failing key, so the error is
    // attributed to the stage head: first transaction, read-set head.
    match error {
        TransactionBatchError::AccountSource { index, pubkey, source } => {
            assert_eq!(index, 0);
            assert_eq!(pubkey, payer_a.pubkey());
            assert_eq!(source.message(), "batch endpoint unavailable");
        }
        other => panic!("expected TransactionBatchError::AccountSource, got {other:?}"),
    }
}
