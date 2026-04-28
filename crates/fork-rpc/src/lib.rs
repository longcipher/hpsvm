//! RPC-backed account source support for `hpsvm`.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use hpsvm::{AccountSource, AccountSourceError};
use parking_lot::Mutex;
use solana_account::AccountSharedData;
use solana_address::Address;
use solana_rpc_client::rpc_client::RpcClient;
use solana_rpc_client_api::config::RpcAccountInfoConfig;

/// Read-through account source backed by a Solana RPC endpoint and a local cache.
#[derive(Clone)]
pub struct RpcForkSource {
    client: Arc<RpcClient>,
    slot: u64,
    cache: Arc<Mutex<HashMap<Address, AccountSharedData>>>,
    cache_hits: Arc<AtomicUsize>,
    cache_misses: Arc<AtomicUsize>,
}

impl std::fmt::Debug for RpcForkSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcForkSource")
            .field("client", &"RpcClient")
            .field("slot", &self.slot)
            .field("cache_len", &self.cache.lock().len())
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
        #[expect(deprecated)]
        self.client
            .get_account_with_config(
                pubkey,
                RpcAccountInfoConfig {
                    min_context_slot: Some(self.slot),
                    ..RpcAccountInfoConfig::default()
                },
            )
            .map(|response| response.value.map(Into::into))
            .map_err(|error| AccountSourceError::new(error.to_string()))
    }
}

impl AccountSource for RpcForkSource {
    fn get_account(
        &self,
        pubkey: &Address,
    ) -> Result<Option<AccountSharedData>, AccountSourceError> {
        if let Some(account) = self.cache.lock().get(pubkey).cloned() {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(Some(account));
        }

        self.cache_misses.fetch_add(1, Ordering::Relaxed);
        let account = self.fetch_account(pubkey)?;
        if let Some(account) = &account {
            self.cache.lock().insert(*pubkey, account.clone());
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
}

impl std::fmt::Debug for RpcForkSourceBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcForkSourceBuilder")
            .field("rpc_url", &self.rpc_url)
            .field("client", &self.client.as_ref().map(|_| "RpcClient"))
            .field("slot", &self.slot)
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
    pub fn with_slot(mut self, slot: u64) -> Self {
        self.slot = Some(slot);
        self
    }

    /// Builds the configured RPC-backed account source.
    pub fn build(self) -> RpcForkSource {
        let client = self.client.unwrap_or_else(|| {
            Arc::new(RpcClient::new(
                self.rpc_url.unwrap_or_else(|| "http://127.0.0.1:8899".to_owned()),
            ))
        });

        RpcForkSource {
            client,
            slot: self.slot.unwrap_or_default(),
            cache: Arc::new(Mutex::new(HashMap::new())),
            cache_hits: Arc::new(AtomicUsize::new(0)),
            cache_misses: Arc::new(AtomicUsize::new(0)),
        }
    }
}
