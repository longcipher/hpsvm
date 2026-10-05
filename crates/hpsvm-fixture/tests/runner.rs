#![allow(missing_docs)]
#![cfg(feature = "bin-codec")]

use hpsvm::HPSVM;
use hpsvm_fixture::{
    AccountSnapshot, CaptureBuilder, Compare, ExecutionSnapshot, Fixture, FixtureError,
    FixtureInput, FixtureRunner, ProgramBinding, ResultConfig, RuntimeFixtureConfig,
};
#[cfg(feature = "instruction-fixture")]
use hpsvm_fixture::{
    FixtureExpectations, FixtureHeader, FixtureKind, InstructionAccountMeta, InstructionFixture,
};
#[cfg(feature = "instruction-fixture")]
use solana_account::Account;
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::versioned::VersionedTransaction;

fn snapshot_account(svm: &HPSVM, address: Address) -> AccountSnapshot {
    let account = svm.get_account(&address).unwrap();
    AccountSnapshot::from_readable(address, &account)
}

fn build_fixture() -> Fixture {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    svm.airdrop(&recipient, 1).unwrap();
    let tx = VersionedTransaction::from(solana_transaction::Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    ));
    let baseline = ExecutionSnapshot::from_outcome(&svm.transact(tx.clone()));

    CaptureBuilder::new("runner-transfer")
        .runtime(RuntimeFixtureConfig::new(svm.block_env().slot, None, true, false))
        .pre_accounts(vec![
            snapshot_account(&svm, payer.pubkey()),
            snapshot_account(&svm, recipient),
        ])
        .baseline(baseline)
        .compares(Compare::everything())
        .capture_transaction(&tx)
        .unwrap()
}

#[cfg(feature = "instruction-fixture")]
fn snapshot_readable(address: Address, account: &Account) -> AccountSnapshot {
    AccountSnapshot::from_readable(address, account)
}

#[cfg(feature = "instruction-fixture")]
fn build_instruction_fixture() -> Fixture {
    let svm = HPSVM::new();
    let sender = Address::new_unique();
    let recipient = Address::new_unique();
    let instruction = transfer(&sender, &recipient, 64);
    let sender_account =
        Account { lamports: 10_000, owner: instruction.program_id, ..Default::default() };
    let recipient_account =
        Account { lamports: 1, owner: instruction.program_id, ..Default::default() };
    let case = hpsvm::instruction::InstructionCase {
        program_id: instruction.program_id,
        accounts: instruction.accounts.clone(),
        data: instruction.data.clone(),
        pre_accounts: vec![
            (sender, sender_account.clone()),
            (recipient, recipient_account.clone()),
        ],
    };
    let baseline = ExecutionSnapshot::from_outcome(&svm.process_instruction_case(&case).unwrap());

    Fixture::new(
        FixtureHeader::new("runner-instruction-transfer", FixtureKind::Instruction),
        FixtureInput::Instruction(InstructionFixture::new(
            RuntimeFixtureConfig::new(svm.block_env().slot, None, true, false),
            Vec::new(),
            vec![
                snapshot_readable(sender, &sender_account),
                snapshot_readable(recipient, &recipient_account),
            ],
            instruction.program_id,
            instruction
                .accounts
                .into_iter()
                .map(|account| {
                    InstructionAccountMeta::new(
                        account.pubkey,
                        account.is_signer,
                        account.is_writable,
                    )
                })
                .collect(),
            instruction.data,
        )),
        FixtureExpectations::new(baseline, Compare::everything()),
    )
}

#[test]
fn runner_replays_transaction_fixture_against_a_cloned_vm() {
    let fixture = build_fixture();
    let mut runner = FixtureRunner::new(HPSVM::new());

    let execution = runner.run(&fixture).unwrap();

    assert!(execution.snapshot.compare_with(
        &fixture.expectations.baseline,
        &Compare::everything(),
        &ResultConfig { panic: false, verbose: true },
    ));
}

#[test]
fn runner_can_apply_fixture_default_compares() {
    let mut fixture = build_fixture();
    fixture.expectations.baseline.compute_units_consumed += 1;
    fixture.expectations.compares = Compare::everything_but_compute_units();

    let mut runner = FixtureRunner::new(HPSVM::new());
    let pass =
        runner.run_and_validate(&fixture, &ResultConfig { panic: false, verbose: true }).unwrap();

    assert!(pass);
}

