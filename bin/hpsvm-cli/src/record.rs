//! Fixture authoring: turn a signed transaction plus its pre-state into a
//! replayable fixture with a real recorded baseline.

use std::path::{Path, PathBuf};

use hpsvm::HPSVM;
use hpsvm_fixture::{
    AccountSnapshot, CaptureBuilder, Compare, ExecutionSnapshot, FixtureError, FixtureFormat,
    ProgramBinding, RuntimeFixtureConfig,
};
use serde::Deserialize;
use solana_account::Account;
use solana_address::Address;
use solana_sdk_ids::bpf_loader_upgradeable;
use solana_transaction::versioned::VersionedTransaction;

use crate::{TransactionEncodingArg, error::CliError, program_map::parse_program_map};

/// Everything needed to record one fixture, as parsed from the CLI.
pub(crate) struct RecordRequest {
    pub(crate) transaction: PathBuf,
    pub(crate) transaction_encoding: TransactionEncodingArg,
    pub(crate) accounts: PathBuf,
    pub(crate) output: PathBuf,
    pub(crate) name: Option<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) source: Option<String>,
    pub(crate) programs: Vec<String>,
    pub(crate) loader: Option<Address>,
    pub(crate) slot: Option<u64>,
    pub(crate) sigverify: bool,
    pub(crate) log_bytes_limit: Option<usize>,
    pub(crate) compute_unit_limit: Option<u64>,
    pub(crate) ignore_compute_units: bool,
}

/// One pre-execution account as read from the `--accounts` JSON file.
///
/// The shape deliberately matches `AccountSnapshot` inside a fixture's
/// `input.pre_accounts`, so an existing fixture's pre-accounts can be copied
/// straight into a recording input file. Addresses accept either a base58
/// string or the 32-byte array that `Address` uses when it is serialized
/// without a string representation, which is what a fixture file contains.
#[derive(Debug, Deserialize)]
struct RecordAccount {
    #[serde(deserialize_with = "address_from_repr")]
    address: Address,
    lamports: u64,
    #[serde(deserialize_with = "address_from_repr")]
    owner: Address,
    #[serde(default)]
    executable: bool,
    #[serde(default)]
    rent_epoch: u64,
    #[serde(default)]
    data: Vec<u8>,
}

fn address_from_repr<'de, D>(deserializer: D) -> Result<Address, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserializer.deserialize_any(AddressVisitor)
}

/// Accepts a base58 string or a 32-byte array, reporting a precise error for
/// each case instead of the generic "matched no variant" message an untagged
/// enum would produce.
struct AddressVisitor;

impl<'de> serde::de::Visitor<'de> for AddressVisitor {
    type Value = Address;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a base58 address string or a 32-byte address array")
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Address, E> {
        value
            .parse::<Address>()
            .map_err(|error| E::custom(format!("invalid base58 address {value:?}: {error}")))
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut sequence: A) -> Result<Address, A::Error> {
        let mut bytes = [0u8; 32];
        for (index, slot) in bytes.iter_mut().enumerate() {
            *slot = sequence
                .next_element()?
                .ok_or_else(|| serde::de::Error::invalid_length(index, &"a 32-byte address"))?;
        }
        if sequence.next_element::<u8>()?.is_some() {
            return Err(serde::de::Error::custom("address array must have exactly 32 bytes"));
        }
        Ok(Address::new_from_array(bytes))
    }
}

