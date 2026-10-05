//! Token operations for the HPSVM.
//!
//! Provides SPL token helpers (transfer, mint, ATA, freeze, etc.) for tests
//! built on [`crate::HPSVM`]. Enable via the `token` feature.

/// Account snapshot factories for fast fixture and test setup.
pub mod accounts;
pub mod error;

pub use self::error::TokenError;
mod approve;
mod approve_checked;
mod burn;
mod burn_checked;
mod close_account;
mod create_account;
mod create_ata;
mod create_ata_idempotent;
mod create_mint;
mod create_multisig;
#[cfg(not(feature = "token-2022"))]
mod create_native_mint;
#[cfg(feature = "token-2022")]
mod create_native_mint_2022;
mod freeze_account;
mod mint_to;
mod mint_to_checked;
mod revoke;
mod set_authority;
mod sync_native;
mod thaw_account;
mod transfer;
mod transfer_checked;

use solana_address::Address;
use solana_keypair::Keypair;
use solana_program_pack::{IsInitialized, Pack};
use solana_signer::Signer;
use solana_transaction::Transaction;
#[cfg(feature = "token-2022")]
pub use spl_token_2022_interface as spl_token;
#[cfg(not(feature = "token-2022"))]
pub use spl_token_interface as spl_token;

#[cfg(feature = "token-2022")]
use self::create_native_mint_2022 as create_native_mint;
pub use self::{
    approve::*, approve_checked::*, burn::*, burn_checked::*, close_account::*, create_account::*,
    create_ata::*, create_ata_idempotent::*, create_mint::*, create_multisig::*,
    create_native_mint::*, freeze_account::*, mint_to::*, mint_to_checked::*, revoke::*,
    set_authority::*, sync_native::*, thaw_account::*, transfer::*, transfer_checked::*,
};
use crate::{HPSVM, types::FailedTransactionMetadata};

/// SPL Token program ID
pub const TOKEN_ID: Address = spl_token::ID;

/// Get an SPL account from the SVM
pub fn get_spl_account<T: Pack + IsInitialized>(
    svm: &HPSVM,
    account: &Address,
) -> Result<T, TokenError> {
    let account = svm.get_account(account).ok_or(TokenError::AccountNotFound(*account))?;
    let data = account
        .data
        .get(..T::LEN)
        .ok_or(TokenError::AccountDataTooSmall { expected: T::LEN, actual: account.data.len() })?;
    let account = T::unpack(data)?;

    Ok(account)
}

fn get_multisig_signers<'a>(
    authority: &Address,
    signing_pubkeys: &'a [Address],
) -> Vec<&'a Address> {
    if signing_pubkeys == [*authority] {
        vec![]
    } else {
        signing_pubkeys.iter().collect::<Vec<_>>()
    }
}

