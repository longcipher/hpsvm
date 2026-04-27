use hpsvm::{HPSVM, types::FailedTransactionMetadata};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;

use super::{TOKEN_ID, spl_token::instruction::sync_native};

/// ### Description
/// Builder for the [`sync_native`] instruction.
///
/// ### Optional fields
/// - `token_program_id`: [`TOKEN_ID`] by default.
#[derive(Debug)]
pub struct SyncNative<'a> {
    svm: &'a mut HPSVM,
    payer: &'a Keypair,
    account: &'a Address,
    token_program_id: Option<&'a Address>,
}

impl<'a> SyncNative<'a> {
    /// Creates a new instance of [`sync_native`] instruction.
    pub fn new(svm: &'a mut HPSVM, payer: &'a Keypair, account: &'a Address) -> Self {
        SyncNative { svm, payer, account, token_program_id: None }
    }

    /// Sets the token program id for the instruction.
    pub fn token_program_id(mut self, program_id: &'a Address) -> Self {
        self.token_program_id = Some(program_id);
        self
    }

    /// Sends the transaction.
    pub fn send(self) -> Result<(), FailedTransactionMetadata> {
        let token_program_id = self.token_program_id.unwrap_or(&TOKEN_ID);

        let ix = sync_native(token_program_id, self.account)?;

        let block_hash = self.svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&self.payer.pubkey()),
            &[self.payer],
            block_hash,
        );
        self.svm.send_transaction(tx)?;

        Ok(())
    }
}
