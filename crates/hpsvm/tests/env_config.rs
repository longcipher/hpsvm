use hpsvm::HPSVM;
use solana_account::Account;
use solana_clock::Clock;
use solana_sysvar::SysvarSerialize;
use solana_sysvar_id::SysvarId;

#[test]
fn warp_to_slot_updates_block_env_and_clock_sysvar() {
    let mut svm = HPSVM::new();

    svm.warp_to_slot(42);

    assert_eq!(svm.get_sysvar::<Clock>().slot, 42);
    assert_eq!(svm.block_env().slot, 42);
}

#[test]
fn set_account_keeps_block_env_in_sync_with_clock_sysvar() {
    let mut svm = HPSVM::new();
    let mut clock = svm.get_sysvar::<Clock>();
    clock.slot = 99;

    let mut clock_account = Account::new(1, Clock::size_of(), &solana_sdk_ids::sysvar::id());
    clock_account.serialize_data(&clock).unwrap();

    svm.set_account(Clock::id(), clock_account).unwrap();

    assert_eq!(svm.get_sysvar::<Clock>().slot, 99);
    assert_eq!(svm.block_env().slot, 99);
}
