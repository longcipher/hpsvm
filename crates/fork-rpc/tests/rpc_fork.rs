//! Integration tests for the RPC fork account source.

use std::str::FromStr;

use hpsvm::AccountSource;
use hpsvm_fork_rpc::RpcForkSource;
use solana_address::Address;
use solana_rpc_client::{
    mock_sender::PUBKEY,
    rpc_client::{RpcClient, create_rpc_client_mocks},
};

#[test]
/// Repeated reads of the same remote account should be served from cache.
fn rpc_fork_source_serves_cached_accounts_without_refetching() {
    let client = RpcClient::new_mock_with_mocks("succeeds".to_owned(), create_rpc_client_mocks());
    let source = RpcForkSource::builder().with_client(client).with_slot(1).build();
    let key = Address::from_str(PUBKEY).unwrap();

    let first = source.get_account(&key).unwrap();
    let second = source.get_account(&key).unwrap();

    assert!(first.is_some());
    assert_eq!(first, second);
    assert_eq!(source.cache_hits() + source.cache_misses(), 2);
    assert_eq!(source.cache_misses(), 1);
}
