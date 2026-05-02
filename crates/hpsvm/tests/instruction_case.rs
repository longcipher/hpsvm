use hpsvm::{HPSVM, instruction::InstructionCase};
use solana_account::{Account, ReadableAccount};
use solana_address::Address;
use solana_system_interface::instruction::transfer;

#[test]
fn process_instruction_case_executes_without_mutating_original_vm() {
    let svm = HPSVM::new();
    let sender = Address::new_unique();
    let recipient = Address::new_unique();
    let transfer_ix = transfer(&sender, &recipient, 64);
    let case = InstructionCase {
        program_id: transfer_ix.program_id,
        accounts: transfer_ix.accounts,
        data: transfer_ix.data,
        pre_accounts: vec![
            (
                sender,
                Account {
                    lamports: 10_000,
                    owner: solana_sdk_ids::system_program::id(),
                    ..Default::default()
                },
            ),
            (
                recipient,
                Account {
                    lamports: 1,
                    owner: solana_sdk_ids::system_program::id(),
                    ..Default::default()
                },
            ),
        ],
    };

    let outcome = svm
        .process_instruction_case(&case)
        .expect("instruction case execution should build a transaction");

    assert!(outcome.status().is_ok());
    assert!(svm.get_account(&sender).is_none());
    assert!(svm.get_account(&recipient).is_none());

    let post_accounts = outcome.post_accounts();
    let recipient_account = post_accounts
        .iter()
        .find(|(address, _)| *address == recipient)
        .expect("recipient account should be present after execution");
    assert_eq!(recipient_account.1.lamports(), 65);

    let sender_account = post_accounts
        .iter()
        .find(|(address, _)| *address == sender)
        .expect("sender account should be present after execution");
    assert!(sender_account.1.lamports() < 10_000 - 64);
}
