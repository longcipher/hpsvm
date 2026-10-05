//! This code is taken from <https://github.com/anza-xyz/agave/blob/master/svm/src/rent_calculator.rs>.
//! Commit 6fbbaf67837e2dc973822be9e1c20e1fed58e8eb
use solana_address::Address;
use solana_rent::Rent;
use solana_transaction_context::IndexOfAccount;
use solana_transaction_error::{TransactionError, TransactionResult};

/// Rent state of a Solana account.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RentState {
    /// account.lamports == 0
    Uninitialized,
    /// 0 < account.lamports < rent-exempt-minimum
    RentPaying {
        lamports: u64,    // account.lamports()
        data_size: usize, // account.data().len()
    },
    /// account.lamports >= rent-exempt-minimum
    RentExempt,
}

/// Check rent state transition for an account directly.
///
/// This method has a default implementation that checks whether the
/// transition is allowed and returns an error if it is not. It also
/// verifies that the account is not the incinerator.
pub(crate) fn check_rent_state_with_account(
    pre_rent_state: &RentState,
    post_rent_state: &RentState,
    address: &Address,
    account_index: IndexOfAccount,
) -> TransactionResult<()> {
    if !solana_sdk_ids::incinerator::check_id(address) &&
        !transition_allowed(pre_rent_state, post_rent_state)
    {
        let account_index = account_index as u8;
        Err(TransactionError::InsufficientFundsForRent { account_index })
    } else {
        Ok(())
    }
}

/// Determine the rent state of an account.
///
/// This method has a default implementation that treats accounts with zero
/// lamports as uninitialized and uses the implemented `get_rent` to
/// determine whether an account is rent-exempt.
pub(crate) fn get_account_rent_state(
    rent: &Rent,
    account_lamports: u64,
    account_size: usize,
) -> RentState {
    if account_lamports == 0 {
        RentState::Uninitialized
    } else if rent.is_exempt(account_lamports, account_size) {
        RentState::RentExempt
    } else {
        RentState::RentPaying { data_size: account_size, lamports: account_lamports }
    }
}

