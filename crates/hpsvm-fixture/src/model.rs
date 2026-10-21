#[cfg(feature = "instruction-fixture")]
use hpsvm::instruction::InstructionCase;
#[cfg(feature = "instruction-fixture")]
use solana_account::Account;
use solana_address::Address;
#[cfg(feature = "instruction-fixture")]
use solana_transaction::AccountMeta;

use crate::{AccountSnapshot, Compare, ExecutionSnapshot};

/// A replayable fixture: an input, the state it runs against, and the
/// expectations its execution must satisfy.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bin-codec", derive(wincode::SchemaWrite, wincode::SchemaRead))]
#[non_exhaustive]
pub struct Fixture {
    /// The fixture's metadata.
    pub header: FixtureHeader,
    /// What the fixture executes.
    pub input: FixtureInput,
    /// What the execution must produce.
    pub expectations: FixtureExpectations,
}

impl Fixture {
    /// Creates a fixture.
    pub fn new(
        header: FixtureHeader,
        input: FixtureInput,
        expectations: FixtureExpectations,
    ) -> Self {
        Self { header, input, expectations }
    }

    /// The program bindings this fixture declares.
    ///
    /// The single place the bindings of a fixture are read from: a
    /// recording, a replay and a benchmark all resolve the same list, so
    /// they cannot disagree about which programs a fixture needs.
    pub fn programs(&self) -> &[ProgramBinding] {
        match &self.input {
            FixtureInput::Transaction(transaction) => &transaction.programs,
            #[cfg(feature = "instruction-fixture")]
            FixtureInput::Instruction(instruction) => &instruction.programs,
        }
    }

    /// The runtime configuration this fixture was recorded under.
    ///
    /// Applying it through [`configure_vm`](crate::configure_vm) reproduces
    /// the conditions the baseline was captured in.
    pub fn runtime(&self) -> &RuntimeFixtureConfig {
        match &self.input {
            FixtureInput::Transaction(transaction) => &transaction.runtime,
            #[cfg(feature = "instruction-fixture")]
            FixtureInput::Instruction(instruction) => &instruction.runtime,
        }
    }
}

/// A fixture's metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bin-codec", derive(wincode::SchemaWrite, wincode::SchemaRead))]
#[non_exhaustive]
pub struct FixtureHeader {
    /// The version of the fixture format the fixture is written in.
    pub schema_version: u16,
    /// The fixture's name.
    pub name: String,
    /// The kind of input the fixture holds.
    pub kind: FixtureKind,
    /// Where the fixture came from, when it was imported.
    pub source: Option<String>,
    /// Free-form labels for grouping fixtures.
    pub tags: Vec<String>,
}

impl FixtureHeader {
    /// Creates a header for a fixture of the given kind.
    pub fn new(name: impl Into<String>, kind: FixtureKind) -> Self {
        Self { schema_version: 1, name: name.into(), kind, source: None, tags: Vec::new() }
    }

    /// Records where the fixture came from.
    pub fn source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Adds a free-form label.
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }
}

/// The kind of input a fixture holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bin-codec", derive(wincode::SchemaWrite, wincode::SchemaRead))]
#[non_exhaustive]
pub enum FixtureKind {
    /// A signed transaction.
    Transaction,
    /// A single instruction.
    #[cfg(feature = "instruction-fixture")]
    Instruction,
}

/// What a fixture executes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bin-codec", derive(wincode::SchemaWrite, wincode::SchemaRead))]
#[non_exhaustive]
pub enum FixtureInput {
    /// A signed transaction and the state it runs against.
    Transaction(TransactionFixture),
    /// A single instruction and the state it runs against.
    #[cfg(feature = "instruction-fixture")]
    Instruction(InstructionFixture),
}

/// A signed transaction and the state it runs against.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bin-codec", derive(wincode::SchemaWrite, wincode::SchemaRead))]
#[non_exhaustive]
pub struct TransactionFixture {
    /// The runtime configuration the transaction runs under.
    pub runtime: RuntimeFixtureConfig,
    /// The programs the transaction calls.
    pub programs: Vec<ProgramBinding>,
    /// The accounts the transaction starts from.
    pub pre_accounts: Vec<AccountSnapshot>,
    /// The `wincode`-serialized transaction.
    pub transaction_bytes: Vec<u8>,
}

impl TransactionFixture {
    /// Creates a transaction fixture.
    pub fn new(
        runtime: RuntimeFixtureConfig,
        programs: Vec<ProgramBinding>,
        pre_accounts: Vec<AccountSnapshot>,
        transaction_bytes: Vec<u8>,
    ) -> Self {
        Self { runtime, programs, pre_accounts, transaction_bytes }
    }
}

#[cfg(feature = "instruction-fixture")]
/// A single instruction and the state it runs against.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bin-codec", derive(wincode::SchemaWrite, wincode::SchemaRead))]
#[non_exhaustive]
pub struct InstructionFixture {
    /// The runtime configuration the instruction runs under.
    pub runtime: RuntimeFixtureConfig,
    /// The programs the instruction calls.
    pub programs: Vec<ProgramBinding>,
    /// The accounts the instruction starts from.
    pub pre_accounts: Vec<AccountSnapshot>,
    /// The program the instruction invokes.
    pub program_id: Address,
    /// The accounts the instruction passes.
    pub accounts: Vec<InstructionAccountMeta>,
    /// The instruction's data.
    pub data: Vec<u8>,
}

