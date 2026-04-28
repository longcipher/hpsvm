use std::path::PathBuf;

use hpsvm::HPSVM;
use solana_account::{Account, ReadableAccount};
use solana_address::{Address, address};

fn read_counter_program() -> Vec<u8> {
    let mut so_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    so_path.push("test_programs/target/deploy/counter.so");
    std::fs::read(so_path).unwrap()
}

#[test]
fn accounts_view_exposes_account_and_program_reads() {
    let mut svm = HPSVM::new();
    let account_address = Address::new_unique();
    let program_id = address!("GtdambwDgHWrDJdVPBkEHGhCwokqgAoch162teUjJse2");
    let owner = Address::new_unique();
    let program_bytes = read_counter_program();

    svm.set_account(
        account_address,
        Account { lamports: 42, data: vec![1, 2, 3], owner, ..Default::default() },
    )
    .unwrap();
    svm.add_program(program_id, &program_bytes).unwrap();

    let accounts = svm.accounts();
    let account_ref = accounts
        .get_account_ref(&account_address)
        .expect("fixture account should exist in accounts view");
    assert_eq!(account_ref.lamports(), 42);
    assert_eq!(account_ref.data(), &[1, 2, 3]);

    let owned_account = accounts
        .get_account(&account_address)
        .expect("fixture account should clone from accounts view");
    assert_eq!(owned_account.lamports(), 42);
    assert_eq!(owned_account.data(), &[1, 2, 3]);

    assert_eq!(accounts.try_program_elf_bytes(&program_id).unwrap(), program_bytes.as_slice());
}
