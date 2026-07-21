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
