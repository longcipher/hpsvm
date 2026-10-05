#![cfg(feature = "token")]
//! Per-branch coverage for the SPL token instruction builders.
//!
//! The `token_one_owner` / `token_multisig` suites exercise one happy path
//! through each builder. This suite pins the branches those paths do not reach:
//!
//! - the `source` / `owner` / `token_program_id` overrides,
//! - the derived-ATA default (omitting `source` derives the canonical ATA),
//! - `account_kp` overrides,
//! - `decimals` mismatches for the checked instructions,
//! - and authority errors when the wrong signer is used.

use hpsvm::{
    HPSVM,
    token::{
        Approve, ApproveChecked, Burn, BurnChecked, CloseAccount, CreateAccount,
        CreateAssociatedTokenAccount, CreateAssociatedTokenAccountIdempotent, CreateMint,
        CreateMultisig, MintTo, MintToChecked, Revoke, SetAuthority, Transfer, TransferChecked,
        get_spl_account,
        spl_token::{instruction::AuthorityType, state::Account},
    },
};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_native_token::LAMPORTS_PER_SOL;
use solana_program_option::COption;
use solana_signer::Signer;

fn funded() -> (HPSVM, Keypair) {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), LAMPORTS_PER_SOL * 100).unwrap();
    (svm, payer)
}

fn mint_with(svm: &mut HPSVM, payer: &Keypair, owner: Address, decimals: u8) -> Address {
    CreateMint::new(svm, payer).authority(&owner).decimals(decimals).send().unwrap()
}

fn mint_payer_owned(svm: &mut HPSVM, payer: &Keypair) -> Address {
    mint_with(svm, payer, payer.pubkey(), 8)
}

fn balance(svm: &HPSVM, address: &Address) -> u64 {
    get_spl_account::<Account>(svm, address).unwrap().amount
}

fn is_instruction_error(error: &hpsvm::types::FailedTransactionMetadata) -> bool {
    matches!(error.err, solana_transaction_error::TransactionError::InstructionError(..))
}

// ---------------------------------------------------------------------
// source / destination overrides
// ---------------------------------------------------------------------

#[test]
fn transfer_derives_the_source_ata_when_source_is_omitted() {
    let (mut svm, payer) = funded();
    let mint = mint_payer_owned(&mut svm, &payer);
    // A plain token account, because the derived source is the payer's own ATA.
    let destination = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();

    // With `owner` defaulting to the payer, the derived source is the payer's ATA.
    // The derived source is the payer's ATA, which has to exist before it can
    // hold a balance.
    let source = CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint).send().unwrap();
    assert_eq!(
        source,
        spl_associated_token_account_interface::address::get_associated_token_address_with_program_id(
            &payer.pubkey(),
            &mint,
            &hpsvm::token::TOKEN_ID,
        ),
        "the builder must derive the standard associated token address"
    );
    MintTo::new(&mut svm, &payer, &mint, &source, 500).send().unwrap();

    Transfer::new(&mut svm, &payer, &mint, &destination, 200).send().unwrap();

    assert_eq!(balance(&svm, &destination), 200);
    assert_eq!(balance(&svm, &source), 300);
}

#[test]
fn transfer_honours_an_explicit_source_override() {
    let (mut svm, payer) = funded();
    let mint = mint_payer_owned(&mut svm, &payer);
    let destination = CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint).send().unwrap();

    // A second, non-derived token account used as the explicit source.
    let explicit_source = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();
    MintTo::new(&mut svm, &payer, &mint, &explicit_source, 500).send().unwrap();

    Transfer::new(&mut svm, &payer, &mint, &destination, 200)
        .source(&explicit_source)
        .send()
        .unwrap();

    assert_eq!(balance(&svm, &destination), 200);
    assert_eq!(balance(&svm, &explicit_source), 300);
}