#[cfg(feature = "instruction-fixture")]
impl InstructionFixture {
    /// Creates an instruction fixture.
    pub fn new(
        runtime: RuntimeFixtureConfig,
        programs: Vec<ProgramBinding>,
        pre_accounts: Vec<AccountSnapshot>,
        program_id: Address,
        accounts: Vec<InstructionAccountMeta>,
        data: Vec<u8>,
    ) -> Self {
        Self { runtime, programs, pre_accounts, program_id, accounts, data }
    }

    /// Converts the fixture into the instruction case the VM executes.
    #[must_use]
    pub fn instruction_case(&self) -> InstructionCase {
        InstructionCase {
            program_id: self.program_id,
            accounts: self.accounts.iter().map(InstructionAccountMeta::to_account_meta).collect(),
            data: self.data.clone(),
            pre_accounts: self
                .pre_accounts
                .iter()
                .map(|account| {
                    (
                        account.address,
                        Account {
                            lamports: account.lamports,
                            data: account.data.clone(),
                            owner: account.owner,
                            executable: account.executable,
                            rent_epoch: account.rent_epoch,
                        },
                    )
                })
                .collect(),
        }
    }
}

#[cfg(feature = "instruction-fixture")]
/// One account an instruction passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bin-codec", derive(wincode::SchemaWrite, wincode::SchemaRead))]
#[non_exhaustive]
pub struct InstructionAccountMeta {
    /// The account's address.
    pub pubkey: Address,
    /// Whether the account signed the transaction.
    pub is_signer: bool,
    /// Whether the instruction may write the account.
    pub is_writable: bool,
}

#[cfg(feature = "instruction-fixture")]
impl InstructionAccountMeta {
    /// Creates an account meta.
    pub const fn new(pubkey: Address, is_signer: bool, is_writable: bool) -> Self {
        Self { pubkey, is_signer, is_writable }
    }

    /// Converts the account meta into the Solana instruction form.
    #[must_use]
    pub fn to_account_meta(&self) -> AccountMeta {
        AccountMeta {
            pubkey: self.pubkey,
            is_signer: self.is_signer,
            is_writable: self.is_writable,
        }
    }
}

/// The runtime configuration a fixture is recorded and replayed under.
///
/// Applying it through [`configure_vm`](crate::configure_vm) reproduces the
/// conditions the baseline was captured in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bin-codec", derive(wincode::SchemaWrite, wincode::SchemaRead))]
#[non_exhaustive]
pub struct RuntimeFixtureConfig {
    /// The slot the fixture runs at.
    pub slot: u64,
    /// The log truncation ceiling, or `None` for no limit.
    pub log_bytes_limit: Option<usize>,
    /// Whether signatures are verified.
    pub sigverify: bool,
    /// Whether the blockhash is checked.
    pub blockhash_check: bool,
    /// The compute unit budget, when the fixture pins one.
    #[cfg_attr(feature = "serde", serde(default))]
    pub compute_unit_limit: Option<u64>,
}

impl RuntimeFixtureConfig {
    /// Creates a runtime configuration.
    pub const fn new(
        slot: u64,
        log_bytes_limit: Option<usize>,
        sigverify: bool,
        blockhash_check: bool,
    ) -> Self {
        Self { slot, log_bytes_limit, sigverify, blockhash_check, compute_unit_limit: None }
    }

    /// Pins the compute unit budget the fixture runs under.
    pub const fn with_compute_unit_limit(mut self, compute_unit_limit: u64) -> Self {
        self.compute_unit_limit = Some(compute_unit_limit);
        self
    }
}

/// A program the fixture calls, and the loader it is installed with.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bin-codec", derive(wincode::SchemaWrite, wincode::SchemaRead))]
#[non_exhaustive]
pub struct ProgramBinding {
    /// The program's address.
    pub program_id: Address,
    /// The loader the program is installed with.
    pub loader_id: Address,
    /// A free-form label for the program's role in the fixture.
    pub role: Option<String>,
}

impl ProgramBinding {
    /// Creates a program binding.
    pub fn new(program_id: Address, loader_id: Address, role: Option<String>) -> Self {
        Self { program_id, loader_id, role }
    }
}

/// What a fixture's execution must produce.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bin-codec", derive(wincode::SchemaWrite, wincode::SchemaRead))]
#[non_exhaustive]
pub struct FixtureExpectations {
    /// The recorded baseline the execution is compared against.
    pub baseline: ExecutionSnapshot,
    /// Which parts of the baseline the comparison covers.
    pub compares: Vec<Compare>,
}

impl FixtureExpectations {
    /// Creates a fixture's expectations.
    pub fn new(baseline: ExecutionSnapshot, compares: Vec<Compare>) -> Self {
        Self { baseline, compares }
    }
}

/// The format a fixture is stored in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FixtureFormat {
    /// The JSON form.
    Json,
    /// The binary form.
    Binary,
}
