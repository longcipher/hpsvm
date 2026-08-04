//! RPC-backed account source for forking live cluster state.
//!
//! Enabled by the `fork` feature. Provides [`RpcForkSource`], a read-through
//! [`AccountSource`] that fetches accounts from a Solana RPC endpoint and
//! memoizes them in a bounded, TTL-aware local cache.

use std::{
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use lru::LruCache;
use parking_lot::Mutex;
use solana_account::AccountSharedData;
use solana_address::Address;
use solana_rpc_client::rpc_client::RpcClient;
use solana_rpc_client_api::config::CommitmentConfig;

use crate::account_source::{AccountSource, AccountSourceError};

/// Default maximum number of accounts retained in the local cache.
const DEFAULT_MAX_CAPACITY: usize = 10_000;

/// Default time-to-live for cached accounts.
const DEFAULT_TTL: Duration = Duration::from_secs(3600);

/// A cached account together with the instant at which it was fetched.
struct CachedAccount {
    fetched_at: Instant,
    account: AccountSharedData,
}

/// Bounded LRU cache with per-entry time-to-live.
///
/// Backed by [`lru::LruCache`] so eviction is O(1). Expiry is evaluated lazily
/// on read, which is sufficient for a fork-time account cache: entries are only
/// observable through [`Self::get`], and stale entries are replaced in place by
/// the subsequent [`Self::insert`].
struct AccountCache {
    entries: Mutex<LruCache<Address, CachedAccount>>,
    ttl: Duration,
}

impl AccountCache {
    fn new(max_capacity: NonZeroUsize, ttl: Duration) -> Self {
        Self { entries: Mutex::new(LruCache::new(max_capacity)), ttl }
    }

    fn get(&self, pubkey: &Address) -> Option<AccountSharedData> {
        let mut entries = self.entries.lock();
        let entry = entries.get(pubkey)?;
        if entry.fetched_at.elapsed() >= self.ttl {
            entries.pop(pubkey);
            return None;
        }
        Some(entry.account.clone())
    }

    fn insert(&self, pubkey: Address, account: AccountSharedData) {
        self.entries.lock().put(pubkey, CachedAccount { fetched_at: Instant::now(), account });
    }

    fn len(&self) -> usize {
        self.entries.lock().len()
    }
}

/// Read-through account source backed by a Solana RPC endpoint and a local cache.
#[derive(Clone)]
pub struct RpcForkSource {
    client: Arc<RpcClient>,
    slot: u64,
    cache: Arc<AccountCache>,
    cache_hits: Arc<AtomicUsize>,
    cache_misses: Arc<AtomicUsize>,
}

impl std::fmt::Debug for RpcForkSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcForkSource")
            .field("client", &"RpcClient")
            .field("slot", &self.slot)
            .field("cache_len", &self.cache.len())
            .field("cache_hits", &self.cache_hits())
            .field("cache_misses", &self.cache_misses())
            .finish()
    }
}

impl RpcForkSource {
    /// Creates a builder for an RPC-backed account source.
    pub fn builder() -> RpcForkSourceBuilder {
        RpcForkSourceBuilder::default()
    }

    /// Returns the number of reads served directly from the local cache.
    pub fn cache_hits(&self) -> usize {
        self.cache_hits.load(Ordering::Relaxed)
    }

    /// Returns the number of cache misses that triggered an RPC read.
    pub fn cache_misses(&self) -> usize {
        self.cache_misses.load(Ordering::Relaxed)
    }

    /// Returns the minimum RPC context slot used for fetches.
    pub const fn slot(&self) -> u64 {
        self.slot
    }

    fn fetch_account(
        &self,
        pubkey: &Address,
    ) -> Result<Option<AccountSharedData>, AccountSourceError> {
        self.client
            .get_account_with_commitment(pubkey, CommitmentConfig::confirmed())
            .map(|response| response.value.map(Into::into))
            .map_err(|error| AccountSourceError::new(error.to_string()))
    }
}

