#![allow(missing_docs)]
#![cfg(feature = "fd-compat")]

use std::{path::PathBuf, process::Command};

use hpsvm::{HPSVM, instruction::InstructionCase};
use hpsvm_fixture::{AccountSnapshot, ExecutionSnapshot};
use hpsvm_fixture_fd::FiredancerFixture;
use mollusk_svm_fuzz_fixture_firedancer as fd_codec;
use solana_account::Account;
use solana_address::Address;
use solana_system_interface::instruction::transfer;

fn temp_fixture_path() -> PathBuf {
    let unique = Address::new_unique();
    std::env::temp_dir().join(format!("hpsvm-cli-fd-compat-{unique}.fix"))
}

fn address_bytes(address: Address) -> Vec<u8> {
    address.to_bytes().to_vec()
}

fn account_state(snapshot: &AccountSnapshot) -> fd_codec::proto::AcctState {
    fd_codec::proto::AcctState {
        address: address_bytes(snapshot.address),
        owner: address_bytes(snapshot.owner),
        lamports: snapshot.lamports,
        data: snapshot.data.clone(),
        executable: snapshot.executable,
        rent_epoch: snapshot.rent_epoch,
        seed_addr: None,
    }
}

fn write_firedancer_system_transfer_fixture() -> PathBuf {
    let svm = HPSVM::new();
    let sender = Address::new_unique();
    let recipient = Address::new_unique();
    let instruction = transfer(&sender, &recipient, 64);
    let sender_account = Account {
        lamports: 10_000,
        owner: solana_sdk_ids::system_program::id(),
        ..Default::default()
    };
    let recipient_account =
        Account { lamports: 1, owner: solana_sdk_ids::system_program::id(), ..Default::default() };
    let case = InstructionCase {
        program_id: instruction.program_id,
        accounts: instruction.accounts.clone(),
        data: instruction.data.clone(),
        pre_accounts: vec![
            (sender, sender_account.clone()),
            (recipient, recipient_account.clone()),
        ],
    };
    let baseline = ExecutionSnapshot::from_outcome(
        &svm.process_instruction_case(&case).expect("instruction case should execute"),
    );
    let cu_avail = 1_000_000;
    let fd_fixture = FiredancerFixture::from_proto(fd_codec::proto::InstrFixture {
        metadata: Some(fd_codec::proto::FixtureMetadata {
            fn_entrypoint: String::from("fd-system-transfer"),
        }),
        input: Some(fd_codec::proto::InstrContext {
            program_id: address_bytes(instruction.program_id),
            accounts: vec![
                account_state(&AccountSnapshot::from_readable(sender, &sender_account)),
                account_state(&AccountSnapshot::from_readable(recipient, &recipient_account)),
            ],
            instr_accounts: vec![
                fd_codec::proto::InstrAcct { index: 0, is_writable: true, is_signer: true },
                fd_codec::proto::InstrAcct { index: 1, is_writable: true, is_signer: false },
            ],
            data: instruction.data,
            cu_avail,
            slot_context: Some(fd_codec::proto::SlotContext { slot: svm.block_env().slot }),
            epoch_context: None,
        }),
        output: Some(fd_codec::proto::InstrEffects {
            result: 0,
            custom_err: 0,
            modified_accounts: baseline.post_accounts.iter().map(account_state).collect(),
            cu_avail: cu_avail - baseline.compute_units_consumed,
            return_data: Vec::new(),
        }),
    });
    let path = temp_fixture_path();
    fd_fixture.save(&path).expect("firedancer fixture should save");
    path
}

#[test]
fn fixture_inspect_can_import_firedancer_fixture() {
    let path = write_firedancer_system_transfer_fixture();

    let output = Command::new(env!("CARGO_BIN_EXE_hpsvm"))
        .args([
            "fixture",
            "inspect",
            path.to_str().expect("temp path must be valid utf-8"),
            "--fixture-format",
            "firedancer",
        ])
        .output()
        .expect("inspect command must execute");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"name\": \"fd-system-transfer\""));
    assert!(stdout.contains("\"source\": \"firedancer\""));

    std::fs::remove_file(path).ok();
}

#[test]
fn fixture_run_can_replay_firedancer_instruction_fixture() {
    let path = write_firedancer_system_transfer_fixture();

    let output = Command::new(env!("CARGO_BIN_EXE_hpsvm"))
        .args([
            "fixture",
            "run",
            path.to_str().expect("temp path must be valid utf-8"),
            "--fixture-format",
            "firedancer",
        ])
        .output()
        .expect("run command must execute");

    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stdout).contains("PASS: fd-system-transfer"));

    std::fs::remove_file(path).ok();
}
