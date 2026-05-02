#![allow(missing_debug_implementations, missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

mod error;

use std::{
    fs::{self, File},
    io::{BufReader, BufWriter},
    path::Path,
};

use hpsvm_fixture::{
    AccountCompareScope, AccountSnapshot, Compare, ExecutionSnapshot, ExecutionSnapshotFields,
    ExecutionStatus, FixtureExpectations, FixtureHeader, FixtureInput, FixtureKind,
    InstructionAccountMeta, InstructionFixture, ReturnDataSnapshot, RuntimeFixtureConfig,
};
use mollusk_svm_fuzz_fixture_firedancer as fd_codec;
use prost::Message;
use solana_address::Address;

pub use crate::error::AdapterError;

const FIREDANCER_SOURCE: &str = "firedancer";
const FIREDANCER_TAG: &str = "external:firedancer";

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FiredancerFixture {
    inner: fd_codec::proto::InstrFixture,
}

impl FiredancerFixture {
    pub fn from_proto(inner: fd_codec::proto::InstrFixture) -> Self {
        Self { inner }
    }

    pub fn as_proto(&self) -> &fd_codec::proto::InstrFixture {
        &self.inner
    }

    pub fn into_proto(self) -> fd_codec::proto::InstrFixture {
        self.inner
    }

    #[must_use]
    pub fn to_model(&self) -> fd_codec::Fixture {
        self.inner.clone().into()
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, AdapterError> {
        let path = path.as_ref();
        match fixture_format_for_path(path)? {
            FiredancerFixtureFormat::Binary => {
                let bytes = fs::read(path)?;
                let inner = fd_codec::proto::InstrFixture::decode(bytes.as_slice())?;
                Ok(Self { inner })
            }
            FiredancerFixtureFormat::Json => {
                let reader = BufReader::new(File::open(path)?);
                let inner = serde_json::from_reader(reader)?;
                Ok(Self { inner })
            }
        }
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), AdapterError> {
        let path = path.as_ref();
        match fixture_format_for_path(path)? {
            FiredancerFixtureFormat::Binary => fs::write(path, self.inner.encode_to_vec())?,
            FiredancerFixtureFormat::Json => {
                let writer = BufWriter::new(File::create(path)?);
                serde_json::to_writer_pretty(writer, &self.inner)?;
            }
        }
        Ok(())
    }
}

impl From<fd_codec::Fixture> for FiredancerFixture {
    fn from(value: fd_codec::Fixture) -> Self {
        Self { inner: value.into() }
    }
}

impl From<fd_codec::proto::InstrFixture> for FiredancerFixture {
    fn from(value: fd_codec::proto::InstrFixture) -> Self {
        Self::from_proto(value)
    }
}

impl From<FiredancerFixture> for fd_codec::proto::InstrFixture {
    fn from(value: FiredancerFixture) -> Self {
        value.inner
    }
}

impl TryFrom<FiredancerFixture> for hpsvm_fixture::Fixture {
    type Error = AdapterError;

    fn try_from(value: FiredancerFixture) -> Result<Self, Self::Error> {
        let fd_codec::proto::InstrFixture { metadata, input, output } = value.inner;
        let input = input.ok_or(AdapterError::MissingField { field: "input" })?;
        let output = output.ok_or(AdapterError::MissingField { field: "output" })?;

        let pre_accounts = input
            .accounts
            .into_iter()
            .map(|account| account_snapshot_from_proto(account, "input.accounts"))
            .collect::<Result<Vec<_>, _>>()?;

        let instruction_accounts = input
            .instr_accounts
            .into_iter()
            .map(|account| instruction_account_from_proto(&pre_accounts, account))
            .collect::<Result<Vec<_>, _>>()?;

        let post_accounts = output
            .modified_accounts
            .into_iter()
            .map(|account| account_snapshot_from_proto(account, "output.modified_accounts"))
            .collect::<Result<Vec<_>, _>>()?;

        let program_id = address_from_bytes(&input.program_id, "input.program_id")?;
        let compute_units_consumed = input.cu_avail.checked_sub(output.cu_avail).ok_or(
            AdapterError::InconsistentComputeUnits {
                before: input.cu_avail,
                after: output.cu_avail,
            },
        )?;
        let baseline = ExecutionSnapshot::from_fields(ExecutionSnapshotFields {
            status: status_from_output(output.result, output.custom_err),
            included: true,
            compute_units_consumed,
            fee: 0,
            logs: Vec::new(),
            return_data: return_data_from_output(program_id, output.return_data),
            inner_instructions: Vec::new(),
            post_accounts: post_accounts.clone(),
        });

        let header = FixtureHeader::new(
            metadata
                .as_ref()
                .map(|value| value.fn_entrypoint.as_str())
                .filter(|value| !value.is_empty())
                .map_or_else(|| format!("firedancer-{program_id}"), str::to_owned),
            FixtureKind::Instruction,
        )
        .source(FIREDANCER_SOURCE)
        .tag(FIREDANCER_TAG);

        Ok(Self::new(
            header,
            FixtureInput::Instruction(InstructionFixture::new(
                RuntimeFixtureConfig::new(
                    input.slot_context.map_or(0, |slot| slot.slot),
                    None,
                    false,
                    false,
                ),
                Vec::new(),
                pre_accounts,
                program_id,
                instruction_accounts,
                input.data,
            )),
            FixtureExpectations::new(baseline, compares_for_post_accounts(&post_accounts)),
        ))
    }
}

impl TryFrom<hpsvm_fixture::Fixture> for FiredancerFixture {
    type Error = AdapterError;