impl AccountSource for RpcForkSource {
    #[cfg_attr(feature = "hotpath", hotpath::measure)]
    fn get_account(
        &self,
        pubkey: &Address,
    ) -> Result<Option<AccountSharedData>, AccountSourceError> {
        if let Some(account) = self.cache.get(pubkey) {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(Some(account));
        }

        self.cache_misses.fetch_add(1, Ordering::Relaxed);
        let account = self.fetch_account(pubkey)?;
        if let Some(account) = &account {
            self.cache.insert(*pubkey, account.clone());
        }
        Ok(account)
    }
}

/// Builder for [`RpcForkSource`].
#[derive(Default)]
pub struct RpcForkSourceBuilder {
    rpc_url: Option<String>,
    client: Option<Arc<RpcClient>>,
    slot: Option<u64>,
    max_capacity: Option<usize>,
    ttl: Option<Duration>,
}

impl std::fmt::Debug for RpcForkSourceBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcForkSourceBuilder")
            .field("rpc_url", &self.rpc_url)
            .field("client", &self.client.as_ref().map(|_| "RpcClient"))
            .field("slot", &self.slot)
            .field("max_capacity", &self.max_capacity)
            .field("ttl", &self.ttl)
            .finish()
    }
}

impl RpcForkSourceBuilder {
    /// Configures the RPC URL used when no explicit client is provided.
    pub fn with_rpc_url(mut self, rpc_url: impl Into<String>) -> Self {
        self.rpc_url = Some(rpc_url.into());
        self
    }

    /// Reuses an existing RPC client, including mock clients in tests.
    pub fn with_client(mut self, client: RpcClient) -> Self {
        self.client = Some(Arc::new(client));
        self
    }

    /// Sets the minimum context slot used for remote account reads.
    pub const fn with_slot(mut self, slot: u64) -> Self {
        self.slot = Some(slot);
        self
    }

    /// Sets the maximum number of entries in the cache. Defaults to 10,000.
    ///
    /// A value of `Some(0)` is clamped to a capacity of 1.
    pub const fn with_max_capacity(mut self, max_capacity: Option<usize>) -> Self {
        self.max_capacity = max_capacity;
        self
    }

    /// Sets the time-to-live for cache entries. Defaults to one hour.
    pub const fn with_ttl(mut self, ttl: Option<Duration>) -> Self {
        self.ttl = ttl;
        self
    }

    /// Builds the configured RPC-backed account source.
    pub fn build(self) -> RpcForkSource {
        let client = self.client.unwrap_or_else(|| {
            Arc::new(RpcClient::new(
                self.rpc_url.unwrap_or_else(|| "http://127.0.0.1:8899".to_owned()),
            ))
        });

        let max_capacity = NonZeroUsize::new(self.max_capacity.unwrap_or(DEFAULT_MAX_CAPACITY))
            .unwrap_or(NonZeroUsize::MIN);
        let cache = Arc::new(AccountCache::new(max_capacity, self.ttl.unwrap_or(DEFAULT_TTL)));

        RpcForkSource {
            client,
            slot: self.slot.unwrap_or_default(),
            cache,
            cache_hits: Arc::new(AtomicUsize::new(0)),
            cache_misses: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use solana_account::WritableAccount;

    use super::*;

    fn account_with_lamports(lamports: u64) -> AccountSharedData {
        let mut account = AccountSharedData::default();
        account.set_lamports(lamports);
        account
    }

    #[test]
    fn cache_returns_inserted_account_within_ttl() {
        let cache = AccountCache::new(NonZeroUsize::MIN, Duration::from_secs(60));
        let key = Address::new_unique();

        cache.insert(key, account_with_lamports(7));

        assert_eq!(cache.get(&key), Some(account_with_lamports(7)));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_expires_entries_past_ttl() {
        let cache = AccountCache::new(NonZeroUsize::MIN, Duration::ZERO);
        let key = Address::new_unique();

        cache.insert(key, account_with_lamports(7));

        assert_eq!(cache.get(&key), None);
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn cache_evicts_least_recently_used_entry_at_capacity() {
        let cache = AccountCache::new(NonZeroUsize::MIN, Duration::from_secs(60));
        let first = Address::new_unique();
        let second = Address::new_unique();

        cache.insert(first, account_with_lamports(1));
        cache.insert(second, account_with_lamports(2));

        assert_eq!(cache.get(&first), None);
        assert_eq!(cache.get(&second), Some(account_with_lamports(2)));
    }
}