#[test]
fn transfer_by_an_owner_other_than_the_payer_requires_that_owners_signature() {
    let (mut svm, payer) = funded();
    let new_owner = Keypair::new();
    svm.airdrop(&new_owner.pubkey(), 1_000_000_000).unwrap();
    let mint = mint_payer_owned(&mut svm, &payer);
    let source = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();
    let destination = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();
    MintTo::new(&mut svm, &payer, &mint, &source, 500).send().unwrap();

    // Hand the source over, so the payer can no longer move its tokens.
    SetAuthority::new(&mut svm, &payer, &source, AuthorityType::AccountOwner)
        .new_authority(&new_owner.pubkey())
        .send()
        .unwrap();
    let state: Account = get_spl_account(&svm, &source).unwrap();
    assert_eq!(state.owner, new_owner.pubkey());

    // The previous owner is no longer the account owner, so the transfer fails.
    let error = Transfer::new(&mut svm, &payer, &mint, &destination, 100)
        .source(&source)
        .send()
        .expect_err("the previous owner must lose its authority");
    assert!(is_instruction_error(&error), "expected an instruction error, got {:?}", error.err);

    // The new owner can move the tokens, with the payer paying the fee.
    Transfer::new(&mut svm, &payer, &mint, &destination, 100)
        .source(&source)
        .owner(&new_owner)
        .send()
        .unwrap();
    assert_eq!(balance(&svm, &destination), 100);
    assert_eq!(balance(&svm, &source), 400);
}

#[test]
fn transfer_beyond_the_balance_fails() {
    let (mut svm, payer) = funded();
    let mint = mint_payer_owned(&mut svm, &payer);
    let destination = CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint).send().unwrap();
    let source = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();
    MintTo::new(&mut svm, &payer, &mint, &source, 100).send().unwrap();

    let error = Transfer::new(&mut svm, &payer, &mint, &destination, 101)
        .source(&source)
        .send()
        .expect_err("an overdrawn transfer must fail");
    assert!(is_instruction_error(&error), "expected an instruction error, got {:?}", error.err);

    // Exactly the balance is allowed.
    Transfer::new(&mut svm, &payer, &mint, &destination, 100).source(&source).send().unwrap();
    assert_eq!(balance(&svm, &source), 0);
}

#[test]
fn transferring_zero_tokens_is_allowed() {
    let (mut svm, payer) = funded();
    let mint = mint_payer_owned(&mut svm, &payer);
    let destination = CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint).send().unwrap();
    let source = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();

    Transfer::new(&mut svm, &payer, &mint, &destination, 0).source(&source).send().unwrap();

    assert_eq!(balance(&svm, &destination), 0);
    assert_eq!(balance(&svm, &source), 0);
}

// ---------------------------------------------------------------------
// checked instructions: decimals validation
// ---------------------------------------------------------------------

#[test]
fn transfer_checked_rejects_a_decimals_mismatch() {
    let (mut svm, payer) = funded();
    let mint = mint_with(&mut svm, &payer, payer.pubkey(), 8);
    let source = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();
    MintTo::new(&mut svm, &payer, &mint, &source, 100).send().unwrap();
    let destination = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();

    // The builder defaults to the mint's own 8 decimals; only an explicit
    // mismatch is rejected by the token program.
    let error = TransferChecked::new(&mut svm, &payer, &mint, &destination, 10)
        .decimals(6)
        .source(&source)
        .send()
        .expect_err("a decimals mismatch must be rejected");
    assert!(is_instruction_error(&error), "expected an instruction error, got {:?}", error.err);

    TransferChecked::new(&mut svm, &payer, &mint, &destination, 10)
        .decimals(8)
        .source(&source)
        .send()
        .unwrap();
    assert_eq!(balance(&svm, &destination), 10);
    assert_eq!(balance(&svm, &source), 90);
}

