use std::{fs, path::Path};
#[cfg(feature = "json-codec")]
use std::{
    fs::File,
    io::{BufReader, BufWriter},
};

use mollusk_svm_fuzz_fixture_firedancer as fd_codec;
use prost::Message;
use solana_address::Address;

pub use self::error::AdapterError;
use crate::{
    AccountCompareScope, AccountSnapshot, Compare, ExecutionSnapshot, ExecutionSnapshotFields,
    ExecutionStatus, FixtureExpectations, FixtureHeader, FixtureInput, FixtureKind,
    InstructionAccountMeta, InstructionFixture, ReturnDataSnapshot, RuntimeFixtureConfig,
};

mod error {
    use thiserror::Error;

    #[derive(Debug, Error)]
    pub enum AdapterError {
        #[error("I/O error: {0}")]
        Io(#[from] std::io::Error),
        #[cfg(feature = "json-codec")]
        #[error("JSON codec error: {0}")]
        Json(#[from] serde_json::Error),
        #[error("protobuf codec decode error: {0}")]
        Decode(#[from] prost::DecodeError),
        #[error("unsupported firedancer fixture format for {path}")]
        UnsupportedFormat { path: String },
        #[error("missing firedancer fixture field {field}")]
        MissingField { field: &'static str },
        #[error("invalid 32-byte address length for {field}: got {actual}")]
        InvalidAddressLength { field: &'static str, actual: usize },
        #[error("instruction account index {index} is out of range for {accounts_len} accounts")]
        InvalidInstructionAccountIndex { index: usize, accounts_len: usize },
        #[error("instruction account {address} is missing from pre_accounts")]
        MissingInstructionAccount { address: String },
        #[error("seed-derived account metadata is not supported in {field}")]
        UnsupportedSeedAddress { field: &'static str },
        #[error("firedancer compute units are inconsistent: before={before}, after={after}")]
        InconsistentComputeUnits { before: u64, after: u64 },
        #[error("hpsvm fixture consumed {consumed} compute units but runtime budget is {budget}")]
        ComputeUnitsExceedBudget { budget: u64, consumed: u64 },
        #[error(
            "firedancer execution status is inconsistent: result={result}, custom_err={custom_err}"
        )]
        InconsistentExecutionStatus { result: i32, custom_err: u32 },
        #[error("execution status {kind} cannot be exported to firedancer status fields")]
        UnsupportedExecutionStatus { kind: String },
        #[error(
            "return data program {program_id} cannot be exported to firedancer fixture for instruction program {instruction_program_id}"
        )]
        UnsupportedReturnDataProgram { program_id: String, instruction_program_id: String },
        #[error("hpsvm fixture kind {kind} cannot be exported to firedancer; expected {expected}")]
        UnsupportedFixtureKind { kind: &'static str, expected: &'static str },
        #[error(
            "exporting hpsvm instruction fixtures requires the initial compute unit budget, which the canonical fixture model does not store"
        )]
        MissingComputeUnitBudget,
    }
}

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
            #[cfg(feature = "json-codec")]
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
            #[cfg(feature = "json-codec")]
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

impl TryFrom<FiredancerFixture> for crate::Fixture {
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
            status: status_from_output(output.result, output.custom_err)?,
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
                )
                .with_compute_unit_limit(input.cu_avail),
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

impl TryFrom<crate::Fixture> for FiredancerFixture {
    type Error = AdapterError;

