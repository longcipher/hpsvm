use solana_account::Account;
use solana_address::Address;
use solana_program_pack::Pack;
use solana_rent::Rent;
use solana_system_interface::program as system_program;

use super::{
    TOKEN_ID,
    spl_token::state::{Account as TokenAccount, AccountState, Mint},
};
use crate::HPSVM;

/// Create a system-owned account snapshot.
#[must_use]
pub fn system_account(lamports: u64) -> Account {
    Account { lamports, owner: system_program::id(), ..Default::default() }
}

/// Create a keyed system-owned account snapshot.
#[must_use]
pub fn keyed_system_account(address: Address, lamports: u64) -> (Address, Account) {
    (address, system_account(lamports))
}

/// Create an initialized SPL mint account snapshot owned by the default token program.
#[must_use]
pub fn mint_account(mint: Mint) -> Account {
    mint_account_with_program(mint, TOKEN_ID)
}

/// Create an initialized SPL mint account snapshot owned by a specific token program.
#[must_use]
pub fn mint_account_with_program(mint: Mint, token_program_id: Address) -> Account {
    let mut data = vec![0_u8; Mint::LEN];
    Mint::pack(mint, &mut data).expect("mint state should pack into a correctly sized buffer");
    Account {
        lamports: Rent::default().minimum_balance(Mint::LEN),
        data,
        owner: token_program_id,
        ..Default::default()
    }
}

/// Create a keyed initialized SPL mint account snapshot.
#[must_use]
pub fn keyed_mint_account(address: Address, mint: Mint) -> (Address, Account) {
    (address, mint_account(mint))
}

/// Create a keyed initialized SPL mint account snapshot owned by a specific token program.
#[must_use]
pub fn keyed_mint_account_with_program(
    address: Address,
    mint: Mint,
    token_program_id: Address,
) -> (Address, Account) {
    (address, mint_account_with_program(mint, token_program_id))
}

/// Create an initialized SPL token account snapshot owned by the default token program.
#[must_use]
pub fn token_account(token: TokenAccount) -> Account {
    token_account_with_program(token, TOKEN_ID)
}

/// Create an initialized SPL token account snapshot owned by a specific token program.
#[must_use]
pub fn token_account_with_program(token: TokenAccount, token_program_id: Address) -> Account {
    let mut data = vec![0_u8; TokenAccount::LEN];
    TokenAccount::pack(token, &mut data)
        .expect("token account state should pack into a correctly sized buffer");
    Account {
        lamports: Rent::default().minimum_balance(TokenAccount::LEN),
        data,
        owner: token_program_id,
        ..Default::default()
    }
}

/// Create a keyed initialized SPL token account snapshot.
#[must_use]
pub fn keyed_token_account(address: Address, token: TokenAccount) -> (Address, Account) {
    (address, token_account(token))
}

/// Create a keyed initialized SPL token account snapshot owned by a specific token program.
#[must_use]
pub fn keyed_token_account_with_program(
    address: Address,
    token: TokenAccount,
    token_program_id: Address,
) -> (Address, Account) {
    (address, token_account_with_program(token, token_program_id))
}

/// Create a keyed associated token account snapshot with the canonical ATA address.
#[must_use]
pub fn keyed_associated_token_account(
    wallet: Address,
    mint: Address,
    amount: u64,
) -> (Address, Account) {
    keyed_associated_token_account_with_program(wallet, mint, amount, TOKEN_ID)
}

/// Create a keyed associated token account snapshot for a specific token program.
#[must_use]
pub fn keyed_associated_token_account_with_program(
    wallet: Address,
    mint: Address,
    amount: u64,
    token_program_id: Address,
) -> (Address, Account) {
    let address = spl_associated_token_account_interface::address::
        get_associated_token_address_with_program_id(&wallet, &mint, &token_program_id);
    let token = TokenAccount {
        mint,
        owner: wallet,
        amount,
        state: AccountState::Initialized,
        ..Default::default()
    };
    (address, token_account_with_program(token, token_program_id))
}

