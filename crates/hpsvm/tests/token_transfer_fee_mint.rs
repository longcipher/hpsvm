#![cfg(all(feature = "token", feature = "token-2022"))]
//! Tests for token-2022 mint extensions on [`CreateMint`].
//!
//! A mint with a `TransferFeeConfig` could not be created through the builder
//! before: `CreateMint` allocated the base `Mint::LEN` and only sent
//! `InitializeMint2`, so the account had no room for an extension and no way to
//! set one. Tests that needed a fee-bearing mint had to hand-roll the account
//! creation, the extension initialization, and the ordering rules.

use hpsvm::{
    HPSVM,
    token::{
        CreateMint, get_spl_account,
        spl_token::{
            self,
            extension::{BaseStateWithExtensions, transfer_fee::TransferFeeConfig},
            state::Mint,
        },
    },
};
use solana_keypair::Keypair;
use solana_native_token::LAMPORTS_PER_SOL;
use solana_program_pack::Pack;
use solana_signer::Signer;

#[test]
fn transfer_fee_builder_creates_a_mint_with_the_config() {
    let svm = &mut HPSVM::new();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

    let mint_address = CreateMint::new(svm, &payer)
        .decimals(6)
        .transfer_fee(500, 1_000_000)
        .send()
        .expect("a mint with a transfer fee must be creatable");

    let mint: Mint = get_spl_account(svm, &mint_address).unwrap();
    assert!(mint.is_initialized);
    assert_eq!(mint.decimals, 6);

    let account = svm.get_account(&mint_address).expect("mint account");
    let parsed = spl_token::extension::StateWithExtensions::<Mint>::unpack(&account.data)
        .expect("mint must carry its extension");
    let config = parsed.get_extension::<TransferFeeConfig>().expect("TransferFeeConfig");
    // Both authorities are the payer: a null here would mean the builder set
    // none, and nobody could change or withdraw the fee.
    assert_eq!(config.transfer_fee_config_authority.get(), Some(payer.pubkey()));
    assert_eq!(config.withdraw_withheld_authority.get(), Some(payer.pubkey()));
    assert_eq!(u16::from(config.newer_transfer_fee.transfer_fee_basis_points), 500);
    assert_eq!(u64::from(config.newer_transfer_fee.maximum_fee), 1_000_000);
}

#[test]
fn a_plain_mint_still_has_no_extensions() {
    let svm = &mut HPSVM::new();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

    let mint_address = CreateMint::new(svm, &payer).send().expect("plain mint");
    let account = svm.get_account(&mint_address).expect("mint account");
    // A base-only token-2022 mint is the legacy 82-byte length.
    assert_eq!(account.data.len(), Mint::LEN);
}