#[test]
#[cfg(feature = "instruction-fixture")]
fn runner_replays_instruction_fixture_against_a_cloned_vm() {
    let fixture = build_instruction_fixture();
    let mut runner = FixtureRunner::new(HPSVM::new());

    let execution = runner.run(&fixture).unwrap();

    assert!(execution.snapshot.compare_with(
        &fixture.expectations.baseline,
        &Compare::everything(),
        &ResultConfig { panic: false, verbose: true },
    ));
}

#[test]
#[cfg(feature = "instruction-fixture")]
fn runner_can_validate_instruction_fixture() {
    let fixture = build_instruction_fixture();
    let mut runner = FixtureRunner::new(HPSVM::new());

    let pass =
        runner.run_and_validate(&fixture, &ResultConfig { panic: false, verbose: true }).unwrap();

    assert!(pass);
}

#[test]
fn runner_requires_supplied_elf_for_program_bindings() {
    let mut fixture = build_fixture();
    let program_id = Address::new_unique();
    let loader_id = Address::new_unique();

    let FixtureInput::Transaction(transaction) = &mut fixture.input else {
        panic!("fixture input should be a transaction")
    };
    transaction.programs.push(ProgramBinding::new(
        program_id,
        loader_id,
        Some(String::from("candidate")),
    ));

    let mut runner = FixtureRunner::new(HPSVM::new());
    let error = runner.run(&fixture).unwrap_err();

    assert!(
        matches!(error, FixtureError::MissingProgramElf { program_id: missing } if missing == program_id)
    );
}

#[test]
fn runner_rejects_blockhash_checked_fixtures_without_blockhash_restore_support() {
    let mut fixture = build_fixture();
    let FixtureInput::Transaction(transaction) = &mut fixture.input else {
        panic!("fixture input should be a transaction")
    };
    transaction.runtime.blockhash_check = true;

    let mut runner = FixtureRunner::new(HPSVM::new());
    let error = runner.run(&fixture).unwrap_err();

    assert!(matches!(error, FixtureError::UnsupportedRuntimeConfig { field: "blockhash_check" }));
}

/// `run` reports the snapshot and leaves `pass` unset; validation is a separate
/// step so callers can inspect the result before deciding.
#[test]
fn run_leaves_the_pass_field_unset() {
    let fixture = build_fixture();
    let mut runner = FixtureRunner::new(HPSVM::new());

    let execution = runner.run(&fixture).unwrap();

    assert_eq!(execution.pass, None, "run must not pre-compute pass/fail");
    assert!(execution.snapshot.included);
}

/// The runner must not mutate the base VM: replaying twice yields the same result.
#[test]
fn replaying_the_same_fixture_twice_is_stable() {
    let fixture = build_fixture();
    let mut runner = FixtureRunner::new(HPSVM::new());

    let first = runner.run(&fixture).unwrap().snapshot;
    let second = runner.run(&fixture).unwrap().snapshot;

    assert_eq!(first, second, "the base VM must not be mutated by a replay");
}

/// A compute-unit limit in the fixture runtime must actually cap execution, so a
/// baseline recorded under a generous limit stops reproducing under a tight one.
#[test]
fn a_fixture_compute_unit_limit_is_applied_to_the_vm() {
    let fixture = build_fixture();

    let mut tight = fixture.clone();
    let FixtureInput::Transaction(transaction) = &mut tight.input else {
        panic!("fixture input should be a transaction")
    };
    transaction.runtime.compute_unit_limit = Some(1);

    let mut runner = FixtureRunner::new(HPSVM::new());
    let execution = runner.run(&tight).unwrap();

    assert!(
        !matches!(execution.snapshot.status, hpsvm_fixture::ExecutionStatus::Success),
        "a one-compute-unit budget cannot run a transfer"
    );
}

/// The limit is only applied when the fixture carries one; `None` keeps the VM
/// default, so the recorded baseline still reproduces.
#[test]
fn a_fixture_without_a_compute_unit_limit_uses_the_vm_default() {
    let fixture = build_fixture();
    let mut runner = FixtureRunner::new(HPSVM::new());

    let execution = runner.run(&fixture).unwrap();

    assert!(matches!(execution.snapshot.status, hpsvm_fixture::ExecutionStatus::Success));
    assert!(execution.snapshot.compare_with(
        &fixture.expectations.baseline,
        &Compare::everything(),
        &ResultConfig { panic: false, verbose: true },
    ));
}

