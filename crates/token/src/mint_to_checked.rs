use hpsvm::{HPSVM, types::FailedTransactionMetadata};
use smallvec::{SmallVec, smallvec};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_signer::{Signer, signers::Signers};
use solana_transaction::Transaction;

use super::{
    TOKEN_ID, get_multisig_signers, get_spl_account,
    spl_token::{instruction::mint_to_checked, state::Mint},
};

/// ### Description
/// Builder for the [`mint_to_checked`] instruction.
///
/// ### Optional fields
/// - `owner`: `payer` by default.
/// - `decimals`: `mint` decimals by default.
/// - `token_program_id`: [`TOKEN_ID`] by default.
#[derive(Debug)]
pub struct MintToChecked<'a> {
    svm: &'a mut HPSVM,
    payer: &'a Keypair,
    mint: &'a Address,
    destination: &'a Address,
    token_program_id: Option<&'a Address>,
    amount: u64,
    decimals: Option<u8>,
    signers: SmallVec<[&'a Keypair; 1]>,
    owner: Option<Address>,
}

impl<'a> MintToChecked<'a> {
    /// Creates a new instance of [`mint_to_checked`] instruction.
    pub fn new(
        svm: &'a mut HPSVM,
        payer: &'a Keypair,
        mint: &'a Address,
        destination: &'a Address,
        amount: u64,
    ) -> Self {
        MintToChecked {
            svm,
            payer,
            mint,
            destination,
            token_program_id: None,
            amount,
            decimals: None,
            signers: smallvec![payer],
            owner: None,
        }
    }

    /// Sets the decimals of the burn.
    pub fn decimals(mut self, value: u8) -> Self {
        self.decimals = Some(value);
        self
    }

    /// Set the owner for the mint operation
    pub fn owner(mut self, owner: &'a Keypair) -> Self {
        self.owner = Some(owner.pubkey());
        self.signers = smallvec![owner];
        self
    }

    /// Set multisig authorization for the mint operation
    pub fn multisig(mut self, multisig: &'a Address, signers: &'a [&'a Keypair]) -> Self {
        self.owner = Some(*multisig);
        self.signers = SmallVec::from(signers);
        self
    }

    /// Sets the token program id of the mint account.
    pub fn token_program_id(mut self, program_id: &'a Address) -> Self {
        self.token_program_id = Some(program_id);
        self
    }

    /// Sends the transaction.
    pub fn send(self) -> Result<(), FailedTransactionMetadata> {
        let payer_pk = self.payer.pubkey();
        let token_program_id = self.token_program_id.unwrap_or(&TOKEN_ID);

        let authority = self.owner.unwrap_or(payer_pk);
        let signing_keys = self.signers.pubkeys();
        let signer_keys = get_multisig_signers(&authority, &signing_keys);

        let mint: Mint = get_spl_account(self.svm, self.mint)?;
        let ix = mint_to_checked(
            token_program_id,
            self.mint,
            self.destination,
            &authority,
            &signer_keys,
            self.amount,
            self.decimals.unwrap_or(mint.decimals),
        )?;

        let block_hash = self.svm.latest_blockhash();
        let mut tx = Transaction::new_with_payer(&[ix], Some(&payer_pk));
        tx.partial_sign(&[self.payer], block_hash);
        tx.partial_sign(self.signers.as_ref(), block_hash);

        self.svm.send_transaction(tx)?;

        Ok(())
    }
}