/// `ApproveChecked` reads the decimals straight off the mint account, so it can
/// never send a mismatched value regardless of the mint's precision.
#[test]
fn approve_checked_uses_the_mint_decimals() {
    for decimals in [0u8, 1, 6, 9] {
        let (mut svm, payer) = funded();
        let mint = mint_with(&mut svm, &payer, payer.pubkey(), decimals);
        let account = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();

        ApproveChecked::new(&mut svm, &payer, &payer.pubkey(), &mint, 10)
            .source(&account)
            .send()
            .unwrap_or_else(|error| {
                panic!("decimals {decimals} must be accepted: {:?}", error.err)
            });

        let state: Account = get_spl_account(&svm, &account).unwrap();
        assert_eq!(state.delegated_amount, 10, "decimals {decimals}");
    }
}

#[test]
fn mint_to_checked_rejects_a_decimals_mismatch() {
    let (mut svm, payer) = funded();
    let mint = mint_with(&mut svm, &payer, payer.pubkey(), 9);
    let account = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();

    // The builder defaults to the mint's own decimals; only an explicit
    // mismatch is rejected.
    let error = MintToChecked::new(&mut svm, &payer, &mint, &account, 10)
        .decimals(1)
        .send()
        .expect_err("mismatch");
    assert!(is_instruction_error(&error), "expected an instruction error, got {:?}", error.err);

    MintToChecked::new(&mut svm, &payer, &mint, &account, 10).decimals(9).send().unwrap();
    assert_eq!(balance(&svm, &account), 10);
}

#[test]
fn burn_checked_rejects_a_decimals_mismatch() {
    let (mut svm, payer) = funded();
    let mint = mint_with(&mut svm, &payer, payer.pubkey(), 9);
    let account = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();
    MintTo::new(&mut svm, &payer, &mint, &account, 100).send().unwrap();

    let error = BurnChecked::new(&mut svm, &payer, &mint, &account, 10)
        .decimals(1)
        .send()
        .expect_err("mismatch");
    assert!(is_instruction_error(&error), "expected an instruction error, got {:?}", error.err);

    BurnChecked::new(&mut svm, &payer, &mint, &account, 10).decimals(9).send().unwrap();
    assert_eq!(balance(&svm, &account), 90);
}

// ---------------------------------------------------------------------
// mint configuration
// ---------------------------------------------------------------------

#[test]
fn create_mint_honours_decimals_and_freeze_authority() {
    let (mut svm, payer) = funded();
    let freezer = Keypair::new();

    let mint = CreateMint::new(&mut svm, &payer)
        .authority(&payer.pubkey())
        .freeze_authority(&freezer.pubkey())
        .decimals(3)
        .send()
        .unwrap();

    let state: hpsvm::token::spl_token::state::Mint = get_spl_account(&svm, &mint).unwrap();
    assert_eq!(state.decimals, 3);
    assert_eq!(state.mint_authority, COption::Some(payer.pubkey()));
    assert_eq!(state.freeze_authority, COption::Some(freezer.pubkey()));
    assert!(state.is_initialized);
    assert_eq!(state.supply, 0);
}

#[test]
fn create_mint_defaults_to_the_payer_as_the_mint_authority() {
    let (mut svm, payer) = funded();
    let mint = CreateMint::new(&mut svm, &payer).send().unwrap();

    let state: hpsvm::token::spl_token::state::Mint = get_spl_account(&svm, &mint).unwrap();
    assert_eq!(state.mint_authority, COption::Some(payer.pubkey()));
    assert_eq!(state.freeze_authority, COption::None);
    // The builder's documented default.
    assert_eq!(state.decimals, 8);
}