    fn try_from(value: crate::Fixture) -> Result<Self, Self::Error> {
        match value.input {
            FixtureInput::Instruction(instruction) => {
                let input_cu_avail = instruction
                    .runtime
                    .compute_unit_limit
                    .ok_or(AdapterError::MissingComputeUnitBudget)?;
                let output_cu_avail = input_cu_avail
                    .checked_sub(value.expectations.baseline.compute_units_consumed)
                    .ok_or(AdapterError::ComputeUnitsExceedBudget {
                        budget: input_cu_avail,
                        consumed: value.expectations.baseline.compute_units_consumed,
                    })?;
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
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FiredancerFixtureFormat {
    Binary,
    #[cfg(feature = "json-codec")]
    Json,
}

fn fixture_format_for_path(path: &Path) -> Result<FiredancerFixtureFormat, AdapterError> {
    match path.extension().and_then(|value| value.to_str()) {
        Some("fix") => Ok(FiredancerFixtureFormat::Binary),
        #[cfg(feature = "json-codec")]
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
    let (result, custom_err) = status_to_output(&snapshot.status)?;
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

fn status_to_output(status: &ExecutionStatus) -> Result<(i32, u32), AdapterError> {
    match status {
        ExecutionStatus::Success => Ok((0, 0)),
        ExecutionStatus::Failure { kind, .. } => {
            if let Some(value) = parse_firedancer_result_with_custom_error(kind) {
                Ok(value)
            } else if let Some(value) = parse_firedancer_custom_error(kind) {
                Ok((1, value))
            } else if let Some(value) = parse_firedancer_program_result(kind) {
                Ok((value, 0))
            } else {
                Err(AdapterError::UnsupportedExecutionStatus { kind: kind.clone() })
            }
        }
    }
}

fn parse_firedancer_result_with_custom_error(kind: &str) -> Option<(i32, u32)> {
    let inner =
        kind.strip_prefix("FiredancerProgramResult(").and_then(|value| value.strip_suffix(')'))?;
    let (result, custom_err) = inner.split_once(",CustomError(")?;
    Some((result.parse().ok()?, custom_err.strip_suffix(')')?.parse().ok()?))
}

fn parse_firedancer_custom_error(kind: &str) -> Option<u32> {
    kind.strip_prefix("FiredancerCustomError(")
        .and_then(|value| value.strip_suffix(')'))
        .and_then(|value| value.parse::<u32>().ok())
}

fn parse_firedancer_program_result(kind: &str) -> Option<i32> {
    kind.strip_prefix("FiredancerProgramResult(")
        .and_then(|value| value.strip_suffix(')'))
        .and_then(|value| value.parse::<i32>().ok())
}

fn status_from_output(result: i32, custom_err: u32) -> Result<ExecutionStatus, AdapterError> {
    match (result, custom_err) {
        (0, 0) => Ok(ExecutionStatus::Success),
        (0, custom_err) => Err(AdapterError::InconsistentExecutionStatus { result, custom_err }),
        (result, 0) => Ok(ExecutionStatus::Failure {
            kind: format!("FiredancerProgramResult({result})"),
            message: format!("firedancer program returned status {result}"),
        }),
        (result, custom_err) => Ok(ExecutionStatus::Failure {
            kind: format!("FiredancerProgramResult({result},CustomError({custom_err}))"),
            message: format!(
                "firedancer program returned status {result} with custom error {custom_err}"
            ),
        }),
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

#[cfg(all(test, feature = "instruction-fixture"))]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn address(fill: u8) -> Address {
        Address::new_from_array([fill; 32])
    }

    fn account_snapshot(fill: u8) -> AccountSnapshot {
        AccountSnapshot::new(address(fill), 10, address(7), false, 0, vec![1, 2, 3])
    }

    fn proto_account(fill: u8) -> fd_codec::proto::AcctState {
        fd_codec::proto::AcctState {
            address: vec![fill; 32],
            owner: vec![7; 32],
            lamports: 10,
            data: vec![1, 2, 3],
            executable: false,
            rent_epoch: 0,
            seed_addr: None,
        }
    }

    fn empty_snapshot() -> ExecutionSnapshot {
        ExecutionSnapshot::from_fields(ExecutionSnapshotFields {
            status: ExecutionStatus::Success,
            included: true,
            compute_units_consumed: 0,
            fee: 0,
            logs: Vec::new(),
            return_data: None,
            inner_instructions: Vec::new(),
            post_accounts: Vec::new(),
        })
    }

    // ---------------------------------------------------------------------
    // address decoding
    // ---------------------------------------------------------------------

    #[test]
    fn address_from_bytes_decodes_exactly_32_bytes() {
        let bytes = vec![3u8; 32];
        assert_eq!(address_from_bytes(&bytes, "f").unwrap(), address(3));
    }

    /// Any length other than 32 must be rejected, not silently zero-padded.
    #[test]
    fn address_from_bytes_rejects_wrong_lengths() {
        for len in [0usize, 1, 31, 33, 64] {
            let bytes = vec![3u8; len];
            match address_from_bytes(&bytes, "input.program_id") {
                Err(AdapterError::InvalidAddressLength { field, actual }) => {
                    assert_eq!(field, "input.program_id");
                    assert_eq!(actual, len);
                }
                other => panic!("expected InvalidAddressLength for len {len}, got {other:?}"),
            }
        }
    }

    // Property: a 32-byte payload always decodes and any other length always
    // fails with the observed length.
    proptest! {
        #[test]
        fn address_from_bytes_accepts_exactly_32_bytes(
            bytes in prop::collection::vec(any::<u8>(), 0..64),
        ) {
            match address_from_bytes(&bytes, "f") {
                Ok(decoded) => {
                    let raw = decoded.to_bytes();
                    prop_assert_eq!(bytes.len(), 32);
                    prop_assert_eq!(raw.as_slice(), bytes.as_slice());
                }
                Err(AdapterError::InvalidAddressLength { actual, .. }) => {
                    prop_assert_eq!(actual, bytes.len());
                }
                Err(other) => prop_assert!(false, "unexpected error {other:?}"),
            }
        }
    }

    #[test]
    fn address_to_bytes_round_trips_through_address_from_bytes() {
        let original = address(11);
        assert_eq!(address_from_bytes(&address_to_bytes(original), "f").unwrap(), original);
    }

    // ---------------------------------------------------------------------
    // account snapshots
    // ---------------------------------------------------------------------

    #[test]
    fn account_snapshot_from_proto_copies_every_field() {
        let snapshot = account_snapshot_from_proto(proto_account(4), "input.accounts").unwrap();
        assert_eq!(snapshot.address, address(4));
        assert_eq!(snapshot.owner, address(7));
        assert_eq!(snapshot.lamports, 10);
        assert_eq!(snapshot.data, vec![1, 2, 3]);
        assert!(!snapshot.executable);
        assert_eq!(snapshot.rent_epoch, 0);
    }

    /// Seed-derived accounts cannot be represented in the hpsvm model.
    #[test]
    fn account_snapshot_from_proto_rejects_seed_derived_addresses() {
        let mut account = proto_account(4);
        account.seed_addr = Some(fd_codec::proto::SeedAddress {
            base: vec![1; 32],
            seed: vec![2],
            owner: vec![3; 32],
        });

        match account_snapshot_from_proto(account, "output.modified_accounts") {
            Err(AdapterError::UnsupportedSeedAddress { field }) => {
                assert_eq!(field, "output.modified_accounts");
            }
            other => panic!("expected UnsupportedSeedAddress, got {other:?}"),
        }
    }

    #[test]
    fn account_snapshot_from_proto_rejects_a_short_address() {
        let mut account = proto_account(4);
        account.address = vec![0u8; 8];
        assert!(matches!(
            account_snapshot_from_proto(account, "input.accounts").unwrap_err(),
            AdapterError::InvalidAddressLength { field: "account.address", actual: 8 }
        ));
    }

    #[test]
    fn account_snapshot_to_proto_round_trips_through_from_proto() {
        let original = account_snapshot(5);
        let encoded = account_snapshot_to_proto(&original);

        assert_eq!(encoded.address, vec![5u8; 32]);
        assert_eq!(encoded.owner, vec![7u8; 32]);
        assert_eq!(encoded.seed_addr, None);
        assert_eq!(account_snapshot_from_proto(encoded, "f").unwrap(), original);
    }

    // ---------------------------------------------------------------------
    // instruction accounts
    // ---------------------------------------------------------------------

    #[test]
    fn instruction_account_from_proto_resolves_by_index() {
        let accounts = vec![account_snapshot(1), account_snapshot(2)];
        let account = instruction_account_from_proto(
            &accounts,
            fd_codec::proto::InstrAcct { index: 1, is_writable: true, is_signer: false },
        )
        .unwrap();

        assert_eq!(account.pubkey, address(2));
        assert!(account.is_writable);
        assert!(!account.is_signer);
    }

    #[test]
    fn instruction_account_from_proto_rejects_an_out_of_range_index() {
        let accounts = vec![account_snapshot(1)];
        match instruction_account_from_proto(
            &accounts,
            fd_codec::proto::InstrAcct { index: 5, is_writable: false, is_signer: false },
        ) {
            Err(AdapterError::InvalidInstructionAccountIndex { index, accounts_len }) => {
                assert_eq!(index, 5);
                assert_eq!(accounts_len, 1);
            }
            other => panic!("expected InvalidInstructionAccountIndex, got {other:?}"),
        }
    }

    /// A negative index arrives as a wrapped `u32`; the conversion must fail
    /// rather than index out of bounds.
    #[test]
    fn instruction_account_from_proto_rejects_a_negative_index() {
        let accounts = vec![account_snapshot(1)];
        match instruction_account_from_proto(
            &accounts,
            fd_codec::proto::InstrAcct { index: u32::MAX, is_writable: false, is_signer: false },
        ) {
            Err(AdapterError::InvalidInstructionAccountIndex { index, accounts_len }) => {
                // A negative `i32` wraps to `u32::MAX`, which widens to
                // `usize::MAX` on this target.
                assert_eq!(index, u32::MAX as usize);
                assert_eq!(accounts_len, 1);
            }
            other => panic!("expected InvalidInstructionAccountIndex, got {other:?}"),
        }
    }

    #[test]
    fn instruction_accounts_to_proto_resolves_addresses_to_indices() {
        let accounts = vec![account_snapshot(1), account_snapshot(2)];
        let encoded = instruction_accounts_to_proto(
            &accounts,
            &[
                InstructionAccountMeta::new(address(2), false, true),
                InstructionAccountMeta::new(address(1), true, true),
            ],
        )
        .unwrap();

        assert_eq!(encoded.len(), 2);
        assert_eq!(encoded[0].index, 1);
        assert!(encoded[0].is_writable);
        assert!(!encoded[0].is_signer);
        assert_eq!(encoded[1].index, 0);
        assert!(encoded[1].is_signer);
    }

    /// An instruction account with no matching pre-account cannot be encoded.
    #[test]
    fn instruction_accounts_to_proto_rejects_an_unknown_address() {
        let accounts = vec![account_snapshot(1)];
        match instruction_accounts_to_proto(
            &accounts,
            &[InstructionAccountMeta::new(address(9), false, false)],
        ) {
            Err(AdapterError::MissingInstructionAccount { address: reported }) => {
                assert_eq!(reported, address(9).to_string());
            }
            other => panic!("expected MissingInstructionAccount, got {other:?}"),
        }
    }

    #[test]
    fn instruction_accounts_to_proto_of_no_instruction_accounts_is_empty() {
        assert!(instruction_accounts_to_proto(&[account_snapshot(1)], &[]).unwrap().is_empty());
    }

    // The two account-index conversions must agree in both directions: an
    // in-range index round-trips, and an out-of-range index is rejected.
    proptest! {
        #[test]
        fn instruction_account_index_conversions_agree(
            count in 1usize..8,
            index in 0usize..8,
        ) {
            let accounts: Vec<AccountSnapshot> =
                (0..count).map(|i| account_snapshot(i as u8)).collect();
            let account = fd_codec::proto::InstrAcct {
                index: index as u32,
                is_writable: false,
                is_signer: false,
            };

            if index < count {
                let decoded = instruction_account_from_proto(&accounts, account).unwrap();
                prop_assert_eq!(decoded.pubkey, accounts[index].address);

                let encoded = instruction_accounts_to_proto(&accounts, &[decoded]).unwrap();
                prop_assert_eq!(encoded[0].index, index as u32);
            } else {
                prop_assert!(instruction_account_from_proto(&accounts, account).is_err());
            }
        }
    }

    // ---------------------------------------------------------------------
    // execution status mapping
    // ---------------------------------------------------------------------

    #[test]
    fn status_to_output_maps_success_to_zero() {
        assert_eq!(status_to_output(&ExecutionStatus::Success).unwrap(), (0, 0));
    }

    #[test]
    fn status_to_output_parses_a_bare_program_result() {
        let status = ExecutionStatus::Failure {
            kind: String::from("FiredancerProgramResult(3)"),
            message: String::new(),
        };
        assert_eq!(status_to_output(&status).unwrap(), (3, 0));
    }

    #[test]
    fn status_to_output_parses_a_program_result_with_a_custom_error() {
        let status = ExecutionStatus::Failure {
            kind: String::from("FiredancerProgramResult(7,CustomError(42))"),
            message: String::new(),
        };
        assert_eq!(status_to_output(&status).unwrap(), (7, 42));
    }

    #[test]
    fn status_to_output_parses_a_bare_custom_error() {
        let status = ExecutionStatus::Failure {
            kind: String::from("FiredancerCustomError(11)"),
            message: String::new(),
        };
        assert_eq!(status_to_output(&status).unwrap(), (1, 11));
    }

    #[test]
    fn status_to_output_rejects_an_unparseable_kind() {
        for kind in [
            String::from("InstructionError"),
            String::from("FiredancerProgramResult(x)"),
            String::from("FiredancerProgramResult(1"),
            String::from("FiredancerCustomError(nope)"),
            String::from(""),
        ] {
            let status = ExecutionStatus::Failure { kind: kind.clone(), message: String::new() };
            match status_to_output(&status) {
                Err(AdapterError::UnsupportedExecutionStatus { kind: reported }) => {
                    assert_eq!(reported, kind);
                }
                other => panic!("expected UnsupportedExecutionStatus for {kind:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn status_from_output_maps_zero_zero_to_success() {
        assert_eq!(status_from_output(0, 0).unwrap(), ExecutionStatus::Success);
    }

    /// `(0, custom_err)` means "success but with an error code", which is
    /// contradictory and must be rejected.
    #[test]
    fn status_from_output_rejects_success_with_a_custom_error() {
        match status_from_output(0, 5) {
            Err(AdapterError::InconsistentExecutionStatus { result, custom_err }) => {
                assert_eq!(result, 0);
                assert_eq!(custom_err, 5);
            }
            other => panic!("expected InconsistentExecutionStatus, got {other:?}"),
        }
    }

    #[test]
    fn status_from_output_maps_a_failure_without_a_custom_error() {
        let status = status_from_output(3, 0).unwrap();
        match status {
            ExecutionStatus::Failure { kind, message } => {
                assert_eq!(kind, "FiredancerProgramResult(3)");
                assert_eq!(message, "firedancer program returned status 3");
            }
            other => panic!("expected ExecutionStatus::Failure, got {other:?}"),
        }
    }

    #[test]
    fn status_from_output_maps_a_failure_with_a_custom_error() {
        let status = status_from_output(3, 42).unwrap();
        match status {
            ExecutionStatus::Failure { kind, message } => {
                assert_eq!(kind, "FiredancerProgramResult(3,CustomError(42))");
                assert!(message.contains("42"));
            }
            other => panic!("expected ExecutionStatus::Failure, got {other:?}"),
        }
    }

    // `status_from_output` then `status_to_output` must be lossless for every
    // representable status pair, and `(0, nonzero)` must stay rejected.
    proptest! {
        #[test]
        fn status_conversion_round_trips(
            result in -32i32..32,
            custom_err in 0u32..256,
        ) {
            match status_from_output(result, custom_err) {
                // `(0, nonzero)` is the one inconsistent pair, and it is rejected.
                Err(AdapterError::InconsistentExecutionStatus {
                    result: reported,
                    custom_err: reported_err,
                }) => {
                    prop_assert_eq!(reported, 0);
                    prop_assert_eq!(reported_err, custom_err);
                    prop_assert_ne!(custom_err, 0);
                }
                Err(other) => prop_assert!(false, "unexpected error {other:?}"),
                Ok(ExecutionStatus::Success) => {
                    prop_assert_eq!(result, 0);
                    prop_assert_eq!(custom_err, 0);
                    prop_assert_eq!(status_to_output(&ExecutionStatus::Success).unwrap(), (0, 0));
                }
                Ok(status) => {
                    let (round_tripped_result, round_tripped_custom_err) =
                        status_to_output(&status).expect("a produced status must be mappable back");
                    prop_assert_eq!(round_tripped_result, result);
                    prop_assert_eq!(round_tripped_custom_err, custom_err);
                }
            }
        }
    }

    #[test]
    fn return_data_from_output_is_none_for_empty_payloads() {
        assert!(return_data_from_output(address(9), Vec::new()).is_none());
    }

    #[test]
    fn return_data_from_output_keeps_non_empty_payloads_with_the_instruction_program() {
        assert_eq!(
            return_data_from_output(address(9), vec![1, 2]),
            Some(ReturnDataSnapshot::new(address(9), vec![1, 2]))
        );
    }

    // ---------------------------------------------------------------------
    // snapshot export
    // ---------------------------------------------------------------------

    #[test]
    fn snapshot_to_proto_effects_encodes_a_success() {
        let snapshot = empty_snapshot();
        let effects = snapshot_to_proto_effects(&snapshot, address(9), 400).unwrap();

        assert_eq!(effects.result, 0);
        assert_eq!(effects.custom_err, 0);
        assert_eq!(effects.cu_avail, 400);
        assert!(effects.modified_accounts.is_empty());
        assert!(effects.return_data.is_empty());
    }

    #[test]
    fn snapshot_to_proto_effects_exports_modified_accounts() {
        let snapshot =
            ExecutionSnapshot { post_accounts: vec![account_snapshot(2)], ..empty_snapshot() };
        let effects = snapshot_to_proto_effects(&snapshot, address(9), 100).unwrap();

        assert_eq!(effects.modified_accounts.len(), 1);
        assert_eq!(effects.modified_accounts[0].address, vec![2u8; 32]);
    }

    #[test]
    fn snapshot_to_proto_effects_carries_return_data_for_the_same_program() {
        let snapshot = ExecutionSnapshot {
            return_data: Some(ReturnDataSnapshot::new(address(9), vec![4, 2])),
            ..empty_snapshot()
        };
        let effects = snapshot_to_proto_effects(&snapshot, address(9), 100).unwrap();
        assert_eq!(effects.return_data, vec![4, 2]);
    }

    /// Firedancer stores return data without a program id, so a return-data
    /// program other than the instruction program cannot be represented.
    #[test]
    fn snapshot_to_proto_effects_rejects_foreign_return_data() {
        let snapshot = ExecutionSnapshot {
            return_data: Some(ReturnDataSnapshot::new(address(8), vec![4, 2])),
            ..empty_snapshot()
        };
        match snapshot_to_proto_effects(&snapshot, address(9), 100) {
            Err(AdapterError::UnsupportedReturnDataProgram {
                program_id,
                instruction_program_id,
            }) => {
                assert_eq!(program_id, address(8).to_string());
                assert_eq!(instruction_program_id, address(9).to_string());
            }
            other => panic!("expected UnsupportedReturnDataProgram, got {other:?}"),
        }
    }

    #[test]
    fn snapshot_to_proto_effects_rejects_an_unmappable_failure() {
        let snapshot = ExecutionSnapshot {
            status: ExecutionStatus::Failure {
                kind: String::from("InstructionError"),
                message: String::new(),
            },
            ..empty_snapshot()
        };
        assert!(matches!(
            snapshot_to_proto_effects(&snapshot, address(9), 100).unwrap_err(),
            AdapterError::UnsupportedExecutionStatus { .. }
        ));
    }

    // ---------------------------------------------------------------------
    // compare selection
    // ---------------------------------------------------------------------

    /// With no modified accounts there is nothing address-scoped to compare, so
    /// the account comparison must be omitted.
    #[test]
    fn compares_for_no_post_accounts_omits_the_account_comparison() {
        assert_eq!(
            compares_for_post_accounts(&[]),
            vec![Compare::Status, Compare::Included, Compare::ComputeUnits, Compare::ReturnData]
        );
    }

    #[test]
    fn compares_for_post_accounts_scopes_to_the_modified_addresses() {
        let compares = compares_for_post_accounts(&[account_snapshot(1), account_snapshot(2)]);
        assert_eq!(compares.len(), 5);
        assert_eq!(
            compares[4],
            Compare::Accounts(AccountCompareScope::Only(vec![address(1), address(2)]))
        );
    }

    /// A repeated address must be de-duplicated so the scope list stays minimal.
    #[test]
    fn compares_for_post_accounts_de_duplicates_addresses() {
        let compares = compares_for_post_accounts(&[
            account_snapshot(1),
            account_snapshot(1),
            account_snapshot(2),
        ]);
        assert_eq!(
            compares[4],
            Compare::Accounts(AccountCompareScope::Only(vec![address(1), address(2)]))
        );
    }

    // ---------------------------------------------------------------------
    // path dispatch
    // ---------------------------------------------------------------------

    #[test]
    fn fixture_format_for_path_accepts_fix_and_json() {
        assert_eq!(
            fixture_format_for_path(Path::new("a.fix")).unwrap(),
            FiredancerFixtureFormat::Binary
        );
        assert_eq!(
            fixture_format_for_path(Path::new("a.json")).unwrap(),
            FiredancerFixtureFormat::Json
        );
    }

    #[test]
    fn fixture_format_for_path_rejects_other_extensions() {
        for name in ["a.bin", "a.txt", "a", "a.JSON"] {
            let error = fixture_format_for_path(Path::new(name))
                .expect_err("unknown extension should fail");
            assert!(
                matches!(error, AdapterError::UnsupportedFormat { .. }),
                "expected UnsupportedFormat for {name}, got {error:?}"
            );
        }
    }

    #[test]
    fn save_and_load_round_trip_the_binary_format() {
        let path = std::env::temp_dir().join(format!("hpsvm-fd-{}.fix", address(1)));
        let fixture = FiredancerFixture::from_proto(fd_codec::proto::InstrFixture {
            metadata: Some(fd_codec::proto::FixtureMetadata {
                fn_entrypoint: String::from("entrypoint"),
            }),
            input: None,
            output: None,
        });

        fixture.save(&path).expect("save should succeed");
        assert_eq!(FiredancerFixture::load(&path).expect("load should succeed"), fixture);

        std::fs::remove_file(path).ok();
    }

    #[cfg(feature = "json-codec")]
    #[test]
    fn save_and_load_round_trip_the_json_format() {
        let path = std::env::temp_dir().join(format!("hpsvm-fd-{}.json", address(2)));
        let fixture = FiredancerFixture::from_proto(fd_codec::proto::InstrFixture {
            metadata: Some(fd_codec::proto::FixtureMetadata {
                fn_entrypoint: String::from("entrypoint"),
            }),
            input: None,
            output: None,
        });

        fixture.save(&path).expect("json save should succeed");
        assert_eq!(FiredancerFixture::load(&path).expect("json load should succeed"), fixture);

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn load_reports_a_missing_file() {
        let path = std::env::temp_dir().join(format!("hpsvm-fd-missing-{}.fix", address(3)));
        assert!(matches!(FiredancerFixture::load(&path).unwrap_err(), AdapterError::Io(_)));
    }

    /// Random bytes must fail protobuf decoding rather than panicking.
    #[test]
    fn load_rejects_undecodable_protobuf_bytes() {
        let path = std::env::temp_dir().join(format!("hpsvm-fd-garbage-{}.fix", address(4)));
        std::fs::write(&path, [0xffu8; 64]).unwrap();

        assert!(matches!(FiredancerFixture::load(&path).unwrap_err(), AdapterError::Decode(_)));

        std::fs::remove_file(path).ok();
    }

    #[cfg(feature = "json-codec")]
    #[test]
    fn load_rejects_malformed_json() {
        let path = std::env::temp_dir().join(format!("hpsvm-fd-bad-json-{}.json", address(5)));
        std::fs::write(&path, b"{ not json").unwrap();

        assert!(matches!(FiredancerFixture::load(&path).unwrap_err(), AdapterError::Json(_)));

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn save_rejects_an_unsupported_extension() {
        let fixture = FiredancerFixture::from_proto(fd_codec::proto::InstrFixture {
            metadata: None,
            input: None,
            output: None,
        });
        assert!(matches!(
            fixture.save("/tmp/nope.txt").unwrap_err(),
            AdapterError::UnsupportedFormat { .. }
        ));
    }

    #[test]
    fn as_proto_into_proto_and_from_proto_agree() {
        let proto = fd_codec::proto::InstrFixture {
            metadata: Some(fd_codec::proto::FixtureMetadata {
                fn_entrypoint: String::from("entrypoint"),
            }),
            input: None,
            output: None,
        };
        let fixture = FiredancerFixture::from_proto(proto.clone());

        assert_eq!(fixture.as_proto(), &proto);
        assert_eq!(fixture.clone().into_proto(), proto);
    }

    #[test]
    fn try_from_fixture_rejects_a_transaction_fixture() {
        let fixture = crate::Fixture::new(
            FixtureHeader::new("tx", FixtureKind::Transaction),
            FixtureInput::Transaction(crate::TransactionFixture::new(
                RuntimeFixtureConfig::new(0, None, false, false),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )),
            FixtureExpectations::new(empty_snapshot(), Vec::new()),
        );

        match FiredancerFixture::try_from(fixture) {
            Err(AdapterError::UnsupportedFixtureKind { kind, expected }) => {
                assert_eq!(kind, "transaction");
                assert_eq!(expected, "instruction");
            }
            other => panic!("expected UnsupportedFixtureKind, got {other:?}"),
        }
    }

    /// Without a compute-unit budget there is no way to express the fixture's
    /// `cu_avail`, so the export must be refused.
    #[test]
    fn try_from_fixture_requires_a_compute_unit_budget() {
        let fixture = crate::Fixture::new(
            FixtureHeader::new("ix", FixtureKind::Instruction),
            FixtureInput::Instruction(InstructionFixture::new(
                RuntimeFixtureConfig::new(0, None, false, false),
                Vec::new(),
                Vec::new(),
                address(9),
                Vec::new(),
                Vec::new(),
            )),
            FixtureExpectations::new(empty_snapshot(), Vec::new()),
        );

        assert!(matches!(
            FiredancerFixture::try_from(fixture).unwrap_err(),
            AdapterError::MissingComputeUnitBudget
        ));
    }

    /// A baseline consuming more compute units than the budget cannot be
    /// encoded, because `cu_avail` would underflow.
    #[test]
    fn try_from_fixture_rejects_a_baseline_over_budget() {
        let fixture = crate::Fixture::new(
            FixtureHeader::new("ix", FixtureKind::Instruction),
            FixtureInput::Instruction(InstructionFixture::new(
                RuntimeFixtureConfig::new(0, None, false, false).with_compute_unit_limit(100),
                Vec::new(),
                Vec::new(),
                address(9),
                Vec::new(),
                Vec::new(),
            )),
            FixtureExpectations::new(
                ExecutionSnapshot { compute_units_consumed: 200, ..empty_snapshot() },
                Vec::new(),
            ),
        );

        match FiredancerFixture::try_from(fixture) {
            Err(AdapterError::ComputeUnitsExceedBudget { budget, consumed }) => {
                assert_eq!(budget, 100);
                assert_eq!(consumed, 200);
            }
            other => panic!("expected ComputeUnitsExceedBudget, got {other:?}"),
        }
    }

    /// A budget consumed exactly is legal and must leave `cu_avail == 0`.
    #[test]
    fn try_from_fixture_accepts_a_baseline_consuming_the_whole_budget() {
        let fixture = crate::Fixture::new(
            FixtureHeader::new("ix", FixtureKind::Instruction),
            FixtureInput::Instruction(InstructionFixture::new(
                RuntimeFixtureConfig::new(0, None, false, false).with_compute_unit_limit(100),
                Vec::new(),
                Vec::new(),
                address(9),
                Vec::new(),
                Vec::new(),
            )),
            FixtureExpectations::new(
                ExecutionSnapshot { compute_units_consumed: 100, ..empty_snapshot() },
                Vec::new(),
            ),
        );

        let exported =
            FiredancerFixture::try_from(fixture).expect("exactly-on-budget should export");
        let proto = exported.into_proto();
        assert_eq!(proto.output.expect("output").cu_avail, 0);
    }

    /// The proto -> model import must reject a fixture with no `input`.
    #[test]
    fn try_from_proto_requires_an_input() {
        let proto = fd_codec::proto::InstrFixture {
            metadata: None,
            input: None,
            output: Some(fd_codec::proto::InstrEffects::default()),
        };
        match crate::Fixture::try_from(FiredancerFixture::from_proto(proto)) {
            Err(AdapterError::MissingField { field }) => assert_eq!(field, "input"),
            other => panic!("expected MissingField, got {other:?}"),
        }
    }

    /// The proto -> model import must reject a fixture with no `output`.
    #[test]
    fn try_from_proto_requires_an_output() {
        let proto = fd_codec::proto::InstrFixture {
            metadata: None,
            input: Some(fd_codec::proto::InstrContext::default()),
            output: None,
        };
        match crate::Fixture::try_from(FiredancerFixture::from_proto(proto)) {
            Err(AdapterError::MissingField { field }) => assert_eq!(field, "output"),
            other => panic!("expected MissingField, got {other:?}"),
        }
    }

    /// `output.cu_avail > input.cu_avail` cannot happen in practice, so the
    /// import must reject the underflow instead of wrapping.
    #[test]
    fn try_from_proto_rejects_inconsistent_compute_units() {
        let proto = fd_codec::proto::InstrFixture {
            metadata: None,
            input: Some(fd_codec::proto::InstrContext {
                program_id: vec![0; 32],
                cu_avail: 100,
                ..Default::default()
            }),
            output: Some(fd_codec::proto::InstrEffects {
                result: 0,
                custom_err: 0,
                cu_avail: 200,
                ..Default::default()
            }),
        };
        match crate::Fixture::try_from(FiredancerFixture::from_proto(proto)) {
            Err(AdapterError::InconsistentComputeUnits { before, after }) => {
                assert_eq!(before, 100);
                assert_eq!(after, 200);
            }
            other => panic!("expected InconsistentComputeUnits, got {other:?}"),
        }
    }

    /// The import must reject a program id that is not 32 bytes.
    #[test]
    fn try_from_proto_rejects_a_short_program_id() {
        let proto = fd_codec::proto::InstrFixture {
            metadata: None,
            input: Some(fd_codec::proto::InstrContext {
                program_id: vec![0; 4],
                cu_avail: 100,
                ..Default::default()
            }),
            output: Some(fd_codec::proto::InstrEffects {
                result: 0,
                custom_err: 0,
                cu_avail: 0,
                ..Default::default()
            }),
        };
        assert!(matches!(
            crate::Fixture::try_from(FiredancerFixture::from_proto(proto)).unwrap_err(),
            AdapterError::InvalidAddressLength { field: "input.program_id", actual: 4 }
        ));
    }

    /// A failing fixture round-trips through the status mapping with its
    /// pre/post accounts preserved.
    #[test]
    fn try_from_proto_preserves_a_failing_status() {
        let proto = fd_codec::proto::InstrFixture {
            metadata: None,
            input: Some(fd_codec::proto::InstrContext {
                program_id: vec![9; 32],
                accounts: vec![proto_account(1)],
                instr_accounts: vec![fd_codec::proto::InstrAcct {
                    index: 0,
                    is_writable: true,
                    is_signer: false,
                }],
                cu_avail: 500,
                ..Default::default()
            }),
            output: Some(fd_codec::proto::InstrEffects {
                result: 7,
                custom_err: 42,
                modified_accounts: vec![proto_account(1)],
                cu_avail: 420,
                return_data: Vec::new(),
            }),
        };

        let fixture = crate::Fixture::try_from(FiredancerFixture::from_proto(proto))
            .expect("failing fixture should import");

        assert!(matches!(fixture.expectations.baseline.status, ExecutionStatus::Failure { .. }));
        assert_eq!(fixture.expectations.baseline.compute_units_consumed, 80);
        assert!(fixture.expectations.baseline.included);
    }

    /// Missing metadata falls back to a name derived from the program id.
    #[test]
    fn try_from_proto_falls_back_to_a_generated_header_name() {
        let proto = fd_codec::proto::InstrFixture {
            metadata: None,
            input: Some(fd_codec::proto::InstrContext {
                program_id: vec![9; 32],
                cu_avail: 100,
                ..Default::default()
            }),
            output: Some(fd_codec::proto::InstrEffects {
                result: 0,
                custom_err: 0,
                cu_avail: 0,
                ..Default::default()
            }),
        };

        let fixture = crate::Fixture::try_from(FiredancerFixture::from_proto(proto)).unwrap();
        assert_eq!(fixture.header.name, format!("firedancer-{}", address(9)));
        assert_eq!(fixture.header.source, Some(String::from(FIREDANCER_SOURCE)));
        assert_eq!(fixture.header.tags, vec![String::from(FIREDANCER_TAG)]);
        assert_eq!(fixture.header.kind, FixtureKind::Instruction);
    }

    /// An empty `fn_entrypoint` also falls back to the generated name.
    #[test]
    fn try_from_proto_falls_back_when_the_entrypoint_name_is_empty() {
        let proto = fd_codec::proto::InstrFixture {
            metadata: Some(fd_codec::proto::FixtureMetadata { fn_entrypoint: String::new() }),
            input: Some(fd_codec::proto::InstrContext {
                program_id: vec![9; 32],
                cu_avail: 100,
                ..Default::default()
            }),
            output: Some(fd_codec::proto::InstrEffects {
                result: 0,
                custom_err: 0,
                cu_avail: 0,
                ..Default::default()
            }),
        };

        let fixture = crate::Fixture::try_from(FiredancerFixture::from_proto(proto)).unwrap();
        assert_eq!(fixture.header.name, format!("firedancer-{}", address(9)));
    }

    /// The imported fixture must carry the compute-unit budget as a runtime
    /// limit, otherwise the runner would use the VM default.
    #[test]
    fn try_from_proto_records_the_compute_unit_budget_as_a_runtime_limit() {
        let proto = fd_codec::proto::InstrFixture {
            metadata: None,
            input: Some(fd_codec::proto::InstrContext {
                program_id: vec![9; 32],
                cu_avail: 1234,
                slot_context: Some(fd_codec::proto::SlotContext { slot: 99 }),
                ..Default::default()
            }),
            output: Some(fd_codec::proto::InstrEffects {
                result: 0,
                custom_err: 0,
                cu_avail: 1000,
                ..Default::default()
            }),
        };

        let fixture = crate::Fixture::try_from(FiredancerFixture::from_proto(proto)).unwrap();
        let crate::FixtureInput::Instruction(instruction) = &fixture.input else {
            panic!("expected an instruction fixture");
        };
        assert_eq!(instruction.runtime.compute_unit_limit, Some(1234));
        assert_eq!(instruction.runtime.slot, 99);
        // No modified accounts means no address-scoped account comparison.
        assert!(
            !fixture
                .expectations
                .compares
                .iter()
                .any(|compare| matches!(compare, Compare::Accounts(_)))
        );
    }

    /// A missing slot context must default to slot 0 rather than failing.
    #[test]
    fn try_from_proto_defaults_the_slot_to_zero() {
        let proto = fd_codec::proto::InstrFixture {
            metadata: None,
            input: Some(fd_codec::proto::InstrContext {
                program_id: vec![9; 32],
                cu_avail: 100,
                slot_context: None,
                ..Default::default()
            }),
            output: Some(fd_codec::proto::InstrEffects {
                result: 0,
                custom_err: 0,
                cu_avail: 0,
                ..Default::default()
            }),
        };

        let fixture = crate::Fixture::try_from(FiredancerFixture::from_proto(proto)).unwrap();
        let crate::FixtureInput::Instruction(instruction) = &fixture.input else {
            panic!("expected an instruction fixture");
        };
        assert_eq!(instruction.runtime.slot, 0);
    }
}