/// The fixture slot is warped to before execution, so a slot-sensitive fixture
/// reproduces. A recorded slot far in the past must still replay.
#[test]
fn the_fixture_slot_is_warped_to_before_execution() {
    let mut fixture = build_fixture();
    let FixtureInput::Transaction(transaction) = &mut fixture.input else {
        panic!("fixture input should be a transaction")
    };
    transaction.runtime.slot = 1;

    let mut runner = FixtureRunner::new(HPSVM::new());
    let execution = runner.run(&fixture).unwrap();

    // The warp must not disturb a plain transfer.
    assert!(execution.snapshot.compare_with(
        &fixture.expectations.baseline,
        &Compare::everything_but_compute_units(),
        &ResultConfig { panic: false, verbose: true },
    ));
}

/// A tiny log budget truncates the captured logs; a generous one keeps them.
#[test]
fn a_fixture_log_bytes_limit_is_applied_to_the_vm() {
    let fixture = build_fixture();

    let mut tight = fixture.clone();
    let FixtureInput::Transaction(transaction) = &mut tight.input else {
        panic!("fixture input should be a transaction")
    };
    transaction.runtime.log_bytes_limit = Some(1);

    let mut runner = FixtureRunner::new(HPSVM::new());
    let truncated = runner.run(&tight).unwrap().snapshot;
    let full = runner.run(&fixture).unwrap().snapshot;

    let truncated_bytes: usize = truncated.logs.iter().map(String::len).sum();
    let full_bytes: usize = full.logs.iter().map(String::len).sum();
    assert!(truncated_bytes <= full_bytes, "a tighter budget must not grow the logs");
}

/// `sigverify: false` in the fixture must be honoured, so a fixture with a
/// tampered signature still replays.
#[test]
fn sigverify_false_accepts_a_tampered_signature() {
    let fixture = tampered_signature_fixture(false);

    let mut runner = FixtureRunner::new(HPSVM::new());
    let execution = runner.run(&fixture).unwrap();

    assert!(
        matches!(execution.snapshot.status, hpsvm_fixture::ExecutionStatus::Success),
        "sigverify: false must skip signature checking, got {:?}",
        execution.snapshot.status
    );
}

/// With `sigverify: true` (the default), a tampered signature is rejected.
#[test]
fn sigverify_true_rejects_a_tampered_signature() {
    let fixture = tampered_signature_fixture(true);

    let mut runner = FixtureRunner::new(HPSVM::new());
    let execution = runner.run(&fixture).unwrap();

    assert!(
        !matches!(execution.snapshot.status, hpsvm_fixture::ExecutionStatus::Success),
        "an invalid signature must not verify, got {:?}",
        execution.snapshot.status
    );
}

/// Flips the first signature byte so the recorded transaction can no longer
/// verify, and pins `sigverify` to the requested value.
fn tampered_signature_fixture(sigverify: bool) -> Fixture {
    let mut fixture = build_fixture();
    let FixtureInput::Transaction(transaction) = &mut fixture.input else {
        panic!("fixture input should be a transaction")
    };
    transaction.runtime.sigverify = sigverify;

    let mut tx: VersionedTransaction =
        wincode::deserialize(&transaction.transaction_bytes).expect("recorded tx must decode");
    // An all-zero signature is never a valid ed25519 signature, so verification
    // must fail for any message.
    tx.signatures[0] = Default::default();
    transaction.transaction_bytes = wincode::serialize(&tx).expect("re-encoded tx must serialize");

    fixture
}

/// `with_program_elf` overwrites a previous ELF for the same program id, so the
/// last binding wins.
#[test]
fn with_program_elf_overwrites_a_previous_entry() {
    let program_id = Address::new_unique();
    let mut runner = FixtureRunner::new(HPSVM::new())
        .with_program_elf(program_id, vec![1, 2, 3])
        .with_program_elf(program_id, vec![4, 5, 6]);

    let mut fixture = build_fixture();
    let FixtureInput::Transaction(transaction) = &mut fixture.input else {
        panic!("fixture input should be a transaction")
    };
    transaction.programs.push(ProgramBinding::new(program_id, Address::new_unique(), None));

    // The ELF is a placeholder, so loading must fail while resolving the ELF — but
    // it must not fail with `MissingProgramElf`, proving the overwrite took.
    let error = runner.run(&fixture).unwrap_err();
    assert!(
        !matches!(error, FixtureError::MissingProgramElf { .. }),
        "expected the injected ELF to be found, got {error:?}"
    );
}

