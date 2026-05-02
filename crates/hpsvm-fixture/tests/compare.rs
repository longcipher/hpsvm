#![allow(missing_docs)]

use hpsvm::HPSVM;
use hpsvm_fixture::{AccountCompareScope, Compare, ExecutionSnapshot, ResultConfig};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::Transaction;

#[test]
fn compare_can_ignore_unselected_accounts() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );

    let baseline = ExecutionSnapshot::from_outcome(&svm.transact(tx));
    let mut candidate = baseline.clone();
    let payer_account = candidate
        .post_accounts
        .iter_mut()
        .find(|account| account.address == payer.pubkey())
        .unwrap();
    payer_account.lamports = payer_account.lamports.saturating_sub(1);

    let config = ResultConfig { panic: false, verbose: true };

    assert!(baseline.compare_with(
        &candidate,
        &[Compare::Accounts(AccountCompareScope::Only(vec![recipient]))],
        &config,
    ));
    assert!(!baseline.compare_with(
        &candidate,
        &[Compare::Accounts(AccountCompareScope::All)],
        &config,
    ));
}

#[test]
fn compare_presets_only_differ_on_compute_units() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );

    let baseline = ExecutionSnapshot::from_outcome(&svm.transact(tx));
    let mut candidate = baseline.clone();
    candidate.compute_units_consumed = candidate.compute_units_consumed.saturating_add(1);

    let config = ResultConfig { panic: false, verbose: true };

    assert!(!baseline.compare_with(&candidate, &Compare::everything(), &config));
    assert!(baseline.compare_with(&candidate, &Compare::everything_but_compute_units(), &config,));
}

#[cfg(feature = "serde")]
#[test]
fn compares_and_account_scopes_round_trip_through_serde() {
    let addresses = vec![Address::new_unique(), Address::new_unique()];
    let compares = vec![
        Compare::Status,
        Compare::Accounts(AccountCompareScope::All),
        Compare::Accounts(AccountCompareScope::Only(addresses.clone())),
        Compare::Accounts(AccountCompareScope::AllExcept(addresses)),
    ];

    let json = serde_json::to_string(&compares).unwrap();
    let round_trip: Vec<Compare> = serde_json::from_str(&json).unwrap();

    assert_eq!(round_trip, compares);
}