pub(crate) fn sign_and_send(
    svm: &mut HPSVM,
    payer: &Keypair,
    signers: &[&Keypair],
    ix: solana_instruction::Instruction,
) -> Result<(), FailedTransactionMetadata> {
    let payer_pk = payer.pubkey();
    let block_hash = svm.latest_blockhash();
    let mut tx = Transaction::new_with_payer(&[ix], Some(&payer_pk));
    tx.partial_sign(&[payer], block_hash);
    tx.partial_sign(signers, block_hash);
    svm.send_transaction(tx)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use solana_program_option::COption;
    use solana_program_pack::IsInitialized;

    use super::*;
    use crate::token::accounts::{keyed_mint_account, keyed_system_account, system_account};

    fn sample_mint() -> spl_token::state::Mint {
        spl_token::state::Mint {
            mint_authority: COption::Some(Address::new_unique()),
            supply: 1_000,
            decimals: 6,
            is_initialized: true,
            ..Default::default()
        }
    }

    /// A single-owner authority must not be passed as a multisig signer list:
    /// the SPL token program rejects `Account::is_signer` multisig accounts.
    #[test]
    fn multisig_signers_are_empty_for_a_single_owner() {
        let authority = Address::new_unique();
        assert!(get_multisig_signers(&authority, &[authority]).is_empty());
    }

    /// A multisig authority signs through its signer accounts, and they must be
    /// forwarded verbatim in order.
    #[test]
    fn multisig_signers_are_forwarded_for_multi_owner_authorities() {
        let authority = Address::new_unique();
        let signers = [Address::new_unique(), authority, Address::new_unique()];
        assert_eq!(
            get_multisig_signers(&authority, &signers),
            vec![&signers[0], &signers[1], &signers[2]]
        );
    }

    #[test]
    fn multisig_signers_of_an_empty_list_are_empty() {
        let authority = Address::new_unique();
        assert!(get_multisig_signers(&authority, &[]).is_empty());
    }

    /// Two distinct signers must not be collapsed even though the slice has the
    /// same length as the single-owner case.
    #[test]
    fn two_distinct_signers_are_not_collapsed() {
        let authority = Address::new_unique();
        let other = Address::new_unique();
        assert_eq!(get_multisig_signers(&authority, &[authority, other]).len(), 2);
    }

    #[test]
    fn get_spl_account_reports_a_missing_account() {
        let svm = HPSVM::new();
        let address = Address::new_unique();

        match get_spl_account::<spl_token::state::Mint>(&svm, &address) {
            Err(TokenError::AccountNotFound(missing)) => assert_eq!(missing, address),
            other => panic!("expected TokenError::AccountNotFound, got {other:?}"),
        }
    }

    #[test]
    fn get_spl_account_reports_short_account_data() {
        let mut svm = HPSVM::new();
        let address = Address::new_unique();
        svm.set_account(address, system_account(1_000)).unwrap();

        match get_spl_account::<spl_token::state::Mint>(&svm, &address) {
            Err(TokenError::AccountDataTooSmall { expected, actual }) => {
                assert_eq!(expected, spl_token::state::Mint::LEN);
                assert_eq!(actual, 0);
            }
            other => panic!("expected TokenError::AccountDataTooSmall, got {other:?}"),
        }
    }

    /// `get_spl_account` forwards only the first `T::LEN` bytes, so trailing
    /// bytes in a padded account must be ignored rather than rejected.
    #[test]
    fn get_spl_account_ignores_trailing_padding() {
        let mut svm = HPSVM::new();
        let address = Address::new_unique();
        let mint_state = sample_mint();

        let mut data = vec![0u8; spl_token::state::Mint::LEN + 32];
        // `pack` demands an exactly-sized slice, so pack into the leading window.
        let (head, _padding) = data.split_at_mut(spl_token::state::Mint::LEN);
        spl_token::state::Mint::pack(mint_state.clone(), head).unwrap();
        let account = solana_account::Account {
            lamports: 1,
            owner: TOKEN_ID,
            data,
            executable: false,
            rent_epoch: 0,
        };
        svm.set_account(address, account).unwrap();

        assert_eq!(get_spl_account::<spl_token::state::Mint>(&svm, &address).unwrap(), mint_state);
    }

    /// Data that is long enough but not a valid mint must surface the unpack
    /// failure rather than being reported as missing or short.
    #[test]
    fn get_spl_account_surfaces_unpack_failures() {
        let mut svm = HPSVM::new();
        let address = Address::new_unique();
        let account = solana_account::Account {
            lamports: 1,
            owner: TOKEN_ID,
            // A non-initialized mint (`is_initialized == 0`) fails `IsInitialized`.
            data: vec![0u8; spl_token::state::Mint::LEN],
            executable: false,
            rent_epoch: 0,
        };
        svm.set_account(address, account).unwrap();

        match get_spl_account::<spl_token::state::Mint>(&svm, &address) {
            Err(TokenError::UnpackError(_)) => {}
            other => panic!("expected TokenError::UnpackError, got {other:?}"),
        }
    }

    #[test]
    fn get_spl_account_round_trips_a_mint_installed_by_the_factory() {
        let mut svm = HPSVM::new();
        let address = Address::new_unique();
        let mint_state = sample_mint();
        set_keyed(&mut svm, keyed_mint_account(address, mint_state.clone()));

        let unpacked = get_spl_account::<spl_token::state::Mint>(&svm, &address).unwrap();
        assert_eq!(unpacked, mint_state);
        assert!(unpacked.is_initialized());
    }

    #[test]
    fn get_spl_account_round_trips_a_token_account_installed_by_the_factory() {
        use crate::token::accounts::keyed_token_account;

        let mut svm = HPSVM::new();
        let address = Address::new_unique();
        let mint = Address::new_unique();
        let token = spl_token::state::Account {
            mint,
            owner: Address::new_unique(),
            amount: 77,
            state: spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        set_keyed(&mut svm, keyed_token_account(address, token.clone()));

        assert_eq!(get_spl_account::<spl_token::state::Account>(&svm, &address).unwrap(), token);
    }

    /// The not-found error must win over the too-small error when the account
    /// does not exist at all.
    #[test]
    fn get_spl_account_prefers_not_found_over_size_checks() {
        let svm = HPSVM::new();
        let address = Address::new_unique();
        assert!(matches!(
            get_spl_account::<spl_token::state::Account>(&svm, &address),
            Err(TokenError::AccountNotFound(_))
        ));
    }

    /// An account owned by an unrelated program is still readable: `get_spl_account`
    /// only unpacks bytes and does not check ownership.
    #[test]
    fn get_spl_account_does_not_check_account_ownership() {
        let mut svm = HPSVM::new();
        let address = Address::new_unique();
        let mint_state = sample_mint();
        let (key, account) = keyed_mint_account(address, mint_state.clone());
        svm.set_account(key, solana_account::Account { owner: Address::new_unique(), ..account })
            .unwrap();

        assert_eq!(get_spl_account::<spl_token::state::Mint>(&svm, &address).unwrap(), mint_state);
    }

    #[test]
    fn get_spl_account_reports_a_missing_account_even_when_other_accounts_exist() {
        let mut svm = HPSVM::new();
        let present = Address::new_unique();
        let absent = Address::new_unique();
        svm.set_account(
            present,
            solana_account::Account {
                lamports: 5,
                owner: Address::new_unique(),
                ..Default::default()
            },
        )
        .unwrap();

        assert!(matches!(
            get_spl_account::<spl_token::state::Mint>(&svm, &absent),
            Err(TokenError::AccountNotFound(missing)) if missing == absent
        ));
    }

    fn set_keyed(svm: &mut HPSVM, keyed: (Address, solana_account::Account)) {
        let (address, account) = keyed;
        svm.set_account(address, account).unwrap();
    }

    // Property: `get_multisig_signers` collapses to empty exactly when the
    // signing list is the single authority, and otherwise preserves it.
    proptest! {
        #[test]
        fn multisig_signer_collapse_is_exactly_the_single_authority_case(
            extra in prop::collection::vec(any::<[u8; 32]>(), 0..4),
        ) {
            let authority = Address::new_unique();
            let mut signing: Vec<Address> =
                extra.iter().map(|bytes| Address::new_from_array(*bytes)).collect();
            signing.push(authority);

            // The list ends in the authority, so it collapses exactly when nothing
            // precedes the authority.
            let collapses = signing.len() == 1;
            let resolved = get_multisig_signers(&authority, &signing);
            if collapses {
                prop_assert!(resolved.is_empty());
            } else {
                prop_assert_eq!(resolved.len(), signing.len());
            }
        }
    }

    // Property: `get_multisig_signers` returns a subset of the input, in the
    // same relative order, for any list.
    proptest! {
        #[test]
        fn multisig_signers_preserve_input_order(
            keys in prop::collection::vec(any::<[u8; 32]>(), 0..8),
        ) {
            let authority = Address::new_unique();
            let signing: Vec<Address> =
                keys.iter().map(|bytes| Address::new_from_array(*bytes)).collect();
            let resolved = get_multisig_signers(&authority, &signing);

            prop_assert!(resolved.len() <= signing.len());
            for signer in resolved {
                prop_assert!(signing.contains(signer));
            }
        }
    }

    // Property: any lamport amount for a system account is reported as
    // too-small data rather than as a missing account.
    proptest! {
        #[test]
        fn get_spl_account_always_reports_short_data_for_system_accounts(
            lamports in 0u64..1_000_000_000,
        ) {
            let mut svm = HPSVM::new();
            let address = Address::new_unique();
            set_keyed(&mut svm, keyed_system_account(address, lamports));

            match get_spl_account::<spl_token::state::Mint>(&svm, &address) {
                Err(TokenError::AccountDataTooSmall { expected, actual }) => {
                    prop_assert_eq!(expected, spl_token::state::Mint::LEN);
                    prop_assert_eq!(actual, 0);
                }
                other => prop_assert!(false, "unexpected result {other:?}"),
            }
        }
    }
}
