use hpsvm::HPSVM;
use solana_account::Account;
use solana_address::Address;
use solana_sdk_ids::system_program;
use solana_system_interface::instruction::transfer;

#[test]
fn process_instruction_commits_state_and_reports_diagnostics() {
    let mut svm = HPSVM::new();
    let sender = Address::new_unique();
    let recipient = Address::new_unique();

    svm.set_account(sender, Account::new(10_000, 0, &system_program::id()))
        .expect("sender account should be inserted");

    let meta = svm
        .process_instruction(transfer(&sender, &recipient, 64))
        .expect("single instruction should execute");

    assert_eq!(svm.get_balance(&recipient), Some(64));

    let recipient_diff = meta
        .diagnostics
        .account_diffs
        .iter()
        .find(|diff| diff.address == recipient)
        .expect("recipient account diff should be recorded");
    assert!(recipient_diff.pre.is_none());
    assert_eq!(recipient_diff.post.as_ref().map(|account| account.lamports), Some(64));

    assert_eq!(meta.diagnostics.pre_balances.len(), meta.diagnostics.post_balances.len());
    assert_eq!(meta.diagnostics.execution_trace.instructions[0].program_id, system_program::id());
}

#[test]
fn simulate_instruction_reports_diagnostics_without_committing() {
    let mut svm = HPSVM::new();
    let sender = Address::new_unique();
    let recipient = Address::new_unique();

    svm.set_account(sender, Account::new(10_000, 0, &system_program::id()))
        .expect("sender account should be inserted");

    let simulation = svm
        .simulate_instruction(transfer(&sender, &recipient, 64))
        .expect("single instruction should simulate");

    assert_eq!(svm.get_balance(&recipient), None);
    assert!(simulation.meta.diagnostics.account_diffs.iter().any(|diff| diff.address == recipient));
}
