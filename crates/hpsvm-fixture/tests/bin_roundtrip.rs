#![allow(missing_docs)]
#![cfg(feature = "bin-codec")]

use hpsvm::HPSVM;
use hpsvm_fixture::{
    AccountSnapshot, CaptureBuilder, Compare, ExecutionSnapshot, Fixture, FixtureExpectations,
    FixtureFormat, FixtureHeader, FixtureInput, FixtureKind, InstructionAccountMeta,
    InstructionFixture, RuntimeFixtureConfig,
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

fn snapshot_readable(address: Address, account: &Account) -> AccountSnapshot {
    AccountSnapshot::from_readable(address, account)
}

#[test]
fn binary_fixture_roundtrip_preserves_transaction_fixture() {
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

    let fixture = CaptureBuilder::new("system-transfer-binary")
        .runtime(RuntimeFixtureConfig::new(svm.block_env().slot, None, true, false))
        .pre_accounts(vec![
            snapshot_account(&svm, payer.pubkey()),
            snapshot_account(&svm, recipient),
        ])
        .baseline(baseline)
        .compares(Compare::everything())
        .capture_transaction(&tx)
        .unwrap();

    let path = std::env::temp_dir().join("hpsvm-fixture-roundtrip.bin");
    fixture.save(&path, FixtureFormat::Binary).unwrap();
    let loaded = Fixture::load(&path).unwrap();

    assert_eq!(loaded, fixture);

    std::fs::remove_file(path).ok();
}

#[test]
fn binary_fixture_roundtrip_preserves_instruction_fixture() {
    let svm = HPSVM::new();
    let sender = Address::new_unique();
    let recipient = Address::new_unique();
    let instruction = transfer(&sender, &recipient, 64);
    let sender_account =
        Account { lamports: 10_000, owner: instruction.program_id, ..Default::default() };
    let recipient_account =
        Account { lamports: 1, owner: instruction.program_id, ..Default::default() };
    let baseline = ExecutionSnapshot::from_outcome(
        &svm.process_instruction_case(&hpsvm::instruction::InstructionCase {
            program_id: instruction.program_id,
            accounts: instruction.accounts.clone(),
            data: instruction.data.clone(),
            pre_accounts: vec![
                (sender, sender_account.clone()),
                (recipient, recipient_account.clone()),
            ],
        })
        .unwrap(),
    );

    let fixture = Fixture::new(
        FixtureHeader::new("instruction-transfer-binary", FixtureKind::Instruction),
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
    );

    let path = std::env::temp_dir().join("hpsvm-instruction-fixture-roundtrip.bin");
    fixture.save(&path, FixtureFormat::Binary).unwrap();
    let loaded = Fixture::load(&path).unwrap();

    assert_eq!(loaded, fixture);

    std::fs::remove_file(path).ok();
}
