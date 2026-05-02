use std::io::Write;

use solana_address::Address;

use crate::{ExecutionSnapshot, ResultConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum Compare {
    Status,
    Included,
    ComputeUnits,
    Fee,
    ReturnData,
    Logs,
    InnerInstructionCount,
    Accounts(AccountCompareScope),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum AccountCompareScope {
    All,
    Only(Vec<Address>),
    AllExcept(Vec<Address>),
}

impl Compare {
    pub fn everything() -> Vec<Self> {
        vec![
            Self::Status,
            Self::Included,
            Self::ComputeUnits,
            Self::Fee,
            Self::ReturnData,
            Self::Logs,
            Self::InnerInstructionCount,
            Self::Accounts(AccountCompareScope::All),
        ]
    }

    pub fn everything_but_compute_units() -> Vec<Self> {
        vec![
            Self::Status,
            Self::Included,
            Self::Fee,
            Self::ReturnData,
            Self::Logs,
            Self::InnerInstructionCount,
            Self::Accounts(AccountCompareScope::All),
        ]
    }
}

impl ExecutionSnapshot {
    pub fn compare_with(&self, other: &Self, compares: &[Compare], config: &ResultConfig) -> bool {
        for compare in compares {
            let pass = match compare {
                Compare::Status => self.status == other.status,
                Compare::Included => self.included == other.included,
                Compare::ComputeUnits => {
                    self.compute_units_consumed == other.compute_units_consumed
                }
                Compare::Fee => self.fee == other.fee,
                Compare::ReturnData => self.return_data == other.return_data,
                Compare::Logs => self.logs == other.logs,
                Compare::InnerInstructionCount => {
                    self.inner_instructions.len() == other.inner_instructions.len()
                }
                Compare::Accounts(scope) => compare_accounts(self, other, scope),
            };

            if !pass {
                return fail(config, format!("comparison failed: {compare:?}"));
            }
        }

        true
    }
}

fn compare_accounts(
    left: &ExecutionSnapshot,
    right: &ExecutionSnapshot,
    scope: &AccountCompareScope,
) -> bool {
    let should_compare = |address: &Address| match scope {
        AccountCompareScope::All => true,
        AccountCompareScope::Only(addresses) => addresses.contains(address),
        AccountCompareScope::AllExcept(addresses) => !addresses.contains(address),
    };

    for account in &left.post_accounts {
        if !should_compare(&account.address) {
            continue;
        }
        let Some(other_account) =
            right.post_accounts.iter().find(|candidate| candidate.address == account.address)
        else {
            return false;
        };
        if account != other_account {
            return false;
        }
    }

    for account in &right.post_accounts {
        if should_compare(&account.address) &&
            !left.post_accounts.iter().any(|candidate| candidate.address == account.address)
        {
            return false;
        }
    }

    true
}

fn fail(config: &ResultConfig, message: String) -> bool {
    assert!(!config.panic, "{message}");
    if config.verbose {
        let _ = writeln!(std::io::stderr(), "{message}");
    }
    false
}