#[test]
fn set_authority_transfers_mint_authority() {
    let (mut svm, payer) = funded();
    let mint = CreateMint::new(&mut svm, &payer).send().unwrap();
    let successor = Keypair::new();

    SetAuthority::new(&mut svm, &payer, &mint, AuthorityType::MintTokens)
        .new_authority(&successor.pubkey())
        .send()
        .unwrap();

    let state: hpsvm::token::spl_token::state::Mint = get_spl_account(&svm, &mint).unwrap();
    assert_eq!(state.mint_authority, COption::Some(successor.pubkey()));

    // The old authority can no longer mint; the new one can.
    let account = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();
    let error = MintTo::new(&mut svm, &payer, &mint, &account, 1).send().expect_err("revoked");
    assert!(is_instruction_error(&error), "expected an instruction error, got {:?}", error.err);

    MintTo::new(&mut svm, &payer, &mint, &account, 1).owner(&successor).send().unwrap();
    assert_eq!(balance(&svm, &account), 1);
}

#[test]
fn set_authority_revokes_mint_authority_entirely() {
    let (mut svm, payer) = funded();
    let mint = CreateMint::new(&mut svm, &payer).send().unwrap();
    let successor = Keypair::new();

    SetAuthority::new(&mut svm, &payer, &mint, AuthorityType::MintTokens)
        .new_authority(&successor.pubkey())
        .send()
        .unwrap();

    // Omitting `new_authority` revokes it.
    SetAuthority::new(&mut svm, &payer, &mint, AuthorityType::MintTokens)
        .owner(&successor)
        .send()
        .unwrap();

    let state: hpsvm::token::spl_token::state::Mint = get_spl_account(&svm, &mint).unwrap();
    assert_eq!(state.mint_authority, COption::None);

    let account = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();
    let error = MintTo::new(&mut svm, &payer, &mint, &account, 1)
        .owner(&successor)
        .send()
        .expect_err("revoked");
    assert!(is_instruction_error(&error), "expected an instruction error, got {:?}", error.err);
}

// ---------------------------------------------------------------------
// ATA idempotency
// ---------------------------------------------------------------------

#[test]
fn create_ata_idempotent_succeeds_when_the_account_already_exists() {
    let (mut svm, payer) = funded();
    let mint = mint_payer_owned(&mut svm, &payer);
    let ata = CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint).send().unwrap();
    MintTo::new(&mut svm, &payer, &mint, &ata, 42).send().unwrap();

    // The plain creator fails on an existing account...
    let error =
        CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint).send().expect_err("exists");
    assert!(
        matches!(error.err, solana_transaction_error::TransactionError::AlreadyProcessed),
        "expected AlreadyProcessed, got {:?}",
        error.err
    );

    // ...while the idempotent variant succeeds and leaves the balance alone.
    CreateAssociatedTokenAccountIdempotent::new(&mut svm, &payer, &mint).send().unwrap();
    assert_eq!(balance(&svm, &ata), 42);
}

#[test]
fn create_ata_idempotent_creates_a_missing_account() {
    let (mut svm, payer) = funded();
    let mint = mint_payer_owned(&mut svm, &payer);

    let ata = CreateAssociatedTokenAccountIdempotent::new(&mut svm, &payer, &mint).send().unwrap();

    assert_eq!(
        ata,
        spl_associated_token_account_interface::address::get_associated_token_address_with_program_id(
            &payer.pubkey(),
            &mint,
            &hpsvm::token::TOKEN_ID,
        )
    );
    assert_eq!(balance(&svm, &ata), 0);
}

#[test]
fn create_ata_for_another_wallet_derives_that_wallet_ata() {
    let (mut svm, payer) = funded();
    let wallet = Address::new_unique();
    let mint = mint_payer_owned(&mut svm, &payer);

    let ata =
        CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint).owner(&wallet).send().unwrap();

    assert_eq!(
        ata,
        spl_associated_token_account_interface::address::get_associated_token_address_with_program_id(
            &wallet,
            &mint,
            &hpsvm::token::TOKEN_ID,
        )
    );
    assert_eq!(get_spl_account::<Account>(&svm, &ata).unwrap().owner, wallet);
}

// ---------------------------------------------------------------------
// create_account overrides
// ---------------------------------------------------------------------

