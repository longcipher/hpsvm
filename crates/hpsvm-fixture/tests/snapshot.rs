#![allow(missing_docs)]

use hpsvm::{
    HPSVM,
    types::{SimulatedTransactionInfo, TransactionMetadata},
};
use hpsvm_fixture::{ExecutionSnapshot, ExecutionStatus};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::{
    Message, compiled_instruction::CompiledInstruction, inner_instruction::InnerInstruction,
};
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::Transaction;

#[test]
fn snapshot_from_outcome_captures_post_accounts_and_metadata() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );

    let outcome = svm.transact(tx);
    let snapshot = ExecutionSnapshot::from_outcome(&outcome);

    assert!(matches!(snapshot.status, ExecutionStatus::Success));
    assert_eq!(snapshot.compute_units_consumed, outcome.meta().compute_units_consumed);
    assert_eq!(snapshot.fee, outcome.meta().fee);
    assert_eq!(snapshot.logs, outcome.meta().logs);
    assert!(snapshot.return_data.is_none());
    assert!(snapshot.inner_instructions.is_empty());
    assert!(
        snapshot
            .post_accounts
            .iter()
            .any(|account| account.address == recipient && account.lamports == 64)
    );
}

#[test]
fn snapshot_from_simulation_preserves_outer_instruction_grouping() {
    let repeated_inner_instruction = InnerInstruction {
        instruction: CompiledInstruction::new_from_raw_parts(3, vec![7, 8, 9], vec![1, 2]),
        stack_height: 2,
    };
    let simulation = SimulatedTransactionInfo {
        meta: TransactionMetadata {
            inner_instructions: vec![
                vec![repeated_inner_instruction.clone()],
                vec![repeated_inner_instruction],
            ],
            ..TransactionMetadata::default()
        },
        post_accounts: Vec::new(),
    };

    let snapshot = ExecutionSnapshot::from_simulation(&simulation);

    assert_eq!(snapshot.inner_instructions.len(), 2);
    assert_eq!(snapshot.inner_instructions[0].outer_instruction_index, 0);
    assert_eq!(snapshot.inner_instructions[1].outer_instruction_index, 1);
    assert_ne!(snapshot.inner_instructions[0], snapshot.inner_instructions[1]);
}
