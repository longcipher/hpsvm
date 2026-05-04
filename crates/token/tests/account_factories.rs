//! Account snapshot factory coverage.

use hpsvm::HPSVM;
use hpsvm_token::{
    TOKEN_ID,
    accounts::{
        keyed_associated_token_account, keyed_mint_account, keyed_system_account,
        keyed_token_account,
    },
    get_spl_account,
    spl_token::state::{Account as TokenAccount, AccountState, Mint},
};
use solana_account::Account;
use solana_address::Address;
use solana_program_pack::Pack;
use solana_system_interface::program as system_program;
use solana_transaction_error::TransactionError;
use spl_associated_token_account_interface::address::get_associated_token_address_with_program_id;

#[test]
fn keyed_mint_account_returns_initialized_mint_snapshot() {
    let mint_address = Address::new_unique();
    let (address, account) = keyed_mint_account(
        mint_address,
        Mint { decimals: 6, supply: 1_000, is_initialized: true, ..Default::default() },
    );

    assert_eq!(address, mint_address);
    assert_eq!(account.owner, TOKEN_ID);
    let unpacked = Mint::unpack(&account.data).expect("mint data should unpack");
    assert_eq!(unpacked.decimals, 6);
    assert_eq!(unpacked.supply, 1_000);
}

#[test]
fn keyed_associated_token_account_derives_address_and_sets_amount() {
    let owner = Address::new_unique();
    let mint = Address::new_unique();
    let (address, account) = keyed_associated_token_account(owner, mint, 500);

    assert_eq!(address, get_associated_token_address_with_program_id(&owner, &mint, &TOKEN_ID));
    assert_eq!(account.owner, TOKEN_ID);

    let unpacked = TokenAccount::unpack(&account.data).expect("token data should unpack");
    assert_eq!(unpacked.mint, mint);
    assert_eq!(unpacked.owner, owner);
    assert_eq!(unpacked.amount, 500);
    assert_eq!(unpacked.state, AccountState::Initialized);
}

#[test]
fn keyed_system_and_token_factories_preserve_addresses() {
    let system_address = Address::new_unique();
    let token_address = Address::new_unique();
    let mint = Address::new_unique();
    let owner = Address::new_unique();

    let (system_key, system_account) = keyed_system_account(system_address, 99);
    assert_eq!(system_key, system_address);
    assert_eq!(system_account.owner, system_program::id());
    assert_eq!(system_account.lamports, 99);

    let token = TokenAccount {
        mint,
        owner,
        amount: 7,
        state: AccountState::Initialized,
        ..Default::default()
    };
    let (token_key, token_account) = keyed_token_account(token_address, token);
    assert_eq!(token_key, token_address);
    assert_eq!(token_account.owner, TOKEN_ID);
}

#[test]
fn get_spl_account_reports_short_account_data_without_panicking() {
    let mut svm = HPSVM::new();
    let account = Address::new_unique();
    svm.set_account(
        account,
        Account { lamports: 1, data: vec![0; 1], owner: TOKEN_ID, ..Default::default() },
    )
    .expect("short token account should be inserted");

    let err = get_spl_account::<TokenAccount>(&svm, &account)
        .expect_err("short account data should be reported as a transaction failure");

    assert_eq!(
        err.err,
        TransactionError::InstructionError(
            0,
            solana_instruction::error::InstructionError::AccountDataTooSmall,
        )
    );
}
