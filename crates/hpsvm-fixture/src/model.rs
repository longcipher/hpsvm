use hpsvm::instruction::InstructionCase;
use solana_account::Account;
use solana_address::Address;
use solana_transaction::AccountMeta;

use crate::{AccountSnapshot, Compare, ExecutionSnapshot};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Fixture {
    pub header: FixtureHeader,
    pub input: FixtureInput,
    pub expectations: FixtureExpectations,
}

impl Fixture {
    pub fn new(
        header: FixtureHeader,
        input: FixtureInput,
        expectations: FixtureExpectations,
    ) -> Self {
        Self { header, input, expectations }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct FixtureHeader {
    pub schema_version: u16,
    pub name: String,
    pub kind: FixtureKind,
    pub source: Option<String>,
    pub tags: Vec<String>,
}

impl FixtureHeader {
    pub fn new(name: impl Into<String>, kind: FixtureKind) -> Self {
        Self { schema_version: 1, name: name.into(), kind, source: None, tags: Vec::new() }
    }

    pub fn source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum FixtureKind {
    Transaction,
    Instruction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum FixtureInput {
    Transaction(TransactionFixture),
    Instruction(InstructionFixture),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct TransactionFixture {
    pub runtime: RuntimeFixtureConfig,
    pub programs: Vec<ProgramBinding>,
    pub pre_accounts: Vec<AccountSnapshot>,
    pub transaction_bytes: Vec<u8>,
}

impl TransactionFixture {
    pub fn new(
        runtime: RuntimeFixtureConfig,
        programs: Vec<ProgramBinding>,
        pre_accounts: Vec<AccountSnapshot>,
        transaction_bytes: Vec<u8>,
    ) -> Self {
        Self { runtime, programs, pre_accounts, transaction_bytes }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct InstructionFixture {
    pub runtime: RuntimeFixtureConfig,
    pub programs: Vec<ProgramBinding>,
    pub pre_accounts: Vec<AccountSnapshot>,
    pub program_id: Address,
    pub accounts: Vec<InstructionAccountMeta>,
    pub data: Vec<u8>,
}

impl InstructionFixture {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct InstructionAccountMeta {
    pub pubkey: Address,
    pub is_signer: bool,
    pub is_writable: bool,
}

impl InstructionAccountMeta {
    pub const fn new(pubkey: Address, is_signer: bool, is_writable: bool) -> Self {
        Self { pubkey, is_signer, is_writable }
    }

    #[must_use]
    pub fn to_account_meta(&self) -> AccountMeta {
        AccountMeta {
            pubkey: self.pubkey,
            is_signer: self.is_signer,
            is_writable: self.is_writable,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct RuntimeFixtureConfig {
    pub slot: u64,
    pub log_bytes_limit: Option<usize>,
    pub sigverify: bool,
    pub blockhash_check: bool,
}

impl RuntimeFixtureConfig {
    pub const fn new(
        slot: u64,
        log_bytes_limit: Option<usize>,
        sigverify: bool,
        blockhash_check: bool,
    ) -> Self {
        Self { slot, log_bytes_limit, sigverify, blockhash_check }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ProgramBinding {
    pub program_id: Address,
    pub loader_id: Address,
    pub role: Option<String>,
}

impl ProgramBinding {
    pub fn new(program_id: Address, loader_id: Address, role: Option<String>) -> Self {
        Self { program_id, loader_id, role }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct FixtureExpectations {
    pub baseline: ExecutionSnapshot,
    pub compares: Vec<Compare>,
}

impl FixtureExpectations {
    pub fn new(baseline: ExecutionSnapshot, compares: Vec<Compare>) -> Self {
        Self { baseline, compares }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureFormat {
    Json,
    Binary,
}