/// Insert a keyed account snapshot into an HPSVM instance.
pub fn set_keyed_account(
    svm: &mut HPSVM,
    (address, account): (Address, Account),
) -> Result<(), crate::error::HPSVMError> {
    svm.set_account(address, account)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use solana_program_option::COption;
    use solana_program_pack::IsInitialized;

    use super::*;

    fn sample_mint() -> Mint {
        Mint {
            mint_authority: COption::Some(Address::new_unique()),
            supply: 1_000,
            decimals: 6,
            is_initialized: true,
            ..Default::default()
        }
    }

    fn sample_token(mint: Address) -> TokenAccount {
        TokenAccount {
            mint,
            owner: Address::new_unique(),
            amount: 42,
            delegate: COption::None,
            state: AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        }
    }

    #[test]
    fn system_account_is_owned_by_the_system_program() {
        let account = system_account(12_345);
        assert_eq!(account.lamports, 12_345);
        assert_eq!(account.owner, system_program::id());
        assert!(account.data.is_empty());
        assert!(!account.executable);
    }

    #[test]
    fn keyed_system_account_keys_the_snapshot() {
        let address = Address::new_unique();
        assert_eq!(keyed_system_account(address, 7), (address, system_account(7)));
    }

    #[test]
    fn mint_account_is_rent_exempt_and_round_trips() {
        let mint_state = sample_mint();
        let account = mint_account(mint_state);

        assert_eq!(account.owner, TOKEN_ID);
        assert_eq!(account.data.len(), Mint::LEN);
        assert_eq!(account.lamports, Rent::default().minimum_balance(Mint::LEN));
        let unpacked = Mint::unpack(&account.data).unwrap();
        assert_eq!(unpacked, mint_state);
        assert!(unpacked.is_initialized());
    }

    #[test]
    fn mint_account_with_program_uses_the_given_owner() {
        let token_program_id = Address::new_unique();
        let account = mint_account_with_program(sample_mint(), token_program_id);
        assert_eq!(account.owner, token_program_id);
        assert_eq!(account.data.len(), Mint::LEN);
    }

    #[test]
    fn keyed_mint_account_variants_key_the_snapshot() {
        let address = Address::new_unique();
        let mint = sample_mint();
        assert_eq!(keyed_mint_account(address, mint), (address, mint_account(mint)));

        let token_program_id = Address::new_unique();
        assert_eq!(
            keyed_mint_account_with_program(address, mint, token_program_id),
            (address, mint_account_with_program(mint, token_program_id))
        );
    }

    #[test]
    fn token_account_is_rent_exempt_and_round_trips() {
        let mint = Address::new_unique();
        let token_state = sample_token(mint);
        let account = token_account(token_state);

        assert_eq!(account.owner, TOKEN_ID);
        assert_eq!(account.data.len(), TokenAccount::LEN);
        assert_eq!(account.lamports, Rent::default().minimum_balance(TokenAccount::LEN));
        assert_eq!(TokenAccount::unpack(&account.data).unwrap(), token_state);
    }

    #[test]
    fn token_account_with_program_uses_the_given_owner() {
        let token_program_id = Address::new_unique();
        let account =
            token_account_with_program(sample_token(Address::new_unique()), token_program_id);
        assert_eq!(account.owner, token_program_id);
        assert_eq!(account.data.len(), TokenAccount::LEN);
    }

    #[test]
    fn keyed_token_account_variants_key_the_snapshot() {
        let address = Address::new_unique();
        let token_state = sample_token(Address::new_unique());
        assert_eq!(
            keyed_token_account(address, token_state),
            (address, token_account(token_state))
        );

        let token_program_id = Address::new_unique();
        assert_eq!(
            keyed_token_account_with_program(address, token_state, token_program_id),
            (address, token_account_with_program(token_state, token_program_id))
        );
    }

    #[test]
    fn keyed_associated_token_account_uses_the_canonical_ata_address() {
        let wallet = Address::new_unique();
        let mint = Address::new_unique();
        let (address, account) = keyed_associated_token_account(wallet, mint, 500);

        assert_eq!(
            address,
            spl_associated_token_account_interface::address::
                get_associated_token_address_with_program_id(&wallet, &mint, &TOKEN_ID)
        );
        assert_eq!(account.owner, TOKEN_ID);
        let unpacked = TokenAccount::unpack(&account.data).unwrap();
        assert_eq!(unpacked.mint, mint);
        assert_eq!(unpacked.owner, wallet);
        assert_eq!(unpacked.amount, 500);
        assert_eq!(unpacked.state, AccountState::Initialized);
    }

    #[test]
    fn keyed_associated_token_account_with_program_derives_a_program_scoped_address() {
        let wallet = Address::new_unique();
        let mint = Address::new_unique();
        let token_program_id = Address::new_unique();
        let (address, account) =
            keyed_associated_token_account_with_program(wallet, mint, 7, token_program_id);

        assert_eq!(
            address,
            spl_associated_token_account_interface::address::
                get_associated_token_address_with_program_id(&wallet, &mint, &token_program_id)
        );
        // A different token program must derive a different ATA address.
        assert_ne!(address, keyed_associated_token_account(wallet, mint, 7).0);
        assert_eq!(account.owner, token_program_id);
    }

    #[test]
    fn a_zero_amount_associated_token_account_is_still_initialized() {
        let (address, account) =
            keyed_associated_token_account(Address::new_unique(), Address::new_unique(), 0);
        let unpacked = TokenAccount::unpack(&account.data).unwrap();
        assert_eq!(unpacked.amount, 0);
        assert!(address != Address::default());
        assert!(account.lamports > 0);
    }

    #[test]
    fn set_keyed_account_installs_the_snapshot_in_the_vm() {
        let mut svm = HPSVM::new();
        let address = Address::new_unique();
        let (key, expected) = keyed_mint_account(address, sample_mint());

        set_keyed_account(&mut svm, (key, expected.clone())).unwrap();

        assert_eq!(svm.get_account(&address), Some(expected));
    }

    #[test]
    fn set_keyed_account_overwrites_a_previous_snapshot() {
        let mut svm = HPSVM::new();
        let address = Address::new_unique();

        set_keyed_account(&mut svm, keyed_system_account(address, 100)).unwrap();
        set_keyed_account(&mut svm, keyed_system_account(address, 900)).unwrap();

        assert_eq!(svm.get_account(&address).map(|a| a.lamports), Some(900));
    }

    // Property: the factories always produce rent-exempt accounts whose packed
    // data length matches the state type, for any supply / decimals / authority.
    proptest! {
        #[test]
        fn mint_factories_are_always_rent_exempt(
            supply in any::<u64>(),
            decimals in any::<u8>(),
            authority in prop::option::of(any::<[u8; 32]>()),
        ) {
            let mint = Mint {
                mint_authority: COption::from(authority.map(Address::new_from_array)),
                supply,
                decimals,
                is_initialized: true,
                ..Default::default()
            };
            let account = mint_account(mint);

            prop_assert_eq!(account.data.len(), Mint::LEN);
            prop_assert_eq!(account.lamports, Rent::default().minimum_balance(Mint::LEN));
            prop_assert_eq!(Mint::unpack(&account.data).unwrap(), mint);
        }
    }

    // Property: token account factories round-trip any amount / delegate /
    // close-authority / native combination, always rent exempt.
    proptest! {
        #[test]
        fn token_factories_round_trip_any_state(
            amount in any::<u64>(),
            delegate in prop::option::of(any::<[u8; 32]>()),
            close in prop::option::of(any::<[u8; 32]>()),
            native in prop::option::of(any::<u64>()),
        ) {
            let mint = Address::new_unique();
            let token = TokenAccount {
                mint,
                owner: Address::new_unique(),
                amount,
                delegate: COption::from(delegate.map(Address::new_from_array)),
                state: AccountState::Initialized,
                is_native: COption::from(native),
                delegated_amount: 0,
                close_authority: COption::from(close.map(Address::new_from_array)),
            };
            let account = token_account(token);

            prop_assert_eq!(account.data.len(), TokenAccount::LEN);
            prop_assert_eq!(account.lamports, Rent::default().minimum_balance(TokenAccount::LEN));
            prop_assert_eq!(TokenAccount::unpack(&account.data).unwrap(), token);
        }
    }

    // Property: the ATA address is a pure function of (wallet, mint) and always
    // differs from the wallet itself.
    proptest! {
        #[test]
        fn associated_token_addresses_are_deterministic(
            wallet in any::<[u8; 32]>(),
            mint in any::<[u8; 32]>(),
            amount in any::<u64>(),
        ) {
            let (first, account) = keyed_associated_token_account(
                Address::new_from_array(wallet),
                Address::new_from_array(mint),
                amount,
            );
            let (second, _) = keyed_associated_token_account(
                Address::new_from_array(wallet),
                Address::new_from_array(mint),
                amount,
            );

            prop_assert_eq!(first, second);
            prop_assert_ne!(first, Address::new_from_array(wallet));
            prop_assert_eq!(TokenAccount::unpack(&account.data).unwrap().amount, amount);
        }
    }
}