#[test]
fn create_account_accepts_an_explicit_account_keypair() {
    let (mut svm, payer) = funded();
    let mint = mint_payer_owned(&mut svm, &payer);
    let account_kp = Keypair::new();
    let expected = account_kp.pubkey();

    let created =
        CreateAccount::new(&mut svm, &payer, &mint).account_kp(account_kp).send().unwrap();

    assert_eq!(created, expected);
    assert_eq!(get_spl_account::<Account>(&svm, &expected).unwrap().mint, mint);
    assert_eq!(get_spl_account::<Account>(&svm, &expected).unwrap().amount, 0);
}

#[test]
fn create_account_honours_an_explicit_owner() {
    let (mut svm, payer) = funded();
    let mint = mint_payer_owned(&mut svm, &payer);
    let owner = Address::new_unique();

    let account = CreateAccount::new(&mut svm, &payer, &mint).owner(&owner).send().unwrap();

    assert_eq!(get_spl_account::<Account>(&svm, &account).unwrap().owner, owner);
}

// ---------------------------------------------------------------------
// multisig
// ---------------------------------------------------------------------

#[test]
fn a_multisig_authority_needs_every_signer() {
    let (mut svm, payer) = funded();
    let mint = mint_payer_owned(&mut svm, &payer);
    let account = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();
    MintTo::new(&mut svm, &payer, &mint, &account, 1_000).send().unwrap();

    let signer_kp_a = Keypair::new();
    let signer_kp_b = Keypair::new();
    svm.airdrop(&signer_kp_a.pubkey(), 1_000_000_000).unwrap();
    svm.airdrop(&signer_kp_b.pubkey(), 1_000_000_000).unwrap();
    let signer_a = signer_kp_a.pubkey();
    let signer_b = signer_kp_b.pubkey();
    let multisig =
        CreateMultisig::new(&mut svm, &payer, &[&signer_a, &signer_b], 2).send().unwrap();

    let destination = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();
    let signers = [&signer_kp_a, &signer_kp_b];

    // Hand the source over to the multisig, otherwise the multisig is not its
    // owner and every multisig-authority instruction fails with `OwnerMismatch`.
    SetAuthority::new(&mut svm, &payer, &account, AuthorityType::AccountOwner)
        .new_authority(&multisig)
        .send()
        .unwrap();
    let state: Account = get_spl_account(&svm, &account).unwrap();
    assert_eq!(state.owner, multisig);

    // A single signer is not enough for a 2-of-2 multisig.
    let error = Transfer::new(&mut svm, &payer, &mint, &destination, 10)
        .source(&account)
        .multisig(&multisig, &[&signer_kp_a])
        .send()
        .expect_err("one signer is not enough");
    assert!(is_instruction_error(&error), "expected an instruction error, got {:?}", error.err);

    // Both signers succeed.
    Transfer::new(&mut svm, &payer, &mint, &destination, 10)
        .source(&account)
        .multisig(&multisig, &signers)
        .send()
        .unwrap();
    assert_eq!(balance(&svm, &destination), 10);
}

#[test]
fn a_one_of_two_multisig_succeeds_with_either_signer() {
    let (mut svm, payer) = funded();
    let mint = mint_payer_owned(&mut svm, &payer);
    let account = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();
    MintTo::new(&mut svm, &payer, &mint, &account, 1_000).send().unwrap();

    let signer_kp_a = Keypair::new();
    let signer_kp_b = Keypair::new();
    svm.airdrop(&signer_kp_a.pubkey(), 1_000_000_000).unwrap();
    svm.airdrop(&signer_kp_b.pubkey(), 1_000_000_000).unwrap();
    let signer_a = signer_kp_a.pubkey();
    let signer_b = signer_kp_b.pubkey();
    let multisig =
        CreateMultisig::new(&mut svm, &payer, &[&signer_a, &signer_b], 1).send().unwrap();
    let destination = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();

    // The multisig must own the source for its authority to be usable.
    SetAuthority::new(&mut svm, &payer, &account, AuthorityType::AccountOwner)
        .new_authority(&multisig)
        .send()
        .unwrap();

    for signer in [&signer_kp_a, &signer_kp_b] {
        Transfer::new(&mut svm, &payer, &mint, &destination, 10)
            .source(&account)
            .multisig(&multisig, &[signer])
            .send()
            .expect("one signer satisfies a 1-of-2 multisig");
    }
    assert_eq!(balance(&svm, &destination), 20);
}

