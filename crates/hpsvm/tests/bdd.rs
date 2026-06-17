use std::{fmt, fmt::Debug, path::PathBuf};

use agave_feature_set::{FeatureSet, raise_cpi_nesting_limit_to_8};
use cucumber::{World as _, given, then, when};
use hpsvm::{
    HPSVM,
    types::{ExecutionOutcome, TransactionMetadata},
};
use solana_account::Account;
use solana_address::{Address, address};
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_instruction::{Instruction, error::InstructionError};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_sdk_ids::{bpf_loader, bpf_loader_upgradeable, system_program};
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::Transaction;
use solana_transaction_error::TransactionError;

#[derive(Default, cucumber::World)]
struct FeatureSetWorld {
    svm: Option<HPSVM>,
    sender_kp: Option<Keypair>,
    sender: Option<Address>,
    recipient: Option<Address>,
    last_meta: Option<TransactionMetadata>,
    last_outcome: Option<ExecutionOutcome>,
    pending_tx: Option<Transaction>,
}

impl Debug for FeatureSetWorld {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FeatureSetWorld").field("has_svm", &self.svm.is_some()).finish()
    }
}

#[given("a default HPSVM instance with all features materialized")]
fn default_hpsvm(world: &mut FeatureSetWorld) {
    let svm = HPSVM::new();
    let token_program_id = address!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

    assert_eq!(
        svm.get_account(&token_program_id).expect("token program should exist").owner,
        bpf_loader_upgradeable::id()
    );
    assert!(svm.get_account(&raise_cpi_nesting_limit_to_8::id()).is_some());

    world.svm = Some(svm);
}

#[when("the VM feature set is replaced with the default disabled feature set")]
fn replace_feature_set(world: &mut FeatureSetWorld) {
    let mut svm = world.svm.take().expect("world should contain an HPSVM instance");
    svm.set_feature_set(FeatureSet::default()).expect("feature-set reconfiguration should succeed");
    world.svm = Some(svm);
}

#[then("the old active feature account should be removed")]
fn feature_account_removed(world: &mut FeatureSetWorld) {
    let svm = world.svm.as_ref().expect("world should contain an HPSVM instance");
    assert!(svm.get_account(&raise_cpi_nesting_limit_to_8::id()).is_none());
}

#[then("the SPL token program should use the legacy loader")]
fn token_program_uses_legacy_loader(world: &mut FeatureSetWorld) {
    let svm = world.svm.as_ref().expect("world should contain an HPSVM instance");
    let token_program_id = address!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

    assert_eq!(
        svm.get_account(&token_program_id).expect("token program should exist").owner,
        bpf_loader::id()
    );
}

#[when("a direct system transfer instruction is processed")]
fn process_system_transfer_instruction(world: &mut FeatureSetWorld) {
    let mut svm = world.svm.take().expect("world should contain an HPSVM instance");
    let sender = Address::new_unique();
    let recipient = Address::new_unique();
    svm.set_account(sender, Account::new(10_000, 0, &system_program::id()))
        .expect("sender account should be inserted");

    let meta = svm
        .process_instruction(transfer(&sender, &recipient, 64))
        .expect("instruction should succeed");

    world.recipient = Some(recipient);
    world.last_meta = Some(meta);
    world.svm = Some(svm);
}

#[then("the recipient account should be committed")]
fn recipient_account_committed(world: &mut FeatureSetWorld) {
    let svm = world.svm.as_ref().expect("world should contain an HPSVM instance");
    let recipient = world.recipient.expect("scenario should record recipient");

    assert_eq!(svm.get_balance(&recipient), Some(64));
}

#[then("the instruction metadata should include account diagnostics")]
fn instruction_metadata_has_account_diagnostics(world: &mut FeatureSetWorld) {
    let meta = world.last_meta.as_ref().expect("scenario should record metadata");
    let recipient = world.recipient.expect("scenario should record recipient");

    assert!(
        meta.diagnostics
            .account_diffs
            .iter()
            .any(|diff| diff.address == recipient && diff.pre.is_none())
    );
    assert!(!meta.diagnostics.execution_trace.instructions.is_empty());
}

