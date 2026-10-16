//! Task 6.3: `#[svm_instruction]` end-to-end verification.
//!
//! A plain function annotated with the macro must produce a working handler
//! with bound accounts/data and a correct CU charge, behaving identically to
//! a manually-implemented equivalent.

use hpsvm::{Hpsvm, SvmInstructionCtx};
use hpsvm_macros::svm_instruction;
use solana_account::Account;
use solana_address::Address;
use solana_instruction::Instruction;
use solana_transaction::{InstructionError, TransactionError};

const CHEAP_CU: u64 = 150;
const EXPENSIVE_CU: u64 = 1150;

#[svm_instruction(program_id = "84LtecYhx229KabnaiMixiuD5Tey4bBo6N4yzZ81Vzpz", compute_units = 150)]
fn move_seven(ctx: &mut SvmInstructionCtx) -> Result<(), InstructionError> {
    if ctx.data.first().is_some_and(|byte| *byte == 0xFF) {
        return Err(InstructionError::Custom(42));
    }
    let from = ctx.accounts.first_mut().ok_or(InstructionError::MissingAccount)?;
    from.lamports = from.lamports.checked_sub(7).ok_or(InstructionError::ArithmeticOverflow)?;
    let to = ctx.accounts.get_mut(1).ok_or(InstructionError::MissingAccount)?;
    to.lamports = to.lamports.checked_add(7).ok_or(InstructionError::ArithmeticOverflow)?;
    if let Some(byte) = to.data.first_mut() {
        *byte = 0xAB;
    }
    Ok(())
}

#[svm_instruction(
    program_id = "GLBtvdBQGbm9k3YGkczARZmm99SfCCv7LcF21T8EPeRW",
    compute_units = 1150
)]
fn move_seven_expensive(ctx: &mut SvmInstructionCtx) -> Result<(), InstructionError> {
    move_seven(ctx)
}

fn owned_account(owner: &Address, lamports: u64) -> Account {
    Account { lamports, data: vec![0u8; 8], owner: *owner, executable: false, rent_epoch: 0 }
}

fn setup_pair(svm: &mut Hpsvm, owner: &Address) -> (Address, Address) {
    let from = Address::new_unique();
    let to = Address::new_unique();
    svm.set_account(from, owned_account(owner, 1_000_000)).unwrap();
    svm.set_account(to, owned_account(owner, 1_000_000)).unwrap();
    (from, to)
}

fn move_instruction(
    program_id: Address,
    payer: Address,
    from: Address,
    to: Address,
    data: Vec<u8>,
) -> Instruction {
    Instruction {
        program_id,
        accounts: vec![
            solana_instruction::account_meta::AccountMeta::new(from, false),
            solana_instruction::account_meta::AccountMeta::new(to, false),
            solana_instruction::account_meta::AccountMeta::new(payer, true),
        ],
        data,
    }
}

/// A funded system-owned signer so fee collection succeeds.
///
/// The macro handler only reads accounts 0 and 1; the payer rides along as
/// account 2, exactly like a real client transaction would carry one.
fn funded_payer(svm: &mut Hpsvm) -> Address {
    let payer = Address::new_unique();
    svm.set_account(
        payer,
        Account {
            lamports: 1_000_000_000,
            data: vec![],
            owner: solana_sdk_ids::system_program::id(),
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
    payer
}

#[test]
fn macro_instruction_moves_lamports_and_charges_cu() {
    let mut svm = Hpsvm::new();
    MoveSeven::register(&mut svm);
    let program_id = MoveSeven::program_id();
    let payer = funded_payer(&mut svm);
    let (from, to) = setup_pair(&mut svm, &program_id);

    let meta =
        svm.process_instruction(move_instruction(program_id, payer, from, to, vec![])).unwrap();

    assert_eq!(svm.get_account(&from).unwrap().lamports, 1_000_000 - 7);
    assert_eq!(svm.get_account(&to).unwrap().lamports, 1_000_000 + 7);
    assert_eq!(svm.get_account(&to).unwrap().data[0], 0xAB);
    assert!(
        meta.compute_units_consumed >= CHEAP_CU,
        "expected at least {CHEAP_CU} CUs, consumed {}",
        meta.compute_units_consumed
    );
}

#[test]
fn macro_cu_charge_matches_declaration() {
    let mut svm = Hpsvm::new();
    MoveSeven::register(&mut svm);
    MoveSevenExpensive::register(&mut svm);
    let cheap_id = MoveSeven::program_id();
    let expensive_id = MoveSevenExpensive::program_id();
    assert_eq!(MoveSeven::COMPUTE_UNITS, CHEAP_CU);
    assert_eq!(MoveSevenExpensive::COMPUTE_UNITS, EXPENSIVE_CU);
    let payer = funded_payer(&mut svm);

    let (cheap_from, cheap_to) = setup_pair(&mut svm, &cheap_id);
    let cheap_meta = svm
        .process_instruction(move_instruction(cheap_id, payer, cheap_from, cheap_to, vec![]))
        .unwrap();

    let (expensive_from, expensive_to) = setup_pair(&mut svm, &expensive_id);
    let expensive_meta = svm
        .process_instruction(move_instruction(
            expensive_id,
            payer,
            expensive_from,
            expensive_to,
            vec![],
        ))
        .unwrap();

    assert_eq!(
        expensive_meta.compute_units_consumed - cheap_meta.compute_units_consumed,
        EXPENSIVE_CU - CHEAP_CU
    );
}

#[test]
fn macro_execute_matches_free_function() {
    let program_id = Address::new_unique();
    let base = SvmInstructionCtx {
        program_id,
        accounts: vec![
            hpsvm::SvmInstructionAccount {
                key: Address::new_unique(),
                is_signer: false,
                is_writable: true,
                lamports: 100,
                data: vec![0u8; 4],
                owner: program_id,
                index_in_transaction: 0,
            },
            hpsvm::SvmInstructionAccount {
                key: Address::new_unique(),
                is_signer: false,
                is_writable: true,
                lamports: 50,
                data: vec![0u8; 4],
                owner: program_id,
                index_in_transaction: 1,
            },
        ],
        data: vec![],
    };

    let mut via_free = base.clone();
    let mut via_execute = base.clone();
    assert_eq!(move_seven(&mut via_free), Ok(()));
    assert_eq!(MoveSeven::execute(&mut via_execute), Ok(()));
    assert_eq!(via_free.accounts, via_execute.accounts);

    let mut err_free = base.clone();
    err_free.data = vec![0xFF];
    let mut err_execute = err_free.clone();
    assert_eq!(err_free.accounts, err_execute.accounts);
    assert_eq!(move_seven(&mut err_free), Err(InstructionError::Custom(42)));
    assert_eq!(MoveSeven::execute(&mut err_execute), Err(InstructionError::Custom(42)));
    assert_eq!(err_free.accounts, err_execute.accounts);
}

#[test]
fn macro_error_propagates_to_transaction() {
    let mut svm = Hpsvm::new();
    MoveSeven::register(&mut svm);
    let program_id = MoveSeven::program_id();
    let payer = funded_payer(&mut svm);
    let (from, to) = setup_pair(&mut svm, &program_id);

    let failure = svm
        .process_instruction(move_instruction(program_id, payer, from, to, vec![0xFF]))
        .unwrap_err();

    assert_eq!(failure.err, TransactionError::InstructionError(0, InstructionError::Custom(42)));
    assert_eq!(svm.get_account(&from).unwrap().lamports, 1_000_000);
    assert_eq!(svm.get_account(&to).unwrap().lamports, 1_000_000);
}