#[test]
fn a_multisig_with_a_signer_quota_above_the_signer_count_fails() {
    let (mut svm, payer) = funded();
    let signer_a = Keypair::new();
    let signer_b = Keypair::new();
    let signer_a_pk = signer_a.pubkey();
    let signer_b_pk = signer_b.pubkey();

    // Asking for 3 signatures from 2 signers exceeds the multisig account size.
    let error = CreateMultisig::new(&mut svm, &payer, &[&signer_a_pk, &signer_b_pk], 3)
        .send()
        .expect_err("a quota above the signer count must be rejected");
    assert!(is_instruction_error(&error), "expected an instruction error, got {:?}", error.err);
}

#[test]
fn a_multisig_with_no_signers_fails() {
    let (mut svm, payer) = funded();
    assert!(CreateMultisig::new(&mut svm, &payer, &[], 1).send().is_err());
}

// ---------------------------------------------------------------------
// burn / close / revoke
// ---------------------------------------------------------------------

#[test]
fn burn_reduces_the_supply_and_the_balance() {
    let (mut svm, payer) = funded();
    let mint = mint_payer_owned(&mut svm, &payer);
    let account = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();
    MintTo::new(&mut svm, &payer, &mint, &account, 1_000).send().unwrap();

    Burn::new(&mut svm, &payer, &mint, &account, 400).send().unwrap();

    assert_eq!(balance(&svm, &account), 600);
    let state: hpsvm::token::spl_token::state::Mint = get_spl_account(&svm, &mint).unwrap();
    assert_eq!(state.supply, 600);
}

#[test]
fn burning_more_than_the_balance_fails() {
    let (mut svm, payer) = funded();
    let mint = mint_payer_owned(&mut svm, &payer);
    let account = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();
    MintTo::new(&mut svm, &payer, &mint, &account, 100).send().unwrap();

    assert!(Burn::new(&mut svm, &payer, &mint, &account, 101).send().is_err());
    assert_eq!(balance(&svm, &account), 100, "a failed burn must not change the balance");
}

#[test]
fn revoke_clears_the_delegate() {
    let (mut svm, payer) = funded();
    let mint = mint_payer_owned(&mut svm, &payer);
    let account = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();
    let delegate = Address::new_unique();

    Approve::new(&mut svm, &payer, &delegate, &account, 50).send().unwrap();
    let state: Account = get_spl_account(&svm, &account).unwrap();
    assert_eq!(state.delegate, COption::Some(delegate));
    assert_eq!(state.delegated_amount, 50);

    Revoke::new(&mut svm, &payer, &account).send().unwrap();

    let state: Account = get_spl_account(&svm, &account).unwrap();
    assert_eq!(state.delegate, COption::None);
    assert_eq!(state.delegated_amount, 0);
}

#[test]
fn revoking_without_a_delegate_is_a_no_op() {
    let (mut svm, payer) = funded();
    let mint = mint_payer_owned(&mut svm, &payer);
    let account = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();

    Revoke::new(&mut svm, &payer, &account).send().unwrap();

    let state: Account = get_spl_account(&svm, &account).unwrap();
    assert_eq!(state.delegate, COption::None);
    assert_eq!(state.delegated_amount, 0);
}

