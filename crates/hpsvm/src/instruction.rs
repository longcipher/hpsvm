use solana_account::Account;
use solana_address::Address;
use solana_instruction::{Instruction, account_meta::AccountMeta};

/// A single-instruction execution case with any preloaded accounts it needs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstructionCase {
    pub program_id: Address,
    pub accounts: Vec<AccountMeta>,
    pub data: Vec<u8>,
    pub pre_accounts: Vec<(Address, Account)>,
}

impl InstructionCase {
    #[must_use]
    pub fn instruction(&self) -> Instruction {
        Instruction {
            program_id: self.program_id,
            accounts: self.accounts.clone(),
            data: self.data.clone(),
        }
    }
}
