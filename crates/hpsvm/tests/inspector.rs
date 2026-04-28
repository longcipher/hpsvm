use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use hpsvm::{HPSVM, Inspector};
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
