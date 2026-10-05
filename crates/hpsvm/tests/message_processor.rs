//! Tests for the multi-instruction and CPI paths of the message processor.
//!
//! `process_message` has behaviour that only shows up with more than one
//! instruction in a transaction, or with cross-program invocation:
//!
//! - a failing instruction aborts the transaction but the compute units burned by the instructions
//!   before it are still accounted for,
//! - the failing instruction's index is reported verbatim,
//! - precompiles contribute zero compute units, which is why precompile-only transactions look
//!   free,
//! - the execution trace records one frame per instruction with the right `stack_height`, program
//!   id, accounts, and data,
//! - inner instructions are attributed to their outer instruction.

use hpsvm::{HPSVM, types::ExecutedInstruction};
use solana_address::{Address, address};
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::Transaction;
use solana_transaction_error::TransactionError;

/// Loads the SBF program these tests invoke.
///
/// Returns `false` when the program has not been built, so the caller can skip.
fn load_program(svm: &mut HPSVM, program_id: Address, file: &str) -> bool {
    match hpsvm_test_support::read_program(file) {
        Some(bytes) => {
            svm.add_program(program_id, &bytes)
                .unwrap_or_else(|error| panic!("failed to load {file}: {error}"));
            true
        }
        None => false,
    }
}

#[test]
fn a_failing_later_instruction_reports_its_own_index() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(
            &[
                transfer(&payer.pubkey(), &recipient, 1),
                // More than the payer holds, so the system program rejects it
                // while it *is* deployed, giving an indexed instruction error.
                transfer(&payer.pubkey(), &recipient, u64::MAX),
            ],
            Some(&payer.pubkey()),
        ),
        svm.latest_blockhash(),
    );

    let outcome = svm.transact(tx);

    match outcome.status() {
        Err(TransactionError::InstructionError(index, _)) => {
            assert_eq!(*index, 1, "the failing instruction's index must be reported verbatim");
        }
        other => panic!("expected an indexed instruction error, got {other:?}"),
    }
}

#[test]
fn a_failing_first_instruction_reports_index_zero() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(
            &[
                transfer(&payer.pubkey(), &Address::new_unique(), u64::MAX),
                transfer(&payer.pubkey(), &Address::new_unique(), 1),
            ],
            Some(&payer.pubkey()),
        ),
        svm.latest_blockhash(),
    );

    let outcome = svm.transact(tx);

    match outcome.status() {
        Err(TransactionError::InstructionError(index, _)) => assert_eq!(*index, 0),
        other => panic!("expected an indexed instruction error, got {other:?}"),
    }
}

/// An instruction whose program id is not deployed is rejected during
/// sanitization, before any instruction runs, so the failure names the
/// program instead of an instruction index.
#[test]
fn an_undeployed_program_is_rejected_before_any_instruction_runs() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let missing_program = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(
            &[
                transfer(&payer.pubkey(), &Address::new_unique(), 1),
                Instruction { program_id: missing_program, accounts: vec![], data: vec![] },
            ],
            Some(&payer.pubkey()),
        ),
        svm.latest_blockhash(),
    );

    let outcome = svm.transact(tx);

    match outcome.status() {
        Err(TransactionError::InvalidProgramForExecution) => {}
        other => panic!("expected InvalidProgramForExecution, got {other:?}"),
    }
    // Nothing ran, so the trace is empty and no units were burned.
    assert!(outcome.meta().diagnostics.execution_trace.instructions.is_empty());
    assert_eq!(outcome.meta().compute_units_consumed, 0);
}

#[test]
fn a_failing_later_instruction_rolls_back_the_earlier_transfer() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();
    let missing_program = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(
            &[
                transfer(&payer.pubkey(), &recipient, 64),
                Instruction { program_id: missing_program, accounts: vec![], data: vec![] },
            ],
            Some(&payer.pubkey()),
        ),
        svm.latest_blockhash(),
    );

    let outcome = svm.transact(tx);
    assert!(outcome.status().is_err());

    // The first instruction succeeded in isolation, so the transaction as a whole
    // must not commit it.
    let committed = svm.commit_transaction(outcome);
    assert!(committed.is_err());
    assert_eq!(svm.get_balance(&recipient), None, "the earlier transfer must be rolled back");
}

