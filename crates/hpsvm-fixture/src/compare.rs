use std::collections::{HashMap, HashSet};

use solana_address::Address;

use crate::{
    AccountSnapshot, ExecutionSnapshot, ResultConfig,
    outcome::{Mismatch, MismatchKind, report_mismatch},
};

/// A part of an execution snapshot two snapshots are compared on.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bin-codec", derive(wincode::SchemaWrite, wincode::SchemaRead))]
#[non_exhaustive]
pub enum Compare {
    /// The execution's status.
    Status,
    /// Whether the execution was included in a block.
    Included,
    /// The compute units the execution consumed.
    ComputeUnits,
    /// The fee the execution paid.
    Fee,
    /// The data the execution returned.
    ReturnData,
    /// The execution's logs.
    Logs,
    /// The number of inner instructions the execution ran.
    InnerInstructionCount,
    /// The accounts the execution touched.
    Accounts(AccountCompareScope),
}

/// Which of the execution's accounts a comparison covers.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bin-codec", derive(wincode::SchemaWrite, wincode::SchemaRead))]
#[non_exhaustive]
pub enum AccountCompareScope {
    /// Every account the execution touched.
    All,
    /// Only the listed accounts.
    Only(Vec<Address>),
    /// Every account except the listed ones.
    AllExcept(Vec<Address>),
}

impl Compare {
    /// Every part of a snapshot, including the compute units.
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

    /// Every part of a snapshot except the compute units.
    ///
    /// Useful when the candidate build is expected to consume a different
    /// number of compute units but must otherwise behave identically.
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
    /// Compares this snapshot with `other` under the given predicates.
    ///
    /// Returns `true` when every predicate holds. A failure is reported
    /// through [`ResultConfig`]; it never aborts the process unless the
    /// `panic` flag is explicitly enabled.
    pub fn compare_with(&self, other: &Self, compares: &[Compare], config: &ResultConfig) -> bool {
        self.try_compare_with(other, compares, config).is_ok()
    }