    fn try_from(value: hpsvm_fixture::Fixture) -> Result<Self, Self::Error> {
        match value.input {
            FixtureInput::Instruction(instruction) => {
                let output_cu_avail = 0;
                let input_cu_avail =
                    value.expectations.baseline.compute_units_consumed + output_cu_avail;
                let input = fd_codec::proto::InstrContext {
                    program_id: address_to_bytes(instruction.program_id),
                    accounts: instruction
                        .pre_accounts
                        .iter()
                        .map(account_snapshot_to_proto)
                        .collect(),
                    instr_accounts: instruction_accounts_to_proto(
                        &instruction.pre_accounts,
                        &instruction.accounts,
                    )?,
                    data: instruction.data,
                    cu_avail: input_cu_avail,
                    slot_context: Some(fd_codec::proto::SlotContext {
                        slot: instruction.runtime.slot,
                    }),
                    epoch_context: None,
                };
                let output = snapshot_to_proto_effects(
                    &value.expectations.baseline,
                    instruction.program_id,
                    output_cu_avail,
                )?;

                Ok(Self::from_proto(fd_codec::proto::InstrFixture {
                    metadata: Some(fd_codec::proto::FixtureMetadata {
                        fn_entrypoint: value.header.name,
                    }),
                    input: Some(input),
                    output: Some(output),
                }))
            }
            FixtureInput::Transaction(_) => Err(AdapterError::UnsupportedFixtureKind {
                kind: "transaction",
                expected: "instruction",
            }),
            _ => Err(AdapterError::UnsupportedFixtureKind {
                kind: "unknown",
                expected: "instruction",
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FiredancerFixtureFormat {
    Binary,
    Json,
}

fn fixture_format_for_path(path: &Path) -> Result<FiredancerFixtureFormat, AdapterError> {
    match path.extension().and_then(|value| value.to_str()) {
        Some("fix") => Ok(FiredancerFixtureFormat::Binary),
        Some("json") => Ok(FiredancerFixtureFormat::Json),
        _ => Err(AdapterError::UnsupportedFormat { path: path.display().to_string() }),
    }
}

fn address_from_bytes(bytes: &[u8], field: &'static str) -> Result<Address, AdapterError> {
    let array: [u8; 32] = bytes
        .try_into()
        .map_err(|_| AdapterError::InvalidAddressLength { field, actual: bytes.len() })?;
    Ok(Address::new_from_array(array))
}

fn account_snapshot_from_proto(
    account: fd_codec::proto::AcctState,
    field: &'static str,
) -> Result<AccountSnapshot, AdapterError> {
    if account.seed_addr.is_some() {
        return Err(AdapterError::UnsupportedSeedAddress { field });
    }

    Ok(AccountSnapshot::new(
        address_from_bytes(&account.address, "account.address")?,
        account.lamports,
        address_from_bytes(&account.owner, "account.owner")?,
        account.executable,
        account.rent_epoch,
        account.data,
    ))
}

fn account_snapshot_to_proto(account: &AccountSnapshot) -> fd_codec::proto::AcctState {
    fd_codec::proto::AcctState {
        address: address_to_bytes(account.address),
        lamports: account.lamports,
        data: account.data.clone(),
        executable: account.executable,
        rent_epoch: account.rent_epoch,
        owner: address_to_bytes(account.owner),
        seed_addr: None,
    }
}

fn address_to_bytes(address: Address) -> Vec<u8> {
    address.to_bytes().to_vec()
}

fn instruction_account_from_proto(
    accounts: &[AccountSnapshot],
    account: fd_codec::proto::InstrAcct,
) -> Result<InstructionAccountMeta, AdapterError> {
    let index = usize::try_from(account.index).map_err(|_| {
        AdapterError::InvalidInstructionAccountIndex {
            index: usize::MAX,
            accounts_len: accounts.len(),
        }
    })?;
    let address = accounts
        .get(index)
        .ok_or(AdapterError::InvalidInstructionAccountIndex {
            index,
            accounts_len: accounts.len(),
        })?
        .address;
    Ok(InstructionAccountMeta::new(address, account.is_signer, account.is_writable))
}

fn instruction_accounts_to_proto(
    accounts: &[AccountSnapshot],
    instruction_accounts: &[InstructionAccountMeta],
) -> Result<Vec<fd_codec::proto::InstrAcct>, AdapterError> {
    instruction_accounts
        .iter()
        .map(|account| {
            let index = accounts
                .iter()
                .position(|candidate| candidate.address == account.pubkey)
                .ok_or_else(|| AdapterError::MissingInstructionAccount {
                address: account.pubkey.to_string(),
            })?;
            Ok(fd_codec::proto::InstrAcct {
                index: u32::try_from(index).map_err(|_| {
                    AdapterError::InvalidInstructionAccountIndex {
                        index,
                        accounts_len: accounts.len(),
                    }
                })?,
                is_writable: account.is_writable,
                is_signer: account.is_signer,
            })
        })
        .collect()
}

fn snapshot_to_proto_effects(
    snapshot: &ExecutionSnapshot,
    instruction_program_id: Address,
    cu_avail: u64,
) -> Result<fd_codec::proto::InstrEffects, AdapterError> {
    let (result, custom_err) = status_to_output(&snapshot.status);
    let return_data = snapshot
        .return_data
        .as_ref()
        .map(|return_data| {
            if return_data.program_id == instruction_program_id {
                Ok(return_data.data.clone())
            } else {
                Err(AdapterError::UnsupportedReturnDataProgram {
                    program_id: return_data.program_id.to_string(),
                    instruction_program_id: instruction_program_id.to_string(),
                })
            }
        })
        .transpose()?
        .unwrap_or_default();

    Ok(fd_codec::proto::InstrEffects {
        result,
        custom_err,
        modified_accounts: snapshot.post_accounts.iter().map(account_snapshot_to_proto).collect(),
        cu_avail,
        return_data,
    })
}

fn status_to_output(status: &ExecutionStatus) -> (i32, u32) {
    match status {
        ExecutionStatus::Success => (0, 0),
        ExecutionStatus::Failure { kind, .. } => {
            if let Some(value) = kind
                .strip_prefix("FiredancerCustomError(")
                .and_then(|value| value.strip_suffix(')'))
                .and_then(|value| value.parse::<u32>().ok())
            {
                (1, value)
            } else if let Some(value) = kind
                .strip_prefix("FiredancerProgramResult(")
                .and_then(|value| value.strip_suffix(')'))
                .and_then(|value| value.parse::<i32>().ok())
            {
                (value, 0)
            } else {
                (1, 0)
            }
        }
        _ => (1, 0),
    }
}

fn status_from_output(result: i32, custom_err: u32) -> ExecutionStatus {
    if result == 0 {
        ExecutionStatus::Success
    } else if custom_err == 0 {
        ExecutionStatus::Failure {
            kind: format!("FiredancerProgramResult({result})"),
            message: format!("firedancer program returned status {result}"),
        }
    } else {
        ExecutionStatus::Failure {
            kind: format!("FiredancerCustomError({custom_err})"),
            message: format!("firedancer program returned custom error {custom_err}"),
        }
    }
}

fn return_data_from_output(
    program_id: Address,
    return_data: Vec<u8>,
) -> Option<ReturnDataSnapshot> {
    if return_data.is_empty() {
        None
    } else {
        Some(ReturnDataSnapshot::new(program_id, return_data))
    }
}

fn compares_for_post_accounts(post_accounts: &[AccountSnapshot]) -> Vec<Compare> {
    let mut compares =
        vec![Compare::Status, Compare::Included, Compare::ComputeUnits, Compare::ReturnData];

    if !post_accounts.is_empty() {
        let mut addresses = Vec::with_capacity(post_accounts.len());
        for account in post_accounts {
            if !addresses.contains(&account.address) {
                addresses.push(account.address);
            }
        }
        compares.push(Compare::Accounts(AccountCompareScope::Only(addresses)));
    }

    compares
}
