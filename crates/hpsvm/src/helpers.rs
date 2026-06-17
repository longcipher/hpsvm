use solana_account::{Account, AccountSharedData, ReadableAccount, WritableAccount};
use solana_address::Address;
use solana_instruction::Instruction;
use solana_program_pack::Pack;
use solana_rent::Rent;
use solana_transaction_context::IndexOfAccount;
use solana_transaction_error::TransactionError;
use spl_token_interface::state::{Account as TokenAccount, Mint as TokenMint};

use crate::{
    accounts_db::AccountsDb,
    types::{ExecutedInstruction, ExecutionTrace, TokenBalance},
    utils::{
        inner_instructions::inner_instructions_list_from_instruction_trace,
        rent::{check_rent_state_with_account, get_account_rent_state},
    },
};

/// Lighter version of the one in the solana-svm crate.
///
/// Check whether the payer_account is capable of paying the fee. The
/// side effect is to subtract the fee amount from the payer_account
/// balance of lamports. If the payer_account is not able to pay the
/// fee a specific error is returned.
pub(crate) fn validate_fee_payer(
    payer_address: &Address,
    payer_account: &mut AccountSharedData,
    payer_index: IndexOfAccount,
    rent: &Rent,
    fee: u64,
) -> solana_transaction_error::TransactionResult<()> {
    if payer_account.lamports() == 0 {
        tracing::error!("Payer account {payer_address} not found.");
        return Err(TransactionError::AccountNotFound);
    }
    let system_account_kind = solana_system_program::get_system_account_kind(payer_account)
        .ok_or_else(|| {
            tracing::error!("Payer account {payer_address} is not a system account");
            TransactionError::InvalidAccountForFee
        })?;
    let min_balance = match system_account_kind {
        solana_system_program::SystemAccountKind::System => 0,
        solana_system_program::SystemAccountKind::Nonce => {
            // Should we ever allow a fees charge to zero a nonce account's
            // balance. The state MUST be set to uninitialized in that case
            rent.minimum_balance(solana_nonce::state::State::size())
        }
    };

    let payer_lamports = payer_account.lamports();

    payer_lamports.checked_sub(min_balance).and_then(|v| v.checked_sub(fee)).ok_or_else(|| {
        tracing::error!(
            "Payer account {payer_address} has insufficient lamports for fee. Payer lamports: \
                {payer_lamports} min_balance: {min_balance} fee: {fee}"
        );
        TransactionError::InsufficientFundsForFee
    })?;

    let payer_len = payer_account.data().len();
    let payer_pre_rent_state = get_account_rent_state(rent, payer_account.lamports(), payer_len);
    // we already checked above if we have sufficient balance so this should never error.
    payer_account.checked_sub_lamports(fee).expect("fee should not exceed account balance");

    let payer_post_rent_state = get_account_rent_state(rent, payer_account.lamports(), payer_len);
    check_rent_state_with_account(
        &payer_pre_rent_state,
        &payer_post_rent_state,
        payer_address,
        payer_index,
    )
}

pub(crate) fn fee_payer_for_instructions(
    instructions: &[Instruction],
    fallback: Address,
) -> Address {
    instructions
        .iter()
        .flat_map(|instruction| instruction.accounts.iter())
        .find(|account| account.is_signer)
        .or_else(|| {
            instructions
                .iter()
                .flat_map(|instruction| instruction.accounts.iter())
                .find(|account| account.is_writable)
        })
        .map(|account| account.pubkey)
        .unwrap_or(fallback)
}

pub(crate) fn public_account_from_shared(account: &AccountSharedData) -> Option<Account> {
    (account.lamports() != 0).then(|| account.clone().into())
}

