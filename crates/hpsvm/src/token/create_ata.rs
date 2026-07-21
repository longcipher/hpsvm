use solana_address::Address;
use solana_keypair::Keypair;
use solana_signer::Signer;
use spl_associated_token_account_interface::instruction::create_associated_token_account;

use super::TOKEN_ID;
use crate::{HPSVM, types::FailedTransactionMetadata};

/// ### Description
/// Builder for the [`create_associated_token_account`] instruction.
///
/// ### Optional fields
/// - `owner`: `payer` by default.
/// - `token_program_id`: [`TOKEN_ID`] by default.
#[derive(Debug)]
pub struct CreateAssociatedTokenAccount<'a> {
    svm: &'a mut HPSVM,
    payer: &'a Keypair,
    mint: &'a Address,
    token_program_id: Option<&'a Address>,
    owner: Option<Address>,
}

impl<'a> CreateAssociatedTokenAccount<'a> {
    /// Creates a new instance of [`create_associated_token_account`] instruction.
    pub fn new(svm: &'a mut HPSVM, payer: &'a Keypair, mint: &'a Address) -> Self {
        CreateAssociatedTokenAccount { svm, payer, owner: None, token_program_id: None, mint }
    }

    /// Sets the owner of the account with single owner.
    pub fn owner(mut self, owner: &'a Address) -> Self {
        self.owner = Some(*owner);
        self
    }

    /// Sets the token program id for the instruction.
    pub fn token_program_id(mut self, program_id: &'a Address) -> Self {
        self.token_program_id = Some(program_id);
        self
    }

    /// Sends the transaction.
    pub fn send(self) -> Result<Address, FailedTransactionMetadata> {
        let token_program_id = self.token_program_id.unwrap_or(&TOKEN_ID);
        let payer_pk = self.payer.pubkey();

        let authority = self.owner.unwrap_or(payer_pk);

        let ix =
            create_associated_token_account(&payer_pk, &authority, self.mint, token_program_id);

        super::sign_and_send(self.svm, self.payer, &[], ix)?;

        let ata = spl_associated_token_account_interface::address::get_associated_token_address_with_program_id(
            &authority,
            self.mint,
            token_program_id,
        );

        Ok(ata)
    }
}
