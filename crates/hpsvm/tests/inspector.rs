use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use hpsvm::{HPSVM, Inspector, TransactionOrigin};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::Transaction;

#[derive(Default)]
struct CountingInspector {
    top_level_instructions: Arc<AtomicUsize>,
}

impl Inspector for CountingInspector {
    fn on_instruction(&self, _svm: &HPSVM, _index: usize, _program_id: &Address) {
        self.top_level_instructions.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct OriginInspector {
    user_instructions: Arc<AtomicUsize>,
    internal_airdrops: Arc<AtomicUsize>,
}

impl Inspector for OriginInspector {
    fn on_instruction_with_origin(
        &self,
        origin: TransactionOrigin,
        _svm: &HPSVM,
        _index: usize,
        _program_id: &Address,
    ) {
        match origin {
            TransactionOrigin::User => {
                self.user_instructions.fetch_add(1, Ordering::SeqCst);
            }
            TransactionOrigin::InternalAirdrop => {
                self.internal_airdrops.fetch_add(1, Ordering::SeqCst);
            }
            TransactionOrigin::Batch { .. } => {}
            _ => {}
        }
    }
}

#[test]
fn inspector_observes_top_level_instructions() {
    let inspector = CountingInspector::default();
    let observed = Arc::clone(&inspector.top_level_instructions);
    let mut svm = HPSVM::new().with_inspector(inspector);
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 1)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );

    svm.send_transaction(tx).unwrap();

    assert_eq!(observed.load(Ordering::SeqCst), 1);
}

#[test]
fn inspector_can_distinguish_internal_airdrops_from_user_transactions() {
    let inspector = OriginInspector::default();
    let user_instructions = Arc::clone(&inspector.user_instructions);
    let internal_airdrops = Arc::clone(&inspector.internal_airdrops);
    let mut svm = HPSVM::new().with_inspector(inspector);
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 1)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );

    svm.send_transaction(tx).unwrap();

    assert_eq!(internal_airdrops.load(Ordering::SeqCst), 1);
    assert_eq!(user_instructions.load(Ordering::SeqCst), 1);
}