#[test]
fn close_account_removes_the_token_account() {
    let (mut svm, payer) = funded();
    let mint = mint_payer_owned(&mut svm, &payer);
    let source = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();
    let destination = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();
    // The account must be empty, so mint and burn back to zero.
    MintTo::new(&mut svm, &payer, &mint, &source, 100).send().unwrap();
    Burn::new(&mut svm, &payer, &mint, &source, 100).send().unwrap();

    let rent_before = svm.get_balance(&source).unwrap();
    let destination_before = svm.get_balance(&destination).unwrap();
    CloseAccount::new(&mut svm, &payer, &source, &destination).send().unwrap();

    assert!(svm.get_account(&source).is_none());
    // The closed account's rent lamports land on top of the destination's own.
    assert_eq!(svm.get_balance(&destination), Some(destination_before + rent_before));
}

#[test]
fn closing_a_non_empty_account_fails() {
    let (mut svm, payer) = funded();
    let mint = mint_payer_owned(&mut svm, &payer);
    let source = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();
    let destination = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();
    MintTo::new(&mut svm, &payer, &mint, &source, 100).send().unwrap();

    assert!(CloseAccount::new(&mut svm, &payer, &source, &destination).send().is_err());
    assert!(svm.get_account(&source).is_some(), "a failed close must not remove the account");
}

#[test]
fn closing_requires_a_zero_balance_but_tolerates_an_allowance() {
    let (mut svm, payer) = funded();
    let mint = mint_payer_owned(&mut svm, &payer);
    let source = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();
    let destination = CreateAccount::new(&mut svm, &payer, &mint).send().unwrap();
    MintTo::new(&mut svm, &payer, &mint, &source, 250).send().unwrap();

    Approve::new(&mut svm, &payer, &destination, &source, 250).send().unwrap();
    let approved: Account = get_spl_account(&svm, &source).unwrap();
    assert_eq!(approved.delegate, COption::Some(destination));
    assert_eq!(approved.delegated_amount, 250);

    // A non-zero balance blocks the close.
    let error = CloseAccount::new(&mut svm, &payer, &source, &destination)
        .send()
        .expect_err("a non-zero balance must block the close");
    assert!(is_instruction_error(&error), "expected an instruction error, got {:?}", error.err);
    assert!(svm.get_account(&source).is_some(), "a failed close must not remove the account");

    // The payer still owns the account, so it can move the tokens out itself. An
    // owner-signed transfer does not spend the delegate's allowance.
    Transfer::new(&mut svm, &payer, &mint, &destination, 250).source(&source).send().unwrap();
    assert_eq!(balance(&svm, &source), 0);
    let state: Account = get_spl_account(&svm, &source).unwrap();
    assert_eq!(state.delegated_amount, 250, "an owner transfer must not spend the allowance");

    // Only the balance matters, so the close succeeds with the allowance still
    // outstanding. A fresh blockhash avoids the duplicate-transaction rejection.
    svm.expire_blockhash();
    let rent_before = svm.get_balance(&source).unwrap();
    let destination_before = svm.get_balance(&destination).unwrap();
    CloseAccount::new(&mut svm, &payer, &source, &destination).send().unwrap();
    assert!(svm.get_account(&source).is_none());
    assert_eq!(svm.get_balance(&destination), Some(destination_before + rent_before));
}
// ---------------------------------------------------------------------
// native mint
// ---------------------------------------------------------------------

#[cfg(not(feature = "token-2022"))]
#[test]
fn the_legacy_native_mint_helper_installs_the_native_mint_account() {
    use hpsvm::token::spl_token::state::Mint;

    let (mut svm, payer) = funded();
    hpsvm::token::create_native_mint(&mut svm);

    let native_mint = hpsvm::token::spl_token::native_mint::ID;
    let state: Mint = get_spl_account(&svm, &native_mint).unwrap();
    assert!(state.is_initialized);
    assert_eq!(state.decimals, hpsvm::token::spl_token::native_mint::DECIMALS);
    assert_eq!(state.supply, 0);

    // Wrapping tokens from the native mint works afterwards.
    let ata = CreateAssociatedTokenAccount::new(&mut svm, &payer, &native_mint).send().unwrap();
    assert_eq!(balance(&svm, &ata), 0);
}
