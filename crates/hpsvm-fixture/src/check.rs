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

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::{InnerInstructionSnapshot, ReturnDataSnapshot};

    fn config() -> ResultConfig {
        ResultConfig { panic: false, verbose: false }
    }

    fn address(seed: u8) -> Address {
        Address::new_from_array([seed; 32])
    }

    fn account(target: Address, lamports: u64, data: Vec<u8>) -> AccountSnapshot {
        AccountSnapshot::new(target, lamports, address(200), false, 0, data)
    }

    fn snapshot() -> ExecutionSnapshot {
        ExecutionSnapshot {
            status: ExecutionStatus::Success,
            included: true,
            compute_units_consumed: 150,
            fee: 5_000,
            logs: vec![String::from("Program log: hello"), String::from("Program log: world")],
            return_data: Some(ReturnDataSnapshot::new(address(9), vec![1, 2, 3])),
            inner_instructions: vec![InnerInstructionSnapshot::new(2, 1, vec![0], vec![7], 0)],
            post_accounts: vec![account(address(1), 1_000, vec![0; 10])],
        }
    }

    fn failed_snapshot() -> ExecutionSnapshot {
        ExecutionSnapshot {
            status: ExecutionStatus::Failure {
                kind: String::from("InstructionError"),
                message: String::from("boom"),
            },
            ..snapshot()
        }
    }

    #[test]
    fn success_passes_on_success_and_fails_on_failure() {
        assert!(snapshot().run_checks(&[Check::Success], &config()));
        assert!(!failed_snapshot().run_checks(&[Check::Success], &config()));
    }

    #[test]
    fn failure_passes_on_failure_and_fails_on_success() {
        assert!(failed_snapshot().run_checks(&[Check::Failure], &config()));
        assert!(!snapshot().run_checks(&[Check::Failure], &config()));
    }

    #[test]
    fn included_compares_the_flag_in_both_directions() {
        assert!(snapshot().run_checks(&[Check::Included(true)], &config()));
        assert!(!snapshot().run_checks(&[Check::Included(false)], &config()));
        assert!(!failed_snapshot().run_checks(&[Check::Included(false)], &config()));
    }

    #[test]
    fn compute_units_must_match_exactly() {
        assert!(snapshot().run_checks(&[Check::ComputeUnits(150)], &config()));
        assert!(!snapshot().run_checks(&[Check::ComputeUnits(151)], &config()));
        assert!(!snapshot().run_checks(&[Check::ComputeUnits(0)], &config()));
    }

    #[test]
    fn fee_must_match_exactly() {
        assert!(snapshot().run_checks(&[Check::Fee(5_000)], &config()));
        assert!(!snapshot().run_checks(&[Check::Fee(4_999)], &config()));
    }

    #[test]
    fn return_data_must_match_byte_for_byte() {
        assert!(snapshot().run_checks(&[Check::ReturnData(vec![1, 2, 3])], &config()));
        assert!(!snapshot().run_checks(&[Check::ReturnData(vec![1, 2])], &config()));
        assert!(!snapshot().run_checks(&[Check::ReturnData(vec![1, 2, 3, 4])], &config()));
    }

    #[test]
    fn return_data_check_fails_when_no_return_data_was_produced() {
        let mut without_return_data = snapshot();
        without_return_data.return_data = None;
        assert!(!without_return_data.run_checks(&[Check::ReturnData(vec![1])], &config()));
    }

    #[test]
    fn log_contains_matches_any_line() {
        assert!(snapshot().run_checks(&[Check::LogContains(String::from("hello"))], &config()));
        assert!(snapshot().run_checks(&[Check::LogContains(String::from("world"))], &config()));
        assert!(!snapshot().run_checks(&[Check::LogContains(String::from("missing"))], &config()));
    }

    #[test]
    fn log_contains_of_an_empty_needle_matches_the_first_line() {
        assert!(snapshot().run_checks(&[Check::LogContains(String::new())], &config()));
    }

    #[test]
    fn log_contains_fails_when_there_are_no_logs() {
        let mut without_logs = snapshot();
        without_logs.logs = Vec::new();
        assert!(!without_logs.run_checks(&[Check::LogContains(String::from("x"))], &config()));
    }

    #[test]
    fn inner_instruction_count_counts_groups() {
        assert!(snapshot().run_checks(&[Check::InnerInstructionCount(1)], &config()));
        assert!(!snapshot().run_checks(&[Check::InnerInstructionCount(2)], &config()));
        assert!(!snapshot().run_checks(&[Check::InnerInstructionCount(0)], &config()));
    }

    #[test]
    fn account_lamports_must_match() {
        assert!(
            snapshot()
                .run_checks(&[Check::account(&address(1)).lamports(1_000).build()], &config())
        );
        assert!(
            !snapshot().run_checks(&[Check::account(&address(1)).lamports(999).build()], &config())
        );
    }

    #[test]
    fn account_owner_must_match() {
        assert!(
            snapshot()
                .run_checks(&[Check::account(&address(1)).owner(address(200)).build()], &config())
        );
        assert!(
            !snapshot()
                .run_checks(&[Check::account(&address(1)).owner(address(201)).build()], &config())
        );
    }

    #[test]
    fn account_executable_must_match() {
        assert!(
            snapshot()
                .run_checks(&[Check::account(&address(1)).executable(false).build()], &config())
        );
        assert!(
            !snapshot()
                .run_checks(&[Check::account(&address(1)).executable(true).build()], &config())
        );
    }

    #[test]
    fn account_data_must_match_exactly() {
        assert!(
            snapshot()
                .run_checks(&[Check::account(&address(1)).data(vec![0; 10]).build()], &config())
        );
        assert!(
            !snapshot()
                .run_checks(&[Check::account(&address(1)).data(vec![0; 9]).build()], &config())
        );
        assert!(
            !snapshot()
                .run_checks(&[Check::account(&address(1)).data(vec![1; 10]).build()], &config())
        );
    }

    #[test]
    fn account_data_slice_matches_a_window_of_the_data() {
        let snapshot = ExecutionSnapshot {
            post_accounts: vec![account(address(1), 10, (0u8..10).collect())],
            ..snapshot()
        };
        assert!(snapshot.run_checks(
            &[Check::account(&address(1)).data_slice(2, vec![2, 3, 4]).build()],
            &config()
        ));
        assert!(
            snapshot.run_checks(
                &[Check::account(&address(1)).data_slice(0, vec![]).build()],
                &config()
            )
        );
        assert!(
            snapshot.run_checks(
                &[Check::account(&address(1)).data_slice(10, vec![]).build()],
                &config()
            )
        );
    }

    #[test]
    fn account_data_slice_rejects_a_non_matching_window() {
        let snapshot = ExecutionSnapshot {
            post_accounts: vec![account(address(1), 10, (0u8..10).collect())],
            ..snapshot()
        };
        assert!(!snapshot.run_checks(
            &[Check::account(&address(1)).data_slice(2, vec![9, 9]).build()],
            &config()
        ));
    }

    /// A `data_slice` window that runs past the end of the account data must
    /// fail rather than panic.
    #[test]
    fn account_data_slice_past_the_end_fails_without_panicking() {
        let snapshot = ExecutionSnapshot {
            post_accounts: vec![account(address(1), 10, vec![0; 4])],
            ..snapshot()
        };
        assert!(!snapshot.run_checks(
            &[Check::account(&address(1)).data_slice(3, vec![0, 1]).build()],
            &config()
        ));
        // An offset beyond the length with an empty needle also fails.
        assert!(
            !snapshot.run_checks(
                &[Check::account(&address(1)).data_slice(99, vec![]).build()],
                &config()
            )
        );
        // `usize::MAX` must not overflow into a wrapped, in-range slice.
        assert!(!snapshot.run_checks(
            &[Check::account(&address(1)).data_slice(usize::MAX, vec![0]).build()],
            &config()
        ));
    }

    #[test]
    fn account_closed_requires_zero_lamports_and_empty_data() {
        let snapshot = ExecutionSnapshot {
            post_accounts: vec![
                account(address(1), 0, Vec::new()),
                account(address(2), 5, Vec::new()),
                account(address(3), 0, vec![1]),
            ],
            ..snapshot()
        };
        assert!(snapshot.run_checks(&[Check::account(&address(1)).closed().build()], &config()));
        assert!(!snapshot.run_checks(&[Check::account(&address(2)).closed().build()], &config()));
        assert!(!snapshot.run_checks(&[Check::account(&address(3)).closed().build()], &config()));
    }

    #[test]
    fn account_closed_on_a_missing_address_fails() {
        assert!(
            !snapshot().run_checks(&[Check::account(&address(77)).closed().build()], &config())
        );
    }

    #[test]
    fn account_rent_exempt_matches_the_rent_calculator() {
        let rent = solana_rent::Rent::default();
        let exempt = rent.minimum_balance(10);
        let snapshot = ExecutionSnapshot {
            post_accounts: vec![
                account(address(1), exempt, vec![0; 10]),
                account(address(2), 1, vec![0; 10]),
            ],
            ..snapshot()
        };
        assert!(
            snapshot.run_checks(&[Check::account(&address(1)).rent_exempt().build()], &config())
        );
        assert!(
            !snapshot.run_checks(&[Check::account(&address(2)).rent_exempt().build()], &config())
        );
    }

    #[test]
    fn all_rent_exempt_treats_empty_accounts_as_exempt() {
        let rent = solana_rent::Rent::default();
        let snapshot = ExecutionSnapshot {
            post_accounts: vec![
                account(address(1), rent.minimum_balance(10), vec![0; 10]),
                account(address(2), 0, Vec::new()),
            ],
            ..snapshot()
        };
        assert!(snapshot.run_checks(&[Check::AllRentExempt], &config()));
    }

    #[test]
    fn all_rent_exempt_fails_on_a_paying_account() {
        let snapshot = ExecutionSnapshot {
            post_accounts: vec![account(address(1), 1, vec![0; 10])],
            ..snapshot()
        };
        assert!(!snapshot.run_checks(&[Check::AllRentExempt], &config()));
    }

    /// An empty `post_accounts` list makes `AllRentExempt` vacuously true.
    #[test]
    fn all_rent_exempt_over_an_empty_account_set_passes() {
        let mut without_accounts = snapshot();
        without_accounts.post_accounts = Vec::new();
        assert!(without_accounts.run_checks(&[Check::AllRentExempt], &config()));
    }

    #[test]
    fn an_empty_check_list_always_passes() {
        assert!(snapshot().run_checks(&[], &config()));
        assert!(failed_snapshot().run_checks(&[], &config()));
    }

    /// Checks are short-circuiting: the first failure stops evaluation.
    #[test]
    fn checks_short_circuit_on_the_first_failure() {
        let checks = [Check::ComputeUnits(1), Check::Failure, Check::Success];
        assert!(!snapshot().run_checks(&checks, &config()));
        // The same list reordered so the impossible check comes last still fails,
        // proving no later check can rescue an earlier failure.
        assert!(!snapshot().run_checks(&[Check::ComputeUnits(999), Check::Success], &config()));
    }

    #[test]
    fn multiple_satisfied_checks_all_pass() {
        let checks = [
            Check::Success,
            Check::Included(true),
            Check::ComputeUnits(150),
            Check::Fee(5_000),
            Check::ReturnData(vec![1, 2, 3]),
            Check::LogContains(String::from("hello")),
            Check::InnerInstructionCount(1),
            Check::AllRentExempt,
        ];
        // `post_accounts[0]` has 1000 lamports for 10 bytes, so AllRentExempt
        // is expected to fail; drop it and assert the rest.
        let without_rent: Vec<Check> =
            checks.iter().filter(|check| !matches!(check, Check::AllRentExempt)).cloned().collect();
        assert!(snapshot().run_checks(&without_rent, &config()));
    }

    /// `panic: true` turns a failed check into a panic instead of `false`, so
    /// callers using it get a hard failure with the check's debug output.
    #[test]
    #[should_panic(expected = "check failed")]
    fn a_failing_check_panics_when_configured_to_panic() {
        let panic_config = ResultConfig { panic: true, verbose: false };
        let _ = snapshot().run_checks(&[Check::ComputeUnits(1)], &panic_config);
    }

    #[test]
    fn a_passing_check_does_not_panic_under_panic_config() {
        let panic_config = ResultConfig { panic: true, verbose: false };
        assert!(snapshot().run_checks(&[Check::Success], &panic_config));
    }

    #[test]
    fn a_failing_check_returns_false_instead_of_panicking_by_default() {
        assert!(!snapshot().run_checks(&[Check::Failure], &config()));
    }

    #[test]
    fn a_failing_check_returns_false_when_verbose() {
        let verbose = ResultConfig { panic: false, verbose: true };
        assert!(!snapshot().run_checks(&[Check::Failure], &verbose));
    }

    // Property: an exact-value check always passes and an off-by-one always
    // fails, for every snapshot.
    proptest! {
        #[test]
        fn checks_are_deterministic_for_the_same_snapshot(
            compute_units in any::<u64>(),
            fee in any::<u64>(),
        ) {
            let snapshot =
                ExecutionSnapshot { compute_units_consumed: compute_units, fee, ..snapshot() };

            prop_assert!(snapshot.run_checks(&[Check::ComputeUnits(compute_units)], &config()));
            prop_assert!(!snapshot.run_checks(
                &[Check::ComputeUnits(compute_units.wrapping_add(1))],
                &config()
            ));
            prop_assert!(snapshot.run_checks(&[Check::Fee(fee)], &config()));
            prop_assert!(!snapshot.run_checks(&[Check::Fee(fee.wrapping_add(1))], &config()));
        }
    }

    // Property: `account_matches` is a conjunction of independent predicates,
    // so any single mismatching predicate is enough to reject.
    proptest! {
        #[test]
        fn any_single_mismatching_predicate_rejects_the_account(
            lamports in any::<u64>(),
            data_len in 0usize..64,
        ) {
            let snapshot = ExecutionSnapshot {
                post_accounts: vec![account(address(1), lamports, vec![0; data_len])],
                ..snapshot()
            };
            // `account` always produces a non-executable snapshot, so the
            // matching expectation asks for `false`. Each remaining check flips
            // exactly one predicate, which is what makes the conjunction claim
            // above testable.
            let base =
                Check::account(&address(1)).lamports(lamports).executable(false).build();
            prop_assert!(snapshot.run_checks(&[base], &config()));

            prop_assert!(!snapshot.run_checks(
                &[Check::account(&address(1)).lamports(lamports.wrapping_add(1)).build()],
                &config()
            ));
            prop_assert!(!snapshot.run_checks(
                &[Check::account(&address(1)).executable(true).build()],
                &config()
            ));
            prop_assert!(!snapshot.run_checks(
                &[Check::account(&address(1)).data(vec![0; data_len + 1]).build()],
                &config()
            ));
        }
    }

    // Property: an account expectation with no constraints set always matches,
    // whatever the account state is.
    proptest! {
        #[test]
        fn an_unconstrained_account_expectation_always_matches(
            lamports in any::<u64>(),
            data in prop::collection::vec(any::<u8>(), 0..32),
            executable in any::<bool>(),
        ) {
            let snapshot = ExecutionSnapshot {
                post_accounts: vec![AccountSnapshot::new(
                    Address::new_unique(),
                    lamports,
                    Address::new_unique(),
                    executable,
                    0,
                    data,
                )],
                ..snapshot()
            };
            let account_address = snapshot.post_accounts[0].address;

            prop_assert!(snapshot.run_checks(&[Check::account(&account_address).build()], &config()));
        }
    }
}