pub(crate) fn token_balances(
    accounts: &[(Address, AccountSharedData)],
    account_db: &AccountsDb,
) -> Vec<TokenBalance> {
    accounts
        .iter()
        .enumerate()
        .filter_map(|(account_index, (address, account))| {
            if account.data().len() != TokenAccount::LEN {
                return None;
            }

            let token_account = TokenAccount::unpack(account.data()).ok()?;
            let decimals = token_mint_decimals(accounts, account_db, &token_account.mint);

            Some(TokenBalance {
                account_index,
                address: *address,
                mint: token_account.mint,
                owner: token_account.owner,
                amount: token_account.amount,
                decimals,
            })
        })
        .collect()
}

fn token_mint_decimals(
    accounts: &[(Address, AccountSharedData)],
    account_db: &AccountsDb,
    mint: &Address,
) -> Option<u8> {
    accounts
        .iter()
        .find(|(address, _)| address == mint)
        .map(|(_, account)| account.clone())
        .or_else(|| account_db.get_account(mint))
        .and_then(|account| {
            (account.data().len() == TokenMint::LEN)
                .then(|| TokenMint::unpack(account.data()).ok().map(|mint| mint.decimals))
                .flatten()
        })
}

pub(crate) fn execution_trace_from_transaction_context(
    sanitized_tx: &solana_transaction::sanitized::SanitizedTransaction,
    transaction_context: &solana_transaction_context::TransactionContext<'_>,
) -> ExecutionTrace {
    use solana_instruction::account_meta::AccountMeta;

    let account_keys = sanitized_tx.message().account_keys();
    let instructions = (0..transaction_context.get_instruction_trace_length())
        .filter_map(|index| {
            let instruction_context =
                transaction_context.get_instruction_context_at_index_in_trace(index).ok()?;
            let program_index = instruction_context
                .get_index_of_program_account_in_transaction()
                .unwrap_or_default() as usize;
            let program_id = account_keys.get(program_index).copied().unwrap_or_default();
            let stack_height =
                u8::try_from(instruction_context.get_stack_height()).unwrap_or(u8::MAX);
            let accounts = (0..instruction_context.get_number_of_instruction_accounts())
                .filter_map(|instruction_account_index| {
                    let transaction_index = instruction_context
                        .get_index_of_instruction_account_in_transaction(instruction_account_index)
                        .ok()? as usize;
                    let pubkey = account_keys.get(transaction_index).copied()?;
                    let is_signer = instruction_context
                        .is_instruction_account_signer(instruction_account_index)
                        .unwrap_or(false);
                    let is_writable = instruction_context
                        .is_instruction_account_writable(instruction_account_index)
                        .unwrap_or(false);
                    Some(AccountMeta { pubkey, is_signer, is_writable })
                })
                .collect();

            Some(ExecutedInstruction {
                stack_height,
                program_id,
                accounts,
                data: instruction_context.get_instruction_data().to_vec(),
            })
        })
        .collect();

    ExecutionTrace { instructions }
}

pub(crate) fn execute_tx_helper(
    sanitized_tx: &solana_transaction::sanitized::SanitizedTransaction,
    ctx: solana_transaction_context::TransactionContext<'_>,
) -> (
    solana_signature::Signature,
    solana_transaction_context::TransactionReturnData,
    solana_message::inner_instruction::InnerInstructionsList,
    ExecutionTrace,
    Vec<(Address, AccountSharedData)>,
) {
    use solana_transaction_context::ExecutionRecord;

    let signature = sanitized_tx.signature().to_owned();
    let inner_instructions = inner_instructions_list_from_instruction_trace(&ctx);
    let execution_trace = execution_trace_from_transaction_context(sanitized_tx, &ctx);
    let ExecutionRecord {
        accounts,
        return_data,
        touched_account_count: _,
        accounts_resize_delta: _,
    } = ctx.into();
    let msg = sanitized_tx.message();
    let post_accounts = accounts
        .into_iter()
        .enumerate()
        .filter_map(|(idx, pair)| msg.is_writable(idx).then_some(pair))
        .collect();
    (signature, return_data, inner_instructions, execution_trace, post_accounts)
}
