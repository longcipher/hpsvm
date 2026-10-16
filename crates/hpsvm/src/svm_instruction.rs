//! Owned instruction-execution context for macro-declared builtins.
//!
//! [`SvmInstructionCtx`] is the handler-facing view of the currently executing
//! instruction. It carries owned copies of the program id, the instruction
//! accounts, and the instruction data, so handler bodies stay plain Rust:
//! they read and mutate `accounts`/`data` without touching RefCells,
//! borrow checkers, or transaction-context internals.
//!
//! Copies are deliberate. A builtin runs inside the SVM's
//! `InvokeContext`, whose accounts are behind runtime borrow guards; copying
//! them out (and writing the handler's mutations back afterwards) keeps the
//! macro-generated entrypoint small and the handler signature stable. The
//! copy cost is documented, not hidden: one `Vec` per account plus its data.
//!
//! [`SvmInstructionCtx::from_invoke_context`] and
//! [`SvmInstructionCtx::write_back`] are `#[doc(hidden)]` macro plumbing.
//! They are public only because the `#[svm_instruction]` expansion lives in
//! downstream crates and must call them. Hand-written code should not call
//! them directly.

use solana_address::Address;
use solana_program_runtime::invoke_context::InvokeContext;
use solana_transaction::InstructionError;
use solana_transaction_context::IndexOfAccount;

/// One instruction account, as an owned snapshot.
///
/// `lamports`, `data`, and `owner` are mutable working copies. After the
/// handler returns `Ok(())`, the generated entrypoint writes changed fields
/// back through the runtime's checked setters, so the usual builtin rules
/// apply: lamports of a read-only account cannot change, lamports of an
/// account the program does not own cannot decrease, and data resizing
/// follows the runtime's realloc policy. Violations surface as
/// [`InstructionError`] and fail the transaction exactly like a hand-written
/// builtin's would.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SvmInstructionAccount {
    /// Transaction-wide address of the account.
    pub key: Address,
    /// Whether the account signed the transaction.
    pub is_signer: bool,
    /// Whether the account may be written.
    pub is_writable: bool,
    /// Working-copy lamports.
    pub lamports: u64,
    /// Working-copy data.
    pub data: Vec<u8>,
    /// Working-copy owner.
    pub owner: Address,
    /// Transaction-wide account index, for diagnostics and CPI correlation.
    pub index_in_transaction: IndexOfAccount,
}

/// Owned handler context for one builtin-instruction invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SvmInstructionCtx {
    /// Program id of the currently executing instruction.
    pub program_id: Address,
    /// Owned snapshots of the instruction accounts, in instruction order.
    pub accounts: Vec<SvmInstructionAccount>,
    /// Owned copy of the instruction data.
    pub data: Vec<u8>,
}

impl SvmInstructionCtx {
    /// Snapshots the currently executing instruction out of the `InvokeContext`.
    ///
    /// Macro plumbing for `#[svm_instruction]`; not intended for direct use.
    /// Every borrow is released before returning, so the caller holds plain
    /// owned data with no outstanding runtime borrows.
    #[doc(hidden)]
    pub fn from_invoke_context(
        invoke_context: &InvokeContext<'_, '_>,
    ) -> Result<Self, InstructionError> {
        let instruction_context =
            invoke_context.transaction_context.get_current_instruction_context()?;
        let program_id = *instruction_context.get_program_key()?;
        let data = instruction_context.get_instruction_data().to_vec();
        let count = instruction_context.get_number_of_instruction_accounts();
        let mut accounts = Vec::with_capacity(usize::from(count));
        for index in 0..count {
            let borrowed = instruction_context.try_borrow_instruction_account(index)?;
            accounts.push(SvmInstructionAccount {
                key: *instruction_context.get_key_of_instruction_account(index)?,
                is_signer: borrowed.is_signer(),
                is_writable: borrowed.is_writable(),
                lamports: borrowed.get_lamports(),
                data: borrowed.get_data().to_vec(),
                owner: *borrowed.get_owner(),
                index_in_transaction: instruction_context
                    .get_index_of_instruction_account_in_transaction(index)?,
            });
        }
        Ok(Self { program_id, accounts, data })
    }

    /// Writes handler-mutated fields back into the running instruction.
    ///
    /// Macro plumbing for `#[svm_instruction]`; not intended for direct use.
    /// Only fields that changed are written, so read-only accounts the
    /// handler left alone never trip the runtime's write guards. Changed
    /// fields go through the same checked setters a hand-written builtin
    /// uses, preserving error behavior.
    #[doc(hidden)]
    pub fn write_back(
        &self,
        invoke_context: &mut InvokeContext<'_, '_>,
    ) -> Result<(), InstructionError> {
        let instruction_context =
            invoke_context.transaction_context.get_current_instruction_context()?;
        let count = instruction_context.get_number_of_instruction_accounts();
        for index in 0..count {
            let account =
                self.accounts.get(usize::from(index)).ok_or(InstructionError::MissingAccount)?;
            let mut borrowed = instruction_context.try_borrow_instruction_account(index)?;
            if borrowed.get_lamports() != account.lamports {
                borrowed.set_lamports(account.lamports)?;
            }
            if borrowed.get_data() != account.data.as_slice() {
                borrowed.set_data_from_slice(&account.data)?;
            }
            if *borrowed.get_owner() != account.owner {
                borrowed.set_owner(account.owner.as_ref())?;
            }
        }
        Ok(())
    }
}