pub(crate) fn record_fixture(request: &RecordRequest) -> Result<(), CliError> {
    // Reject an unusable output target before spending work on execution, so a
    // typo never leaves a half-recorded fixture behind.
    let format = fixture_format_for_output(&request.output)?;

    let transaction = load_transaction(request)?;
    let accounts = load_accounts(&request.accounts)?;
    let programs = parse_program_map(&request.programs)?;

    let mut vm = HPSVM::new();
    let slot = request.slot.unwrap_or(vm.block_env().slot);
    let mut runtime =
        RuntimeFixtureConfig::new(slot, request.log_bytes_limit, request.sigverify, false);
    // Carry the requested budget on the fixture as well as on the VM. The
    // recorded baseline is captured under this limit, so a replay that used the
    // VM default instead would not reproduce it.
    if let Some(compute_unit_limit) = request.compute_unit_limit {
        runtime = runtime.with_compute_unit_limit(compute_unit_limit);
    }

    // Mirror `FixtureRunner` exactly: configure, bind programs, seed accounts,
    // then execute. Any divergence here would record a baseline that a later
    // replay cannot reproduce.
    vm.set_sigverify(runtime.sigverify);
    vm.set_blockhash_check(runtime.blockhash_check);
    vm.set_log_bytes_limit(runtime.log_bytes_limit);
    if let Some(compute_unit_limit) = runtime.compute_unit_limit {
        vm.set_compute_unit_limit(compute_unit_limit);
    }
    vm.warp_to_slot(runtime.slot);

    let loader_id = request.loader.unwrap_or_else(bpf_loader_upgradeable::id);
    let mut bindings = Vec::with_capacity(programs.len());
    for (program_id, elf) in &programs {
        vm.add_program_with_loader(*program_id, elf, loader_id)?;
        bindings.push(ProgramBinding::new(*program_id, loader_id, None));
    }

    for account in &accounts {
        vm.set_account(
            account.address,
            Account {
                lamports: account.lamports,
                data: account.data.clone(),
                owner: account.owner,
                executable: account.executable,
                rent_epoch: account.rent_epoch,
            },
        )?;
    }

    let baseline = ExecutionSnapshot::from_outcome(&vm.transact(transaction.clone()));

    let mut builder = CaptureBuilder::new(fixture_name(request))
        .runtime(runtime)
        .programs(bindings)
        .pre_accounts(accounts)
        .baseline(baseline)
        .compares(if request.ignore_compute_units {
            Compare::everything_but_compute_units()
        } else {
            Compare::everything()
        });
    for tag in &request.tags {
        builder = builder.tag(tag.clone());
    }
    if let Some(source) = &request.source {
        builder = builder.source(source.clone());
    }

    builder.capture_transaction(&transaction)?.save(&request.output, format)?;

    println!("RECORDED: {}", request.output.display());
    Ok(())
}

/// Reads and decodes the transaction to execute.
fn load_transaction(request: &RecordRequest) -> Result<VersionedTransaction, CliError> {
    let encoded = std::fs::read(&request.transaction)?;
    let bytes = request.transaction_encoding.decode(&encoded).map_err(|reason| {
        CliError::TransactionEncoding {
            path: request.transaction.display().to_string(),
            encoding: request.transaction_encoding.as_str().to_owned(),
            reason,
        }
    })?;

    wincode::deserialize::<VersionedTransaction>(&bytes)
        .map_err(|error| CliError::Fixture(FixtureError::DecodeTransaction(error)))
}

fn load_accounts(path: &Path) -> Result<Vec<AccountSnapshot>, CliError> {
    let raw = std::fs::read(path)?;
    let parsed: Vec<RecordAccount> = serde_json::from_slice(&raw).map_err(|error| {
        CliError::AccountFile { path: path.display().to_string(), reason: error.to_string() }
    })?;

    Ok(parsed
        .into_iter()
        .map(|account| {
            AccountSnapshot::new(
                account.address,
                account.lamports,
                account.owner,
                account.executable,
                account.rent_epoch,
                account.data,
            )
        })
        .collect())
}

fn fixture_name(request: &RecordRequest) -> String {
    request.name.clone().unwrap_or_else(|| {
        request
            .output
            .file_stem()
            .map_or_else(|| "fixture".to_owned(), |stem| stem.to_string_lossy().into_owned())
    })
}

fn fixture_format_for_output(path: &Path) -> Result<FixtureFormat, CliError> {
    match path.extension().and_then(std::ffi::OsStr::to_str) {
        Some("json") => Ok(FixtureFormat::Json),
        Some("bin") => Ok(FixtureFormat::Binary),
        _ => Err(FixtureError::UnsupportedFormat { path: path.display().to_string() }.into()),
    }
}