#[given("a funded sender account")]
fn funded_sender_account(world: &mut FeatureSetWorld) {
    let svm = world.svm.as_mut().expect("world should contain an HPSVM instance");
    let kp = Keypair::new();
    let sender = kp.pubkey();
    svm.set_account(sender, Account::new(10_000, 0, &system_program::id()))
        .expect("sender account should be inserted");
    world.sender_kp = Some(kp);
    world.sender = Some(sender);
}

#[when("a successful system transfer is executed via transact")]
fn system_transfer_via_transact(world: &mut FeatureSetWorld) {
    let svm = world.svm.take().expect("world should contain an HPSVM instance");
    let sender = world.sender.expect("scenario should record sender");
    let kp = world.sender_kp.as_ref().expect("scenario should record keypair");
    let recipient = Address::new_unique();

    let tx = Transaction::new(
        &[kp],
        Message::new(&[transfer(&sender, &recipient, 64)], Some(&sender)),
        svm.latest_blockhash(),
    );
    let outcome = svm.transact(tx);
    world.recipient = Some(recipient);
    world.last_outcome = Some(outcome);
    world.svm = Some(svm);
}

#[then("the ExecutionOutcome should contain the fee payer address")]
fn outcome_has_fee_payer(world: &mut FeatureSetWorld) {
    let outcome = world.last_outcome.as_ref().expect("scenario should record outcome");
    let sender = world.sender.expect("scenario should record sender");
    assert!(outcome.fee_payer().is_some(), "fee_payer should be Some on success");
    assert_eq!(outcome.fee_payer(), Some(sender));
}

#[then("the fee payer should match the transaction fee payer")]
fn fee_payer_matches(world: &mut FeatureSetWorld) {
    let outcome = world.last_outcome.as_ref().expect("scenario should record outcome");
    let sender = world.sender.expect("scenario should record sender");
    assert_eq!(outcome.fee_payer(), Some(sender));
}

#[given("a default HPSVM instance")]
fn default_hpsvm_simple(world: &mut FeatureSetWorld) {
    world.svm = Some(HPSVM::new());
}

#[given("a transaction that exceeds the compute budget")]
fn transaction_exceeds_compute_budget(world: &mut FeatureSetWorld) {
    let sender_kp = Keypair::new();
    let sender = sender_kp.pubkey();
    let recipient = Address::new_unique();

    let mut svm_mut = HPSVM::new();
    svm_mut
        .set_account(sender, Account::new(10_000, 0, &system_program::id()))
        .expect("sender account should be inserted");

    let tx = Transaction::new(
        &[&sender_kp],
        Message::new(
            &[
                ComputeBudgetInstruction::set_compute_unit_limit(1),
                transfer(&sender, &recipient, 64),
            ],
            Some(&sender),
        ),
        svm_mut.latest_blockhash(),
    );

    world.pending_tx = Some(tx);
    world.svm = Some(svm_mut);
}

#[when("the transaction is executed")]
fn execute_transaction(world: &mut FeatureSetWorld) {
    let svm = world.svm.as_ref().expect("world should contain an HPSVM instance");
    let tx = world.pending_tx.take().expect("world should contain a pending transaction");
    let outcome = svm.transact(tx);
    world.last_outcome = Some(outcome);
}

#[then("the result should be an error")]
fn result_should_be_error(world: &mut FeatureSetWorld) {
    let outcome = world.last_outcome.as_ref().expect("scenario should record outcome");
    assert!(outcome.status().is_err(), "expected transaction to fail");
}

#[then("the error should indicate compute budget exceeded")]
fn error_indicates_compute_budget_exceeded(world: &mut FeatureSetWorld) {
    let outcome = world.last_outcome.as_ref().expect("scenario should record outcome");
    let err = outcome.status().as_ref().unwrap_err();
    assert_eq!(
        *err,
        TransactionError::InstructionError(0, InstructionError::ComputationalBudgetExceeded),
        "expected ComputationalBudgetExceeded, got {:?}",
        err
    );
}