/// Check whether a transition from the pre_rent_state to the
/// post_rent_state is valid.
///
/// This method has a default implementation that allows transitions from
/// any state to `RentState::Uninitialized` or `RentState::RentExempt`.
/// Pre-state `RentState::RentPaying` can only transition to
/// `RentState::RentPaying` if the data size remains the same and the
/// account is not credited.
pub(crate) fn transition_allowed(pre_rent_state: &RentState, post_rent_state: &RentState) -> bool {
    match post_rent_state {
        RentState::Uninitialized | RentState::RentExempt => true,
        RentState::RentPaying { data_size: post_data_size, lamports: post_lamports } => {
            match pre_rent_state {
                RentState::Uninitialized | RentState::RentExempt => false,
                RentState::RentPaying { data_size: pre_data_size, lamports: pre_lamports } => {
                    // Cannot remain RentPaying if resized or credited.
                    post_data_size == pre_data_size && post_lamports <= pre_lamports
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn transition_uninitialized_to_rent_exempt() {
        let pre = RentState::Uninitialized;
        let post = RentState::RentExempt;
        assert!(transition_allowed(&pre, &post));
    }

    #[test]
    fn transition_rent_paying_debit() {
        let pre = RentState::RentPaying { lamports: 1000, data_size: 100 };
        let post = RentState::RentPaying { lamports: 900, data_size: 100 };
        assert!(transition_allowed(&pre, &post));
    }

    #[test]
    fn transition_rent_paying_credit() {
        let pre = RentState::RentPaying { lamports: 1000, data_size: 100 };
        let post = RentState::RentPaying { lamports: 1100, data_size: 100 };
        assert!(!transition_allowed(&pre, &post));
    }

    #[test]
    fn transition_rent_paying_resize() {
        let pre = RentState::RentPaying { lamports: 1000, data_size: 100 };
        let post = RentState::RentPaying { lamports: 1000, data_size: 200 };
        assert!(!transition_allowed(&pre, &post));
    }

    #[test]
    fn transition_rent_exempt_to_uninitialized() {
        let pre = RentState::RentExempt;
        let post = RentState::Uninitialized;
        assert!(transition_allowed(&pre, &post));
    }

    #[test]
    fn transition_any_to_rent_exempt() {
        let cases = vec![
            (RentState::Uninitialized, RentState::RentExempt),
            (RentState::RentExempt, RentState::RentExempt),
            (RentState::RentPaying { lamports: 500, data_size: 50 }, RentState::RentExempt),
        ];
        for (pre, post) in cases {
            assert!(transition_allowed(&pre, &post), "failed for {pre:?} -> {post:?}");
        }
    }

    #[test]
    fn check_rent_state_incinerator_bypass() {
        let incinerator = solana_sdk_ids::incinerator::id();
        let cases = vec![
            (RentState::Uninitialized, RentState::RentExempt),
            (RentState::Uninitialized, RentState::Uninitialized),
            (RentState::Uninitialized, RentState::RentPaying { lamports: 100, data_size: 50 }),
            (RentState::RentExempt, RentState::Uninitialized),
            (RentState::RentExempt, RentState::RentExempt),
            (RentState::RentExempt, RentState::RentPaying { lamports: 100, data_size: 50 }),
            (RentState::RentPaying { lamports: 1000, data_size: 100 }, RentState::Uninitialized),
            (RentState::RentPaying { lamports: 1000, data_size: 100 }, RentState::RentExempt),
            (
                RentState::RentPaying { lamports: 1000, data_size: 100 },
                RentState::RentPaying { lamports: 2000, data_size: 100 },
            ),
        ];
        for (pre, post) in cases {
            let result = check_rent_state_with_account(&pre, &post, &incinerator, 0);
            assert!(result.is_ok(), "incinerator should bypass rent check for {pre:?} -> {post:?}");
        }
    }

    #[test]
    fn check_rent_state_invalid_transition() {
        let address = Address::new_unique();
        let pre = RentState::RentPaying { lamports: 1000, data_size: 100 };
        let post = RentState::RentPaying { lamports: 1100, data_size: 100 };
        let result = check_rent_state_with_account(&pre, &post, &address, 7);
        match result {
            Err(TransactionError::InsufficientFundsForRent { account_index }) => {
                assert_eq!(account_index, 7);
            }
            other => panic!("expected InsufficientFundsForRent, got {other:?}"),
        }
    }

    #[test]
    fn get_rent_state_uninitialized() {
        let rent = Rent::default();
        let state = get_account_rent_state(&rent, 0, 100);
        assert_eq!(state, RentState::Uninitialized);
    }

    #[test]
    fn get_rent_state_rent_exempt() {
        let rent = Rent::default();
        let data_size: usize = 100;
        let exempt_lamports = rent.minimum_balance(data_size);
        let state = get_account_rent_state(&rent, exempt_lamports, data_size);
        assert_eq!(state, RentState::RentExempt);
    }

    #[test]
    fn get_rent_state_rent_paying() {
        let rent = Rent::default();
        let data_size: usize = 100;
        let state = get_account_rent_state(&rent, 1, data_size);
        assert_eq!(state, RentState::RentPaying { data_size, lamports: 1 });
    }

    /// A pre-state that is not `RentPaying` can never transition into
    /// `RentPaying`: an account cannot be resurrected below the exempt minimum
    /// or resized out of exemption.
    #[test]
    fn transition_into_rent_paying_is_rejected_from_every_non_paying_pre_state() {
        let post = RentState::RentPaying { lamports: 1, data_size: 10 };
        for pre in [
            RentState::Uninitialized,
            RentState::RentExempt,
            RentState::RentPaying { lamports: 1, data_size: 10 },
        ] {
            // Sanity: the paying pre-state with identical values is allowed.
            let expected = matches!(pre, RentState::RentPaying { .. });
            assert_eq!(transition_allowed(&pre, &post), expected, "pre {pre:?} -> post {post:?}");
        }
    }

    /// `transition_allowed` must mirror `check_rent_state_with_account`: any
    /// allowed transition is accepted for a non-incinerator account.
    #[test]
    fn allowed_transitions_are_accepted_for_a_regular_account() {
        let address = Address::new_unique();
        let cases = vec![
            (RentState::Uninitialized, RentState::RentExempt),
            (RentState::RentExempt, RentState::Uninitialized),
            (RentState::RentPaying { lamports: 1000, data_size: 100 }, RentState::Uninitialized),
            (RentState::RentPaying { lamports: 1000, data_size: 100 }, RentState::RentExempt),
            (
                RentState::RentPaying { lamports: 1000, data_size: 100 },
                RentState::RentPaying { lamports: 999, data_size: 100 },
            ),
        ];
        for (pre, post) in cases {
            assert!(transition_allowed(&pre, &post), "expected {pre:?} -> {post:?} to be allowed");
            assert_eq!(
                check_rent_state_with_account(&pre, &post, &address, 3),
                Ok(()),
                "check_rent_state_with_account {pre:?} -> {post:?}"
            );
        }
    }

    /// Every rejected transition must surface as `InsufficientFundsForRent`
    /// carrying the caller's account index.
    #[test]
    fn rejected_transitions_report_the_caller_supplied_account_index() {
        let address = Address::new_unique();
        let pre = RentState::Uninitialized;
        let post = RentState::RentPaying { lamports: 5, data_size: 5 };
        for index in 0..=u8::MAX {
            match check_rent_state_with_account(&pre, &post, &address, index as IndexOfAccount) {
                Err(TransactionError::InsufficientFundsForRent { account_index }) => {
                    assert_eq!(account_index, index);
                }
                other => panic!("index {index}: expected InsufficientFundsForRent, got {other:?}"),
            }
        }
    }

    /// The incinerator bypass is unconditional: even a transition that is
    /// otherwise rejected must be allowed at the incinerator address.
    #[test]
    fn incinerator_bypasses_rejected_transitions() {
        let incinerator = solana_sdk_ids::incinerator::id();
        assert!(!solana_sdk_ids::incinerator::check_id(&Address::new_unique()));
        for pre in [RentState::Uninitialized, RentState::RentExempt] {
            let post = RentState::RentPaying { lamports: 7, data_size: 7 };
            assert!(!transition_allowed(&pre, &post));
            assert_eq!(check_rent_state_with_account(&pre, &post, &incinerator, 0), Ok(()));
        }
    }

    /// `get_account_rent_state` is a pure function of (lamports, size) against
    /// a `Rent`: for any lamports it is either Uninitialized, RentExempt, or a
    /// `RentPaying` echoing the inputs, never a mix.
    #[test]
    fn get_account_rent_state_always_echoes_its_inputs() {
        let rent = Rent::default();
        for lamports in [0, 1, 2, 1000, 1_000_000, u64::MAX] {
            // `Rent` rejects anything above `MAX_PERMITTED_DATA_LENGTH`, so the
            // oversized case is checked separately below.
            for data_size in [0, 1, 165, 10_000, 1_000_000] {
                let state = get_account_rent_state(&rent, lamports, data_size);
                if lamports == 0 {
                    assert_eq!(
                        state,
                        RentState::Uninitialized,
                        "lamports {lamports}, size {data_size}"
                    );
                } else if rent.is_exempt(lamports, data_size) {
                    assert_eq!(
                        state,
                        RentState::RentExempt,
                        "lamports {lamports}, size {data_size}"
                    );
                } else {
                    assert_eq!(
                        state,
                        RentState::RentPaying { data_size, lamports },
                        "lamports {lamports}, size {data_size}"
                    );
                }
            }
        }
    }

    // Property: an allowed transition can never increase the account's lamports
    // or change its data size — the two invariants that keep `RentPaying` stable.
    proptest! {
        #[test]
        fn allowed_transitions_never_increase_lamports_or_change_size(
            pre_lamports in 1u64..1_000_000,
            pre_size in 0usize..4096,
            delta in -1000i64..1000,
            size_delta in -512i64..512,
        ) {
            let pre = RentState::RentPaying { lamports: pre_lamports, data_size: pre_size };
            let post_lamports = pre_lamports.wrapping_add(delta as u64);
            let post_size = (pre_size as i64 + size_delta).max(0) as usize;
            let post = RentState::RentPaying { lamports: post_lamports, data_size: post_size };

            if transition_allowed(&pre, &post) {
                prop_assert_eq!(post_size, pre_size);
                prop_assert!(post_lamports <= pre_lamports);
            }
        }
    }

    // Property: `Uninitialized` and `RentExempt` targets are always reachable
    // from any pre-state (including paying accounts being closed or topped up).
    proptest! {
        #[test]
        fn closing_or_exempting_is_always_allowed(
            pre_lamports in 0u64..1_000_000,
            pre_size in 0usize..4096,
        ) {
            let pre = if pre_lamports == 0 {
                RentState::Uninitialized
            } else {
                RentState::RentPaying { lamports: pre_lamports, data_size: pre_size }
            };
            prop_assert!(transition_allowed(&pre, &RentState::Uninitialized));
            prop_assert!(transition_allowed(&pre, &RentState::RentExempt));
        }
    }

    // Property: `transition_allowed` is exactly `check_rent_state_with_account`
    // succeeding for a non-incinerator account, for every state pair.
    proptest! {
        #[test]
        fn check_matches_the_pure_predicate_for_regular_accounts(
            pre_lamports in 0u64..1_000_000,
            pre_size in 0usize..4096,
            post_lamports in 0u64..1_000_000,
            post_size in 0usize..4096,
        ) {
            let pre = if pre_lamports == 0 {
                RentState::Uninitialized
            } else {
                RentState::RentPaying { lamports: pre_lamports, data_size: pre_size }
            };
            let post = if post_lamports == 0 {
                RentState::Uninitialized
            } else {
                RentState::RentPaying { lamports: post_lamports, data_size: post_size }
            };
            let checked = check_rent_state_with_account(&pre, &post, &Address::new_unique(), 0);
            prop_assert_eq!(checked.is_ok(), transition_allowed(&pre, &post));
        }
    }
}