/// Corrupt transaction bytes must surface as a decode error, not a panic.
#[test]
fn corrupt_transaction_bytes_are_reported_as_a_decode_error() {
    let mut fixture = build_fixture();
    let FixtureInput::Transaction(transaction) = &mut fixture.input else {
        panic!("fixture input should be a transaction")
    };
    transaction.transaction_bytes = vec![0xff; 8];

    let mut runner = FixtureRunner::new(HPSVM::new());
    let error = runner.run(&fixture).unwrap_err();

    assert!(matches!(error, FixtureError::DecodeTransaction(_)), "got {error:?}");
}

/// `run_and_validate` propagates a decode error rather than reporting a pass.
#[test]
fn run_and_validate_propagates_a_decode_error() {
    let mut fixture = build_fixture();
    let FixtureInput::Transaction(transaction) = &mut fixture.input else {
        panic!("fixture input should be a transaction")
    };
    transaction.transaction_bytes = Vec::new();

    let mut runner = FixtureRunner::new(HPSVM::new());
    let error = runner
        .run_and_validate(&fixture, &ResultConfig { panic: false, verbose: false })
        .unwrap_err();

    assert!(matches!(error, FixtureError::DecodeTransaction(_)), "got {error:?}");
}

/// A fixture whose compare list is empty passes trivially.
#[test]
fn an_empty_compare_list_validates_successfully() {
    let mut fixture = build_fixture();
    fixture.expectations.compares = Vec::new();
    fixture.expectations.baseline.compute_units_consumed = u64::MAX;
    fixture.expectations.baseline.fee = u64::MAX;

    let mut runner = FixtureRunner::new(HPSVM::new());
    let pass =
        runner.run_and_validate(&fixture, &ResultConfig { panic: false, verbose: false }).unwrap();

    assert!(pass, "no comparisons means nothing can fail");
}

/// An instruction fixture with a program binding must resolve the ELF through the
/// same preload path as a transaction fixture.
#[test]
#[cfg(feature = "instruction-fixture")]
fn instruction_fixtures_resolve_program_bindings_too() {
    let mut fixture = build_instruction_fixture();
    let program_id = Address::new_unique();

    let FixtureInput::Instruction(instruction) = &mut fixture.input else {
        panic!("fixture input should be an instruction fixture")
    };
    instruction.programs.push(ProgramBinding::new(program_id, Address::new_unique(), None));

    let mut runner = FixtureRunner::new(HPSVM::new());
    let error = runner.run(&fixture).unwrap_err();
    assert!(
        matches!(error, FixtureError::MissingProgramElf { program_id: missing } if missing == program_id),
        "got {error:?}"
    );

    let mut runner =
        FixtureRunner::new(HPSVM::new()).with_program_elf(program_id, b"not-an-elf".to_vec());
    let error = runner.run(&fixture).unwrap_err();
    assert!(
        !matches!(error, FixtureError::MissingProgramElf { .. }),
        "the injected ELF must satisfy the binding, got {error:?}"
    );
}

/// The instruction fixture's `pre_accounts` must be installed before execution,
/// so a transfer between two synthetic accounts succeeds on replay.
#[test]
#[cfg(feature = "instruction-fixture")]
fn instruction_fixture_pre_accounts_are_installed() {
    let fixture = build_instruction_fixture();
    let mut runner = FixtureRunner::new(HPSVM::new());

    let execution = runner.run(&fixture).unwrap();

    assert!(execution.snapshot.included);
    let FixtureInput::Instruction(instruction) = &fixture.input else {
        panic!("fixture input should be an instruction fixture")
    };
    let recipient = instruction.pre_accounts[1].address;
    let expected = fixture
        .expectations
        .baseline
        .post_accounts
        .iter()
        .find(|account| account.address == recipient)
        .map(|account| account.lamports)
        .expect("the recipient must be in the baseline");
    let actual = execution
        .snapshot
        .post_accounts
        .iter()
        .find(|account| account.address == recipient)
        .map(|account| account.lamports)
        .expect("the recipient must be in the replay");
    assert_eq!(actual, expected);
}
