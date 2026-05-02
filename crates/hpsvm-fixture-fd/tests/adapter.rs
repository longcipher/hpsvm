#![allow(missing_docs)]

use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use hpsvm_fixture::{
    AccountCompareScope, AccountSnapshot, Compare, ExecutionSnapshot, ExecutionSnapshotFields,
    ExecutionStatus, Fixture, FixtureExpectations, FixtureHeader, FixtureInput, FixtureKind,
    InstructionAccountMeta, InstructionFixture, RuntimeFixtureConfig,
};
use hpsvm_fixture_fd::{AdapterError, FiredancerFixture};
use mollusk_svm_fuzz_fixture_firedancer as fd_codec;
use solana_address::Address;

fn address_bytes(fill: u8) -> Vec<u8> {
    vec![fill; 32]
}

fn address(fill: u8) -> Address {
    Address::new_from_array([fill; 32])
}

fn sample_proto_fixture() -> fd_codec::proto::InstrFixture {
    fd_codec::proto::InstrFixture {
        metadata: Some(fd_codec::proto::FixtureMetadata {
            fn_entrypoint: String::from("fd-entrypoint"),
        }),
        input: Some(fd_codec::proto::InstrContext {
            program_id: address_bytes(9),
            accounts: vec![
                fd_codec::proto::AcctState {
                    address: address_bytes(1),
                    owner: address_bytes(7),
                    lamports: 15,
                    data: vec![1, 2, 3],
                    executable: false,
                    rent_epoch: 0,
                    seed_addr: None,
                },
                fd_codec::proto::AcctState {
                    address: address_bytes(2),
                    owner: address_bytes(7),
                    lamports: 21,
                    data: vec![4, 5, 6],
                    executable: false,
                    rent_epoch: 0,
                    seed_addr: None,
                },
            ],
            instr_accounts: vec![
                fd_codec::proto::InstrAcct { index: 0, is_writable: true, is_signer: true },
                fd_codec::proto::InstrAcct { index: 1, is_writable: true, is_signer: false },
            ],
            data: vec![8, 6, 7, 5, 3, 0, 9],
            cu_avail: 500,
            slot_context: Some(fd_codec::proto::SlotContext { slot: 77 }),
            epoch_context: None,
        }),
        output: Some(fd_codec::proto::InstrEffects {
            result: 0,
            custom_err: 0,
            modified_accounts: vec![fd_codec::proto::AcctState {
                address: address_bytes(2),
                owner: address_bytes(7),
                lamports: 99,
                data: vec![9, 9],
                executable: false,
                rent_epoch: 0,
                seed_addr: None,
            }],
            cu_avail: 420,
            return_data: vec![4, 2],
        }),
    }
}

fn temp_fixture_path(extension: &str) -> PathBuf {
    let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos(),
        Err(_) => 0,
    };
    std::env::temp_dir()
        .join(format!("hpsvm-fixture-fd-{pid}-{nanos}.{extension}", pid = std::process::id()))
}

#[test]
fn firedancer_fixture_save_and_load_roundtrip_preserves_proto_shape() -> Result<(), AdapterError> {
    let fixture = FiredancerFixture::from_proto(sample_proto_fixture());
    let path = temp_fixture_path("fix");

    fixture.save(&path)?;
    let loaded = FiredancerFixture::load(&path)?;

    assert_eq!(loaded.as_proto(), fixture.as_proto());

    let _ = std::fs::remove_file(path);
    Ok(())
}

