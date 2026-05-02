use std::io::Write;

use solana_address::Address;
use solana_rent::Rent;

use crate::{AccountSnapshot, ExecutionSnapshot, ExecutionStatus, ResultConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum Check {
    Success,
    Failure,
    Included(bool),
    ComputeUnits(u64),
    Fee(u64),
    ReturnData(Vec<u8>),
    LogContains(String),
    InnerInstructionCount(usize),
    Account(AccountExpectation),
    AllRentExempt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct AccountExpectation {
    pub address: Address,
    pub lamports: Option<u64>,
    pub owner: Option<Address>,
    pub executable: Option<bool>,
    pub data: Option<Vec<u8>>,
    pub data_slice: Option<(usize, Vec<u8>)>,
    pub closed: Option<bool>,
    pub rent_exempt: Option<bool>,
}

#[derive(Debug, Clone)]
#[must_use = "builder methods return a new builder"]
pub struct AccountExpectationBuilder {
    inner: AccountExpectation,
}

impl Check {
    pub fn account(address: &Address) -> AccountExpectationBuilder {
        AccountExpectationBuilder {
            inner: AccountExpectation {
                address: *address,
                lamports: None,
                owner: None,
                executable: None,
                data: None,
                data_slice: None,
                closed: None,
                rent_exempt: None,
            },
        }
    }
}

impl AccountExpectationBuilder {
    pub fn lamports(mut self, lamports: u64) -> Self {
        self.inner.lamports = Some(lamports);
        self
    }

    pub fn owner(mut self, owner: Address) -> Self {
        self.inner.owner = Some(owner);
        self
    }

    pub fn executable(mut self, executable: bool) -> Self {
        self.inner.executable = Some(executable);
        self
    }

    pub fn data(mut self, data: Vec<u8>) -> Self {
        self.inner.data = Some(data);
        self
    }

    pub fn data_slice(mut self, offset: usize, data: Vec<u8>) -> Self {
        self.inner.data_slice = Some((offset, data));
        self
    }

    pub fn closed(mut self) -> Self {
        self.inner.closed = Some(true);
        self
    }

    pub fn rent_exempt(mut self) -> Self {
        self.inner.rent_exempt = Some(true);
        self
    }

    pub fn build(self) -> Check {
        Check::Account(self.inner)
    }
}

impl ExecutionSnapshot {
    pub fn run_checks(&self, checks: &[Check], config: &ResultConfig) -> bool {
        for check in checks {
            let pass = match check {
                Check::Success => matches!(self.status, ExecutionStatus::Success),
                Check::Failure => !matches!(self.status, ExecutionStatus::Success),
                Check::Included(expected) => self.included == *expected,
                Check::ComputeUnits(expected) => self.compute_units_consumed == *expected,
                Check::Fee(expected) => self.fee == *expected,
                Check::ReturnData(expected) => self
                    .return_data
                    .as_ref()
                    .is_some_and(|return_data| return_data.data == *expected),
                Check::LogContains(expected) => {
                    self.logs.iter().any(|line| line.contains(expected))
                }
                Check::InnerInstructionCount(expected) => {
                    self.inner_instructions.len() == *expected
                }
                Check::Account(expected) => self
                    .post_accounts
                    .iter()
                    .find(|account| account.address == expected.address)
                    .is_some_and(|account| account_matches(account, expected)),
                Check::AllRentExempt => self
                    .post_accounts
                    .iter()
                    .all(|account| account.lamports == 0 || is_rent_exempt(account)),
            };

            if !pass {
                return fail(config, format!("check failed: {check:?}"));
            }
        }

        true
    }
}

fn account_matches(account: &AccountSnapshot, expected: &AccountExpectation) -> bool {
    if expected.lamports.is_some_and(|lamports| account.lamports != lamports) {
        return false;
    }
    if expected.owner.is_some_and(|owner| account.owner != owner) {
        return false;
    }
    if expected.executable.is_some_and(|executable| account.executable != executable) {
        return false;
    }
    if expected.data.as_ref().is_some_and(|data| account.data != *data) {
        return false;
    }
    if let Some((offset, data)) = &expected.data_slice {
        let end = offset.saturating_add(data.len());
        if end > account.data.len() || account.data[*offset..end] != data[..] {
            return false;
        }
    }
    if expected.closed.is_some_and(|closed| closed) &&
        !(account.lamports == 0 && account.data.is_empty())
    {
        return false;
    }
    if expected.rent_exempt.is_some_and(|rent_exempt| is_rent_exempt(account) != rent_exempt) {
        return false;
    }
    true
}

fn is_rent_exempt(account: &AccountSnapshot) -> bool {
    Rent::default().is_exempt(account.lamports, account.data.len())
}

fn fail(config: &ResultConfig, message: String) -> bool {
    assert!(!config.panic, "{message}");
    if config.verbose {
        let _ = writeln!(std::io::stderr(), "{message}");
    }
    false
}
