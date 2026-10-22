//! Diagnostics coverage for the SPL token balances.
//!
//! The token balances are the part of the execution diagnostics that only a
//! transaction touching an SPL token account can produce. These tests pin
//! both directions: a transaction that touches a token account reports it,
//! and a transaction that touches none reports nothing.

use hpsvm::HPSVM;
use solana_account::Account;
use solana_address::Address;
use solana_keypair::Keypair;
use solana_program_pack::Pack;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::Transaction;
use spl_token_interface::state::{Account as TokenAccount, Mint as TokenMint};

/// Builds a token mint account with `decimals` decimals.
fn mint_account(decimals: u8) -> Account {
    let mut mint = TokenMint::default();
    mint.decimals = decimals;
    // The token interface refuses to unpack an uninitialized mint, so the
    // decimals of an uninitialized mint are never resolved.
    mint.is_initialized = true;
    let mut account = Account::new(1_000_000, TokenMint::LEN, &spl_token_interface::ID);
    TokenMint::pack(mint, &mut account.data).expect("a mint must pack");
    account
}

/// Builds a SPL token account holding `amount` units of `mint`.
fn token_account(mint: &Address, owner: &Address, amount: u64) -> Account {
    let mut token = TokenAccount::default();
    token.mint = *mint;
    token.owner = *owner;
    token.amount = amount;
    // The token interface refuses to unpack an uninitialized account, so the
    // balance of a closed token account is never reported.
    token.state = spl_token_interface::state::AccountState::Initialized;
    let mut account = Account::new(1_000_000, TokenAccount::LEN, &spl_token_interface::ID);
    TokenAccount::pack(token, &mut account.data).expect("a token account must pack");
    account
}

/// A transaction that writes to a token account reports the token balance,
/// both before and after the execution.
#[test]
fn a_token_account_is_reported_in_the_diagnostics() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let payer_address = payer.pubkey();
    svm.airdrop(&payer_address, 10_000_000).expect("airdrop should succeed");

    let mint_address = Address::new_unique();
    let token_address = Address::new_unique();
    svm.set_account(mint_address, mint_account(6)).expect("mint should be inserted");
    svm.set_account(token_address, token_account(&mint_address, &payer_address, 42))
        .expect("token account should be inserted");
    assert_eq!(
        svm.get_account(&token_address).expect("token account must exist").data.len(),
        TokenAccount::LEN,
        "the token account must keep its token-sized data"
    );
    assert_eq!(svm.get_account(&mint_address).expect("mint must exist").data.len(), TokenMint::LEN);

    let blockhash = svm.latest_blockhash();
    let metadata = svm
        .send_transaction(Transaction::new_signed_with_payer(
            &[transfer(&payer_address, &token_address, 1)],
            Some(&payer_address),
            &[&payer],
            blockhash,
        ))
        .expect("transaction should execute");

    let pre = &metadata.diagnostics.pre_token_balances;
    let post = &metadata.diagnostics.post_token_balances;

    assert_eq!(pre.len(), 1, "the token account must be reported before execution");
    assert_eq!(post.len(), 1, "the token account must be reported after execution");

    assert_eq!(pre[0].address, token_address);
    assert_eq!(pre[0].mint, mint_address);
    assert_eq!(pre[0].owner, payer_address);
    assert_eq!(pre[0].amount, 42);
    assert_eq!(pre[0].decimals, Some(6), "the mint's decimals must be resolved");

    assert_eq!(post[0].address, token_address);
    assert_eq!(post[0].mint, mint_address);
    assert_eq!(post[0].amount, 42, "a lamport transfer does not move tokens");
    assert_eq!(post[0].decimals, Some(6));
}

/// A transaction that touches no token account reports no token balances.
#[test]
fn a_transaction_without_token_accounts_reports_no_token_balances() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let payer_address = payer.pubkey();
    svm.airdrop(&payer_address, 10_000_000).expect("airdrop should succeed");

    let recipient = Address::new_unique();
    let blockhash = svm.latest_blockhash();
    let metadata = svm
        .send_transaction(Transaction::new_signed_with_payer(
            &[transfer(&payer_address, &recipient, 1)],
            Some(&payer_address),
            &[&payer],
            blockhash,
        ))
        .expect("transaction should execute");

    assert!(
        metadata.diagnostics.pre_token_balances.is_empty(),
        "no token account is involved, got {:?}",
        metadata.diagnostics.pre_token_balances
    );
    assert!(
        metadata.diagnostics.post_token_balances.is_empty(),
        "no token account is involved, got {:?}",
        metadata.diagnostics.post_token_balances
    );
    // The rest of the diagnostics are unaffected by the token scan.
    assert!(!metadata.diagnostics.pre_balances.is_empty());
    assert!(!metadata.diagnostics.account_diffs.is_empty());
}