#[test]
fn firedancer_fixture_imports_into_hpsvm_instruction_fixture() -> Result<(), AdapterError> {
    let fixture = FiredancerFixture::from_proto(sample_proto_fixture());
    let imported = Fixture::try_from(fixture)?;

    assert_eq!(imported.header.kind, FixtureKind::Instruction);
    assert_eq!(imported.header.name, "fd-entrypoint");
    assert_eq!(imported.header.source.as_deref(), Some("firedancer"));

    let FixtureInput::Instruction(instruction) = imported.input else {
        panic!("expected instruction fixture")
    };
    assert_eq!(instruction.runtime.slot, 77);
    assert_eq!(instruction.programs, Vec::new());
    assert_eq!(instruction.program_id, address(9));
    assert_eq!(instruction.data, vec![8, 6, 7, 5, 3, 0, 9]);
    assert_eq!(
        instruction.accounts,
        vec![
            InstructionAccountMeta::new(address(1), true, true),
            InstructionAccountMeta::new(address(2), false, true),
        ]
    );
    assert_eq!(instruction.pre_accounts.len(), 2);

    assert_eq!(imported.expectations.baseline.status, ExecutionStatus::Success);
    assert!(imported.expectations.baseline.included);
    assert_eq!(imported.expectations.baseline.compute_units_consumed, 80);
    assert_eq!(imported.expectations.baseline.fee, 0);
    assert_eq!(imported.expectations.baseline.logs, Vec::<String>::new());
    assert_eq!(imported.expectations.baseline.post_accounts.len(), 1);
    assert_eq!(
        imported.expectations.baseline.return_data.as_ref().map(|value| value.data.clone()),
        Some(vec![4, 2])
    );
    assert_eq!(
        imported.expectations.compares,
        vec![
            Compare::Status,
            Compare::Included,
            Compare::ComputeUnits,
            Compare::ReturnData,
            Compare::Accounts(AccountCompareScope::Only(vec![address(2)])),
        ]
    );

    Ok(())
}

#[test]
fn hpsvm_instruction_fixture_exports_to_firedancer_proto() -> Result<(), AdapterError> {
    let fixture = Fixture::new(
        FixtureHeader::new("instruction", FixtureKind::Instruction),
        FixtureInput::Instruction(InstructionFixture::new(
            RuntimeFixtureConfig::new(7, None, false, false),
            Vec::new(),
            vec![
                AccountSnapshot::new(address(1), 5, address(7), false, 0, vec![1]),
                AccountSnapshot::new(address(2), 6, address(7), false, 0, vec![2]),
            ],
            address(9),
            vec![
                InstructionAccountMeta::new(address(1), true, true),
                InstructionAccountMeta::new(address(2), false, true),
            ],
            vec![1, 2, 3],
        )),
        FixtureExpectations::new(
            ExecutionSnapshot::from_fields(ExecutionSnapshotFields {
                status: ExecutionStatus::Success,
                included: true,
                compute_units_consumed: 12,
                fee: 0,
                logs: Vec::new(),
                return_data: None,
                inner_instructions: Vec::new(),
                post_accounts: vec![AccountSnapshot::new(
                    address(2),
                    3,
                    address(7),
                    false,
                    0,
                    vec![2],
                )],
            }),
            vec![Compare::Status],
        ),
    );

    let exported = FiredancerFixture::try_from(fixture)?;
    let proto = exported.as_proto();

    assert_eq!(
        proto.metadata.as_ref().map(|metadata| metadata.fn_entrypoint.as_str()),
        Some("instruction")
    );

    let input = proto.input.as_ref().expect("input");
    assert_eq!(input.program_id, address_bytes(9));
    assert_eq!(input.accounts.len(), 2);
    assert_eq!(input.instr_accounts.len(), 2);
    assert_eq!(input.instr_accounts[0].index, 0);
    assert!(input.instr_accounts[0].is_signer);
    assert!(input.instr_accounts[0].is_writable);
    assert_eq!(input.instr_accounts[1].index, 1);
    assert!(!input.instr_accounts[1].is_signer);
    assert!(input.instr_accounts[1].is_writable);
    assert_eq!(input.data, vec![1, 2, 3]);
    assert_eq!(input.cu_avail, 12);
    assert_eq!(input.slot_context.as_ref().map(|slot| slot.slot), Some(7));

    let output = proto.output.as_ref().expect("output");
    assert_eq!(output.result, 0);
    assert_eq!(output.custom_err, 0);
    assert_eq!(output.cu_avail, 0);
    assert_eq!(output.modified_accounts.len(), 1);
    assert_eq!(output.modified_accounts[0].address, address_bytes(2));

    Ok(())
}
