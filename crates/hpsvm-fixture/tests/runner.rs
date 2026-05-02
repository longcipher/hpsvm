#![allow(missing_docs)]

use hpsvm::HPSVM;
use hpsvm_fixture::{
    AccountSnapshot, CaptureBuilder, Compare, ExecutionSnapshot, Fixture, FixtureError,
    FixtureExpectations, FixtureHeader, FixtureInput, FixtureKind, FixtureRunner,
    InstructionAccountMeta, InstructionFixture, ProgramBinding, ResultConfig, RuntimeFixtureConfig,
};
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

fn snapshot_readable(address: Address, account: &Account) -> AccountSnapshot {
    AccountSnapshot::from_readable(address, account)
}

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
