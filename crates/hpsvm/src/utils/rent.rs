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
}
