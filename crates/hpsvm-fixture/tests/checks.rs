#![allow(missing_docs)]

use hpsvm::HPSVM;
use hpsvm_fixture::{Check, ExecutionSnapshot, ResultConfig};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_rent::Rent;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::Transaction;

#[test]
fn checks_can_assert_success_and_resulting_lamports() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );

    let snapshot = ExecutionSnapshot::from_outcome(&svm.transact(tx));
    let checks = vec![
        Check::Success,
        Check::ComputeUnits(snapshot.compute_units_consumed),
        Check::account(&recipient).lamports(64).build(),
    ];

    assert!(snapshot.run_checks(&checks, &ResultConfig { panic: false, verbose: true },));
}

#[test]
fn return_data_check_requires_return_data_to_exist() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );

    let snapshot = ExecutionSnapshot::from_outcome(&svm.transact(tx));

    assert!(snapshot.return_data.is_none());
    assert!(!snapshot.run_checks(
        &[Check::ReturnData(Vec::new())],
        &ResultConfig { panic: false, verbose: false },
    ));
}

#[test]
fn checks_use_rent_model_for_account_and_all_rent_exempt() {
    let config = ResultConfig { panic: false, verbose: true };
    let rent_exempt_minimum = Rent::default().minimum_balance(0);

    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let non_exempt_recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), rent_exempt_minimum + 10_000).unwrap();
    let non_exempt_tx = Transaction::new(
        &[&payer],
        Message::new(
            &[transfer(&payer.pubkey(), &non_exempt_recipient, 64)],
            Some(&payer.pubkey()),
        ),
        svm.latest_blockhash(),
    );

    let non_exempt_snapshot = ExecutionSnapshot::from_outcome(&svm.transact(non_exempt_tx));

    assert!(
        !non_exempt_snapshot
            .run_checks(&[Check::account(&non_exempt_recipient).rent_exempt().build()], &config,)
    );
    assert!(!non_exempt_snapshot.run_checks(&[Check::AllRentExempt], &config));

    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let exempt_recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), (rent_exempt_minimum * 2) + 100_000).unwrap();
    let exempt_tx = Transaction::new(
        &[&payer],
        Message::new(
            &[transfer(&payer.pubkey(), &exempt_recipient, rent_exempt_minimum)],
            Some(&payer.pubkey()),
        ),
        svm.latest_blockhash(),
    );

    let exempt_snapshot = ExecutionSnapshot::from_outcome(&svm.transact(exempt_tx));

    assert!(exempt_snapshot.run_checks(
        &[
            Check::account(&exempt_recipient).lamports(rent_exempt_minimum).rent_exempt().build(),
            Check::AllRentExempt,
        ],
        &config,
    ));

    let mut closed_account_snapshot = exempt_snapshot.clone();
    let closed_account = closed_account_snapshot
        .post_accounts
        .iter_mut()
        .find(|account| account.address == exempt_recipient)
        .unwrap();
    closed_account.lamports = 0;
    closed_account.data.clear();

    assert!(closed_account_snapshot.run_checks(&[Check::AllRentExempt], &config));
}

#[cfg(feature = "serde")]
#[test]
fn checks_and_account_expectations_round_trip_through_serde() {
    let address = Address::new_unique();
    let owner = Address::new_unique();
    let account_expectation_json = serde_json::json!({
        "address": address,
        "lamports": 42,
        "owner": owner,
        "executable": false,
        "data": [1, 2, 3, 4],
        "data_slice": [1, [2, 3]],
        "closed": true,
        "rent_exempt": true
    });
    let account_expectation: hpsvm_fixture::AccountExpectation =
        serde_json::from_value(account_expectation_json.clone()).unwrap();
    let checks = vec![
        Check::Success,
        Check::Included(true),
        Check::account(&address)
            .lamports(42)
            .owner(owner)
            .executable(false)
            .data(vec![1, 2, 3, 4])
            .data_slice(1, vec![2, 3])
            .closed()
            .rent_exempt()
            .build(),
        Check::AllRentExempt,
    ];

    let expectation_round_trip = serde_json::to_value(&account_expectation).unwrap();
    let json = serde_json::to_string(&checks).unwrap();
    let round_trip: Vec<Check> = serde_json::from_str(&json).unwrap();

    assert_eq!(expectation_round_trip, account_expectation_json);
    assert_eq!(round_trip, checks);
}