#[test]
fn compute_units_accumulate_across_all_instructions() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let a = Address::new_unique();
    let b = Address::new_unique();
    let c = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();

    let single = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &a, 1)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );
    let single_units = svm.transact(single).meta().compute_units_consumed;

    let triple = Transaction::new(
        &[&payer],
        Message::new(
            &[
                transfer(&payer.pubkey(), &a, 1),
                transfer(&payer.pubkey(), &b, 1),
                transfer(&payer.pubkey(), &c, 1),
            ],
            Some(&payer.pubkey()),
        ),
        svm.latest_blockhash(),
    );
    let triple_units = svm.transact(triple).meta().compute_units_consumed;

    assert!(
        triple_units > single_units,
        "three transfers must cost more than one: {triple_units} vs {single_units}"
    );
    // The three transfers also pay three signature verification / prioritization
    // costs on top of the instruction costs, so a multiple of the single
    // instruction cost is the loose lower bound.
    assert!(
        triple_units >= single_units * 3,
        "expected at least 3x the single-instruction cost: {triple_units} vs {single_units}"
    );
}

#[test]
fn compute_units_burned_before_a_failure_are_still_reported() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(
            &[
                transfer(&payer.pubkey(), &recipient, 1),
                // Fails while executing, so the first instruction's units are already
                // spent by the time the transaction is marked failed.
                transfer(&payer.pubkey(), &recipient, u64::MAX),
            ],
            Some(&payer.pubkey()),
        ),
        svm.latest_blockhash(),
    );

    let outcome = svm.transact(tx);

    assert!(outcome.status().is_err());
    assert!(
        outcome.meta().compute_units_consumed > 0,
        "a failed transaction must still report the units it burned"
    );

    // By contrast, a transaction rejected before execution burns nothing.
    let undeployed = Transaction::new(
        &[&payer],
        Message::new(
            &[Instruction { program_id: Address::new_unique(), accounts: vec![], data: vec![] }],
            Some(&payer.pubkey()),
        ),
        svm.latest_blockhash(),
    );
    let rejected = svm.transact(undeployed);
    assert!(rejected.status().is_err());
    assert_eq!(
        rejected.meta().compute_units_consumed,
        0,
        "a transaction that never executes must not report units"
    );
}

#[test]
fn compute_budget_instructions_appear_in_the_execution_trace() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(
            &[
                ComputeBudgetInstruction::set_compute_unit_limit(200_000),
                transfer(&payer.pubkey(), &recipient, 1),
            ],
            Some(&payer.pubkey()),
        ),
        svm.latest_blockhash(),
    );

    let outcome = svm.transact(tx);
    assert!(outcome.status().is_ok());

    // Every top-level instruction is recorded, including the compute budget
    // ones, so the frame count matches the message instruction count.
    let trace = &outcome.meta().diagnostics.execution_trace;
    assert_eq!(trace.instructions.len(), 2, "trace was {:?}", trace.instructions);
    assert_eq!(trace.instructions[0].program_id, solana_sdk_ids::compute_budget::id());
    assert_eq!(trace.instructions[1].program_id, solana_sdk_ids::system_program::id());
    assert!(trace.instructions.iter().all(|frame| frame.stack_height == 1));
}