    /// Compares this snapshot with `other` under the given predicates and
    /// reports the first failure as data.
    ///
    /// The `Result` form of [`compare_with`](Self::compare_with): the caller
    /// receives the failing predicate instead of a bare `false`.
    pub fn try_compare_with(
        &self,
        other: &Self,
        compares: &[Compare],
        config: &ResultConfig,
    ) -> Result<(), Mismatch> {
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
                return Err(report_mismatch(
                    config,
                    MismatchKind::Comparison,
                    format!("{compare:?}"),
                ));
            }
        }

        Ok(())
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

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::{ExecutionFailure, ExecutionStatus, InnerInstructionSnapshot, ReturnDataSnapshot};

    fn config() -> ResultConfig {
        ResultConfig { panic: false, verbose: false }
    }

    fn address(seed: u8) -> Address {
        Address::new_from_array([seed; 32])
    }

    fn account(target: Address, lamports: u64, data: Vec<u8>) -> AccountSnapshot {
        AccountSnapshot::new(target, lamports, address(200), false, 0, data)
    }

    fn base() -> ExecutionSnapshot {
        ExecutionSnapshot {
            status: ExecutionStatus::Success,
            included: true,
            compute_units_consumed: 150,
            fee: 5_000,
            logs: vec![String::from("Program log: hello")],
            return_data: Some(ReturnDataSnapshot::new(address(9), vec![1, 2, 3])),
            inner_instructions: vec![InnerInstructionSnapshot::new(2, 1, vec![0], vec![7], 0)],
            post_accounts: vec![
                account(address(1), 1_000, vec![0; 4]),
                account(address(2), 20, vec![1; 4]),
            ],
        }
    }

    #[test]
    fn status_compares_the_execution_outcome() {
        let left = base();
        let right = base();
        assert!(left.compare_with(&right, &[Compare::Status], &config()));

        let failed = ExecutionSnapshot {
            status: ExecutionStatus::Failure {
                failure: ExecutionFailure::Other {
                    kind: String::from("InstructionError"),
                    message: String::from("boom"),
                },
            },
            ..base()
        };
        assert!(!left.compare_with(&failed, &[Compare::Status], &config()));
        assert!(!failed.compare_with(&left, &[Compare::Status], &config()));
        assert!(failed.compare_with(&failed, &[Compare::Status], &config()));
    }

    #[test]
    fn included_compares_the_flag() {
        let left = base();
        let excluded = ExecutionSnapshot { included: false, ..base() };
        assert!(left.compare_with(&left, &[Compare::Included], &config()));
        assert!(!left.compare_with(&excluded, &[Compare::Included], &config()));
        assert!(!excluded.compare_with(&left, &[Compare::Included], &config()));
    }

    #[test]
    fn compute_units_must_match() {
        let left = base();
        let right = ExecutionSnapshot { compute_units_consumed: 151, ..base() };
        assert!(left.compare_with(&base(), &[Compare::ComputeUnits], &config()));
        assert!(!left.compare_with(&right, &[Compare::ComputeUnits], &config()));
    }

    #[test]
    fn fee_must_match() {
        let right = ExecutionSnapshot { fee: 4_999, ..base() };
        assert!(base().compare_with(&base(), &[Compare::Fee], &config()));
        assert!(!base().compare_with(&right, &[Compare::Fee], &config()));
    }

    #[test]
    fn return_data_compares_program_and_payload_together() {
        let left = base();
        assert!(left.compare_with(&base(), &[Compare::ReturnData], &config()));

        let different_data = ExecutionSnapshot {
            return_data: Some(ReturnDataSnapshot::new(address(9), vec![9])),
            ..base()
        };
        assert!(!left.compare_with(&different_data, &[Compare::ReturnData], &config()));

        let different_program = ExecutionSnapshot {
            return_data: Some(ReturnDataSnapshot::new(address(8), vec![1, 2, 3])),
            ..base()
        };
        assert!(!left.compare_with(&different_program, &[Compare::ReturnData], &config()));

        let none = ExecutionSnapshot { return_data: None, ..base() };
        assert!(!left.compare_with(&none, &[Compare::ReturnData], &config()));
    }

    #[test]
    fn logs_compare_as_an_ordered_sequence() {
        let left = base();
        assert!(left.compare_with(&base(), &[Compare::Logs], &config()));

        let reordered = ExecutionSnapshot {
            logs: vec![String::from("Program log: hello"), String::from("extra")],
            ..base()
        };
        assert!(!left.compare_with(&reordered, &[Compare::Logs], &config()));

        let fewer = ExecutionSnapshot { logs: Vec::new(), ..base() };
        assert!(!fewer.compare_with(&left, &[Compare::Logs], &config()));
    }

    #[test]
    fn inner_instruction_count_compares_group_counts() {
        let left = base();
        assert!(left.compare_with(&base(), &[Compare::InnerInstructionCount], &config()));

        let extra = ExecutionSnapshot {
            inner_instructions: vec![
                InnerInstructionSnapshot::new(2, 1, vec![0], vec![7], 0),
                InnerInstructionSnapshot::new(2, 1, vec![0], vec![7], 1),
            ],
            ..base()
        };
        assert!(!left.compare_with(&extra, &[Compare::InnerInstructionCount], &config()));
    }

    #[test]
    fn accounts_all_detects_a_lamport_difference() {
        let left = base();
        let right = ExecutionSnapshot {
            post_accounts: vec![
                account(address(1), 999, vec![0; 4]),
                account(address(2), 20, vec![1; 4]),
            ],
            ..base()
        };
        assert!(left.compare_with(
            &base(),
            &[Compare::Accounts(AccountCompareScope::All)],
            &config()
        ));
        assert!(!left.compare_with(
            &right,
            &[Compare::Accounts(AccountCompareScope::All)],
            &config()
        ));
    }

    #[test]
    fn accounts_all_detects_a_data_only_difference() {
        let left = base();
        let right = ExecutionSnapshot {
            post_accounts: vec![
                account(address(1), 1_000, vec![9; 4]),
                account(address(2), 20, vec![1; 4]),
            ],
            ..base()
        };
        assert!(!left.compare_with(
            &right,
            &[Compare::Accounts(AccountCompareScope::All)],
            &config()
        ));
    }

    #[test]
    fn accounts_all_detects_an_account_missing_on_the_right() {
        let left = base();
        let right = ExecutionSnapshot {
            post_accounts: vec![account(address(1), 1_000, vec![0; 4])],
            ..base()
        };
        assert!(!left.compare_with(
            &right,
            &[Compare::Accounts(AccountCompareScope::All)],
            &config()
        ));
    }

    #[test]
    fn accounts_all_detects_an_account_present_only_on_the_right() {
        let left = ExecutionSnapshot {
            post_accounts: vec![account(address(1), 1_000, vec![0; 4])],
            ..base()
        };
        let right = base();
        assert!(!left.compare_with(
            &right,
            &[Compare::Accounts(AccountCompareScope::All)],
            &config()
        ));
    }

    #[test]
    fn accounts_over_two_empty_sets_matches() {
        let left = ExecutionSnapshot { post_accounts: Vec::new(), ..base() };
        let right = ExecutionSnapshot { post_accounts: Vec::new(), ..base() };
        assert!(left.compare_with(
            &right,
            &[Compare::Accounts(AccountCompareScope::All)],
            &config()
        ));
    }

    #[test]
    fn accounts_only_ignores_unselected_differences() {
        let left = base();
        let right = ExecutionSnapshot {
            post_accounts: vec![
                account(address(1), 1_000, vec![0; 4]),
                account(address(2), 999, vec![7; 4]),
            ],
            ..base()
        };
        // Only account 1 is in scope, and it is identical, so account 2's
        // difference is ignored.
        assert!(left.compare_with(
            &right,
            &[Compare::Accounts(AccountCompareScope::Only(vec![address(1)]))],
            &config()
        ));
        // Including account 2 exposes it.
        assert!(!left.compare_with(
            &right,
            &[Compare::Accounts(AccountCompareScope::Only(vec![address(1), address(2)]))],
            &config()
        ));
    }

    #[test]
    fn accounts_only_fails_when_a_selected_account_is_missing_on_the_right() {
        let right = ExecutionSnapshot {
            post_accounts: vec![account(address(1), 1_000, vec![0; 4])],
            ..base()
        };
        assert!(!base().compare_with(
            &right,
            &[Compare::Accounts(AccountCompareScope::Only(vec![address(2)]))],
            &config()
        ));
    }

    #[test]
    fn accounts_only_with_an_empty_selection_ignores_everything() {
        let left = base();
        let right = ExecutionSnapshot { post_accounts: Vec::new(), ..base() };
        assert!(left.compare_with(
            &right,
            &[Compare::Accounts(AccountCompareScope::Only(Vec::new()))],
            &config()
        ));
    }

    #[test]
    fn accounts_all_except_ignores_the_excluded_accounts() {
        let left = base();
        let right = ExecutionSnapshot {
            post_accounts: vec![
                account(address(1), 1_000, vec![0; 4]),
                account(address(2), 999, vec![7; 4]),
            ],
            ..base()
        };
        assert!(left.compare_with(
            &right,
            &[Compare::Accounts(AccountCompareScope::AllExcept(vec![address(2)]))],
            &config()
        ));
        // Not excluding account 2 exposes the difference.
        assert!(!left.compare_with(
            &right,
            &[Compare::Accounts(AccountCompareScope::AllExcept(Vec::new()))],
            &config()
        ));
    }

    #[test]
    fn accounts_all_except_detects_an_extra_account_that_is_not_excluded() {
        let left = ExecutionSnapshot {
            post_accounts: vec![account(address(1), 1_000, vec![0; 4])],
            ..base()
        };
        let right = base();
        assert!(!left.compare_with(
            &right,
            &[Compare::Accounts(AccountCompareScope::AllExcept(vec![address(1)]))],
            &config()
        ));
        assert!(left.compare_with(
            &right,
            &[Compare::Accounts(AccountCompareScope::AllExcept(vec![address(2)]))],
            &config()
        ));
    }

    #[test]
    fn everything_covers_every_dimension() {
        let left = base();
        assert!(left.compare_with(&base(), &Compare::everything(), &config()));

        // Perturbing any single dimension must break `everything()`.
        let perturbations = vec![
            ExecutionSnapshot {
                status: ExecutionStatus::Failure {
                    failure: ExecutionFailure::Other {
                        kind: String::from("x"),
                        message: String::from("y"),
                    },
                },
                ..base()
            },
            ExecutionSnapshot { included: false, ..base() },
            ExecutionSnapshot { compute_units_consumed: 1, ..base() },
            ExecutionSnapshot { fee: 1, ..base() },
            ExecutionSnapshot { return_data: None, ..base() },
            ExecutionSnapshot { logs: Vec::new(), ..base() },
            ExecutionSnapshot { inner_instructions: Vec::new(), ..base() },
            ExecutionSnapshot { post_accounts: Vec::new(), ..base() },
        ];
        for perturbed in perturbations {
            assert!(
                !left.compare_with(&perturbed, &Compare::everything(), &config()),
                "unexpected match for {perturbed:?}"
            );
        }
    }

    #[test]
    fn everything_but_compute_units_ignores_compute_units_only() {
        let left = base();
        let more_compute = ExecutionSnapshot { compute_units_consumed: 999, ..base() };
        assert!(left.compare_with(
            &more_compute,
            &Compare::everything_but_compute_units(),
            &config()
        ));
        assert!(!left.compare_with(&more_compute, &Compare::everything(), &config()));
    }

    #[test]
    fn everything_but_compute_units_still_catches_every_other_dimension() {
        let left = base();
        for perturbed in [
            ExecutionSnapshot { included: false, ..base() },
            ExecutionSnapshot { fee: 1, ..base() },
            ExecutionSnapshot { post_accounts: Vec::new(), ..base() },
        ] {
            assert!(!left.compare_with(
                &perturbed,
                &Compare::everything_but_compute_units(),
                &config()
            ));
        }
    }

    #[test]
    fn an_empty_compare_list_always_passes() {
        let left = base();
        let right = ExecutionSnapshot { post_accounts: Vec::new(), ..base() };
        assert!(left.compare_with(&right, &[], &config()));
    }

    #[test]
    fn comparisons_short_circuit_on_the_first_failure() {
        let left = base();
        let right = ExecutionSnapshot { fee: 1, compute_units_consumed: 2, ..base() };
        assert!(!left.compare_with(&right, &Compare::everything(), &config()));
    }

    #[test]
    #[should_panic(expected = "comparison failed")]
    fn a_failing_comparison_panics_when_configured_to_panic() {
        let panic_config = ResultConfig { panic: true, verbose: false };
        let left = base();
        let _ = left.compare_with(
            &ExecutionSnapshot { fee: 1, ..base() },
            &[Compare::Fee],
            &panic_config,
        );
    }

    #[test]
    fn a_passing_comparison_does_not_panic_under_panic_config() {
        let panic_config = ResultConfig { panic: true, verbose: false };
        assert!(base().compare_with(&base(), &Compare::everything(), &panic_config));
    }

    #[test]
    fn a_failing_comparison_returns_false_when_verbose() {
        let verbose = ResultConfig { panic: false, verbose: true };
        assert!(!base().compare_with(
            &ExecutionSnapshot { fee: 1, ..base() },
            &[Compare::Fee],
            &verbose
        ));
    }

    // Property: `Accounts(All)` is reflexive for any post-account set.
    proptest! {
        #[test]
        fn accounts_all_equivalence_is_reflexive_for_any_account_set(
            accounts in prop::collection::vec(
                (any::<u64>(), any::<u64>(), prop::collection::vec(any::<u8>(), 0..8)),
                0..6,
            ),
        ) {
            let post_accounts: Vec<AccountSnapshot> = accounts
                .iter()
                .enumerate()
                .map(|(index, (lamports, _, data))| {
                    account(Address::new_from_array([index as u8; 32]), *lamports, data.clone())
                })
                .collect();
            let left = ExecutionSnapshot { post_accounts: post_accounts.clone(), ..base() };
            let right = ExecutionSnapshot { post_accounts, ..base() };

            prop_assert!(left.compare_with(
                &right,
                &[Compare::Accounts(AccountCompareScope::All)],
                &config()
            ));
        }
    }

    // Property: excluding every account makes `AllExcept` vacuously true, even
    // when the account sets are completely different.
    proptest! {
        #[test]
        fn excluding_everything_always_matches(
            accounts in prop::collection::vec(any::<u64>(), 0..6),
        ) {
            let left = ExecutionSnapshot {
                post_accounts: accounts
                    .iter()
                    .enumerate()
                    .map(|(index, lamports)| {
                        account(Address::new_from_array([index as u8; 32]), *lamports, Vec::new())
                    })
                    .collect(),
                ..base()
            };
            let right = ExecutionSnapshot { post_accounts: Vec::new(), ..base() };
            let excluded: Vec<Address> =
                left.post_accounts.iter().map(|account| account.address).collect();

            prop_assert!(left.compare_with(
                &right,
                &[Compare::Accounts(AccountCompareScope::AllExcept(excluded))],
                &config()
            ));
        }
    }

    // Property: perturbing any single numeric field of a snapshot is caught by
    // `Compare::everything()`.
    proptest! {
        #[test]
        fn everything_catches_any_single_field_perturbation(
            compute_units in any::<u64>(),
            fee in any::<u64>(),
        ) {
            let base_snapshot = base();
            let everything = Compare::everything();
            // Start from the generated values so a zero `compute_units` or `fee`
            // is a legitimate baseline rather than an accidental mismatch.
            let left = ExecutionSnapshot {
                compute_units_consumed: compute_units,
                fee,
                ..base_snapshot.clone()
            };
            let fee_drifted = ExecutionSnapshot {
                compute_units_consumed: compute_units,
                fee: fee.wrapping_add(1),
                ..base_snapshot.clone()
            };
            let unit_drifted = ExecutionSnapshot {
                compute_units_consumed: compute_units.wrapping_add(1),
                fee,
                ..base_snapshot
            };

            prop_assert!(left.compare_with(&left.clone(), &everything, &config()));
            prop_assert!(!left.compare_with(&fee_drifted, &everything, &config()));
            prop_assert!(!left.compare_with(&unit_drifted, &everything, &config()));
        }
    }
}
