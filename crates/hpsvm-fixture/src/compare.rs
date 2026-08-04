use std::{
    collections::{HashMap, HashSet},
    io::Write,
};

use solana_address::Address;

use crate::{AccountSnapshot, ExecutionSnapshot, ResultConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bin-codec", derive(wincode::SchemaWrite, wincode::SchemaRead))]
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
#[cfg_attr(feature = "bin-codec", derive(wincode::SchemaWrite, wincode::SchemaRead))]
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

/// Compare the post-execution account sets of two snapshots.
///
/// Both the scope filter and the cross-snapshot lookup are indexed through
/// hash sets/maps, giving `O(n + m)` behaviour. The previous implementation
/// nested a linear `find` inside a linear scan and resolved the scope with
/// `Vec::contains`, which degraded to `O(n^2 * m)` on fixtures that touch many
/// accounts (SPL token-2022 flows in particular).
fn compare_accounts(
    left: &ExecutionSnapshot,
    right: &ExecutionSnapshot,
    scope: &AccountCompareScope,
) -> bool {
    let scoped: Option<HashSet<&Address>> = match scope {
        AccountCompareScope::All => None,
        AccountCompareScope::Only(addresses) | AccountCompareScope::AllExcept(addresses) => {
            Some(addresses.iter().collect())
        }
    };
    let should_compare = |address: &Address| match scope {
        AccountCompareScope::All => true,
        AccountCompareScope::Only(_) => {
            scoped.as_ref().is_some_and(|listed| listed.contains(address))
        }
        AccountCompareScope::AllExcept(_) => {
            !scoped.as_ref().is_some_and(|listed| listed.contains(address))
        }
    };

    let right_by_address: HashMap<&Address, &AccountSnapshot> =
        right.post_accounts.iter().map(|account| (&account.address, account)).collect();

    for account in &left.post_accounts {
        if !should_compare(&account.address) {
            continue;
        }
        let Some(other_account) = right_by_address.get(&account.address) else {
            return false;
        };
        if account != *other_account {
            return false;
        }
    }

    let left_addresses: HashSet<&Address> =
        left.post_accounts.iter().map(|account| &account.address).collect();

    !right.post_accounts.iter().any(|account| {
        should_compare(&account.address) && !left_addresses.contains(&account.address)
    })
}

fn fail(config: &ResultConfig, message: String) -> bool {
    assert!(!config.panic, "{message}");
    if config.verbose {
        let _ = writeln!(std::io::stderr(), "{message}");
    }
    false
}