#[given("a sender account with zero lamports")]
fn zero_lamport_sender_account(world: &mut FeatureSetWorld) {
    let svm = world.svm.as_mut().expect("world should contain an HPSVM instance");
    let kp = Keypair::new();
    let sender = kp.pubkey();
    svm.set_account(sender, Account::new(1, 0, &system_program::id()))
        .expect("sender account should be inserted");
    world.sender_kp = Some(kp);
    world.sender = Some(sender);
}

#[when("a transfer instruction is executed")]
fn execute_transfer_instruction(world: &mut FeatureSetWorld) {
    let svm = world.svm.take().expect("world should contain an HPSVM instance");
    let sender = world.sender.expect("scenario should record sender");
    let kp = world.sender_kp.as_ref().expect("scenario should record keypair");
    let recipient = Address::new_unique();

    let tx = Transaction::new(
        &[kp],
        Message::new(&[transfer(&sender, &recipient, 100)], Some(&sender)),
        svm.latest_blockhash(),
    );
    let outcome = svm.transact(tx);
    world.recipient = Some(recipient);
    world.last_outcome = Some(outcome);
    world.svm = Some(svm);
}

#[then("the error should indicate insufficient funds")]
fn error_indicates_insufficient_funds(world: &mut FeatureSetWorld) {
    let outcome = world.last_outcome.as_ref().expect("scenario should record outcome");
    let err = outcome.status().as_ref().unwrap_err();
    let err_str = format!("{:?}", err);
    assert!(
        err_str.contains("InsufficientFunds") ||
            err_str.contains("insufficient") ||
            err_str.contains("NotEnough") ||
            err_str.contains("Insufficient") ||
            err_str.contains("Not enough"),
        "expected insufficient funds error, got {:?}",
        err
    );
}

#[given("a transaction targeting a non-existent program")]
fn transaction_with_nonexistent_program(world: &mut FeatureSetWorld) {
    let sender_kp = Keypair::new();
    let sender = sender_kp.pubkey();
    let fake_program = Address::new_unique();

    let mut svm_mut = HPSVM::new();
    svm_mut
        .set_account(sender, Account::new(10_000, 0, &system_program::id()))
        .expect("sender account should be inserted");

    let ix = Instruction { program_id: fake_program, accounts: vec![], data: vec![] };

    let tx = Transaction::new(
        &[&sender_kp],
        Message::new(&[ix], Some(&sender)),
        svm_mut.latest_blockhash(),
    );

    world.pending_tx = Some(tx);
    world.svm = Some(svm_mut);
}

#[then("the error should indicate invalid program for instruction")]
fn error_indicates_invalid_program(world: &mut FeatureSetWorld) {
    let outcome = world.last_outcome.as_ref().expect("scenario should record outcome");
    let err = outcome.status().as_ref().unwrap_err();
    assert_eq!(
        *err,
        TransactionError::InvalidProgramForExecution,
        "expected InvalidProgramForExecution, got {:?}",
        err
    );
}

#[given("a transaction with an expired blockhash")]
fn transaction_with_expired_blockhash(world: &mut FeatureSetWorld) {
    let mut svm = world.svm.take().expect("world should contain an HPSVM instance");
    let kp = Keypair::new();
    let sender = kp.pubkey();
    let recipient = Address::new_unique();

    svm.set_account(sender, Account::new(10_000, 0, &system_program::id()))
        .expect("sender account should be inserted");

    let tx = Transaction::new(
        &[&kp],
        Message::new(&[transfer(&sender, &recipient, 64)], Some(&sender)),
        svm.latest_blockhash(),
    );

    svm.expire_blockhash();

    world.pending_tx = Some(tx);
    world.svm = Some(svm);
}

#[then("the error should indicate blockhash not found")]
fn error_indicates_blockhash_not_found(world: &mut FeatureSetWorld) {
    let outcome = world.last_outcome.as_ref().expect("scenario should record outcome");
    let err = outcome.status().as_ref().unwrap_err();
    assert_eq!(
        *err,
        TransactionError::BlockhashNotFound,
        "expected BlockhashNotFound, got {:?}",
        err
    );
}

#[tokio::test]
async fn bdd() {
    let features_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("features");

    FeatureSetWorld::cucumber().run(features_dir).await;
}
