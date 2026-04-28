use std::{collections::HashMap, sync::Arc};

use hpsvm::{AccountSource, HPSVM};
use solana_account::{AccountSharedData, ReadableAccount, state_traits::StateMut};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_nonce::{
    state::{Data, State as NonceState},
    versions::Versions,
};
use solana_rent::Rent;
use solana_sdk_ids::system_program;
use solana_signer::Signer;
use solana_system_interface::instruction::{advance_nonce_account, transfer};
use solana_transaction::Transaction;

#[derive(Clone, Default)]
struct StaticAccountSource {
    accounts: Arc<HashMap<Address, AccountSharedData>>,
}

impl AccountSource for StaticAccountSource {
    fn get_account(
        &self,
        pubkey: &Address,
    ) -> Result<Option<AccountSharedData>, hpsvm::AccountSourceError> {
        Ok(self.accounts.get(pubkey).cloned())
    }
}

fn data_from_state(state: &NonceState) -> &Data {
    match state {
        NonceState::Uninitialized => panic!("expecting initialized nonce state"),
        NonceState::Initialized(data) => data,
    }
}

fn data_from_account<T: ReadableAccount + StateMut<Versions>>(account: &T) -> Data {
    let versions = StateMut::<Versions>::state(account).unwrap();
    data_from_state(&NonceState::from(versions).clone()).clone()
}

#[test]
fn vm_reads_missing_accounts_from_the_configured_source() {
    let address = Address::new_unique();
    let account = AccountSharedData::new(77, 0, &Address::new_unique());
    let source = StaticAccountSource { accounts: Arc::new(HashMap::from([(address, account)])) };

    let svm = HPSVM::default().with_account_source(source);

    assert_eq!(svm.get_account(&address).unwrap().lamports, 77);
}

#[test]
fn durable_nonce_transactions_can_use_source_backed_nonce_accounts() {
    let from_keypair = Keypair::new();
    let from = from_keypair.pubkey();
    let to = Address::new_unique();
    let nonce_kp = Keypair::new();

    let mut source_builder = HPSVM::new();
    source_builder.airdrop(&from, 1_000_000_000).unwrap();
    source_builder.airdrop(&to, 1_000_000_000).unwrap();

    let create_nonce_ixns = solana_system_interface::instruction::create_nonce_account(
        &from,
        &nonce_kp.pubkey(),
        &from,
        1_500_000,
    );
    let create_nonce_tx = Transaction::new(
        &[&from_keypair, &nonce_kp],
        Message::new_with_blockhash(
            &create_nonce_ixns,
            Some(&from),
            &source_builder.latest_blockhash(),
        ),
        source_builder.latest_blockhash(),
    );
    source_builder.send_transaction(create_nonce_tx).unwrap();

    let nonce_account = source_builder.get_account(&nonce_kp.pubkey()).unwrap().into();
    let nonce = data_from_account(&nonce_account).blockhash();
    let source = StaticAccountSource {
        accounts: Arc::new(HashMap::from([(nonce_kp.pubkey(), nonce_account)])),
    };

    let mut svm = HPSVM::new().with_account_source(source);
    svm.airdrop(&from, 1_000_000_000).unwrap();
    svm.airdrop(&to, 1_000_000_000).unwrap();

    let transfer_ix = transfer(&from, &to, 1);
    let advance_ix = advance_nonce_account(&nonce_kp.pubkey(), &from);
    let msg = Message::new_with_blockhash(&[advance_ix, transfer_ix], Some(&from), &nonce);
    let tx_using_nonce = Transaction::new(&[&from_keypair], msg, nonce);

    svm.expire_blockhash();

    svm.send_transaction(tx_using_nonce).unwrap();
}

#[test]
fn rent_checks_use_source_backed_pre_state_for_writable_accounts() {
    let payer = Keypair::new();
    let from = Keypair::new();
    let to = Address::new_unique();

    let mut svm = HPSVM::new();
    let rent_exempt_minimum = svm.get_sysvar::<Rent>().minimum_balance(0);
    let source_account = AccountSharedData::new(rent_exempt_minimum - 10, 0, &system_program::id());
    let source = StaticAccountSource {
        accounts: Arc::new(HashMap::from([(from.pubkey(), source_account)])),
    };

    svm = svm.with_account_source(source);
    svm.airdrop(&payer.pubkey(), 1_000_000_000).unwrap();
    svm.airdrop(&to, 1_000_000_000).unwrap();

    let transfer_ix = transfer(&from.pubkey(), &to, 1);
    let tx = Transaction::new(
        &[&payer, &from],
        Message::new_with_blockhash(&[transfer_ix], Some(&payer.pubkey()), &svm.latest_blockhash()),
        svm.latest_blockhash(),
    );

    svm.send_transaction(tx).unwrap();
}
