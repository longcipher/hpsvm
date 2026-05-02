use hpsvm::HPSVM;
use solana_account::Account;
use solana_address::Address;
use solana_program_pack::Pack;
use solana_rent::Rent;
use solana_system_interface::program as system_program;

use crate::{
    TOKEN_ID,
    spl_token::state::{Account as TokenAccount, AccountState, Mint},
};

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
) -> Result<(), hpsvm::error::HPSVMError> {
    svm.set_account(address, account)
}