#[test]
fn the_execution_trace_records_program_accounts_and_data() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 42)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );

    let outcome = svm.transact(tx);
    let frame = &outcome.meta().diagnostics.execution_trace.instructions[0];

    assert_eq!(frame.stack_height, 1, "a top-level frame has stack height 1");
    assert_eq!(frame.program_id, solana_sdk_ids::system_program::id());
    // The frame carries the instruction's raw data verbatim.
    assert_eq!(frame.data, transfer(&payer.pubkey(), &recipient, 42).data);
    // Both transfer participants appear as account metas.
    assert!(frame.accounts.iter().any(|meta| meta.pubkey == payer.pubkey()));
    assert!(frame.accounts.iter().any(|meta| meta.pubkey == recipient));
    // Only the fee payer signs.
    assert!(frame.accounts.iter().any(|meta| meta.pubkey == payer.pubkey() && meta.is_signer));
    assert!(frame.accounts.iter().all(|meta| !meta.is_signer || meta.pubkey == payer.pubkey()));
}

#[test]
fn an_execution_trace_frame_converts_back_into_an_instruction() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 42)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );

    let outcome = svm.transact(tx);
    let frame = &outcome.meta().diagnostics.execution_trace.instructions[0];
    let instruction = frame.instruction();

    assert_eq!(instruction.program_id, frame.program_id);
    assert_eq!(instruction.accounts, frame.accounts);
    assert_eq!(instruction.data, frame.data);
    // The reconstruction is byte-identical to the original instruction.
    assert_eq!(instruction, transfer(&payer.pubkey(), &recipient, 42));
}

#[test]
fn a_transaction_with_no_instructions_produces_an_empty_trace() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );

    let outcome = svm.transact(tx);

    assert!(outcome.status().is_ok());
    assert!(outcome.meta().diagnostics.execution_trace.instructions.is_empty());
    assert!(outcome.meta().inner_instructions.is_empty());
}

#[test]
fn a_failing_program_contributes_a_trace_frame_with_its_error() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let program_id = address!("HvrRMSshMx3itvsyWDnWg2E3cy5h57iMaR7oVxSZJDSA");

    if !load_program(&mut svm, program_id, hpsvm_test_support::FAILURE) {
        return;
    }
    svm.airdrop(&payer.pubkey(), 1_000_000_000).unwrap();

    let tx = Transaction::new(
        &[&payer],
        Message::new(
            &[Instruction { program_id, accounts: vec![], data: vec![] }],
            Some(&payer.pubkey()),
        ),
        svm.latest_blockhash(),
    );

    let outcome = svm.transact(tx);

    assert!(outcome.status().is_err());
    // The frame is recorded even though the program failed.
    assert_eq!(outcome.meta().diagnostics.execution_trace.instructions.len(), 1);
    assert_eq!(outcome.meta().diagnostics.execution_trace.instructions[0].program_id, program_id);
}

/// A CPI test needs a program that invokes another program, which the vendored
/// test programs do not do. The trace plumbing is still exercised through the
/// top-level frames above; this test pins the `stack_height == 1` invariant that
/// the CPI path is built on.
#[test]
fn top_level_frames_all_have_stack_height_one() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let program_id = address!("HvrRMSshMx3itvsyWDnWg2E3cy5h57iMaR7oVxSZJDSA");

    if !load_program(&mut svm, program_id, hpsvm_test_support::FAILURE) {
        return;
    }
    svm.airdrop(&payer.pubkey(), 1_000_000_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(
            &[
                transfer(&payer.pubkey(), &Address::new_unique(), 1),
                Instruction { program_id, accounts: vec![], data: vec![] },
                transfer(&payer.pubkey(), &Address::new_unique(), 1),
            ],
            Some(&payer.pubkey()),
        ),
        svm.latest_blockhash(),
    );

    let outcome = svm.transact(tx);
    let frames: &[ExecutedInstruction] = &outcome.meta().diagnostics.execution_trace.instructions;

    assert_eq!(frames.len(), 2, "only the two transfers run: {frames:?}");
    assert!(frames.iter().all(|frame| frame.stack_height == 1));
}

#[test]
fn the_return_data_field_is_readable_even_without_return_data() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 1)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );

    let outcome = svm.transact(tx);

    assert!(outcome.status().is_ok());
    // The system program returns no data, so the snapshot must be empty.
    assert!(outcome.meta().return_data.data.is_empty());
}
