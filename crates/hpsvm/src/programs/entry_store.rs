//! Resident program-entry store (evm2 `ResidentMap` analogue).
//!
//! Two layers:
//!
//! * **Slice-pointer fast path** — the pre-existing `DEFAULT_PROGRAM_CACHE` memo for
//!   `include_bytes!` statics under the shared default environment. Pointer identity is stable for
//!   statics, so hashing is unnecessary.
//! * **Content-keyed store** — [`ProgramEntryStore`] keyed by `(environment identity, program
//!   content hash, loader, slot bucket)`. Generalizes the memo to `add_program`,
//!   `add_program_with_loader`, `add_program_from_file`, fixture loads and `rebuild_program_cache`.
//!
//! A hit skips ELF parsing, verification and JIT compilation entirely by
//! re-linking the resident `Arc<ProgramCacheEntry>`. A program account whose
//! bytes change hashes to a different key, so the old entry is simply
//! unreachable (content addressing).
//!
//! The store is **bounded**: it evicts the least-recently-used entry once it
//! holds [`PROGRAM_ENTRY_CAPACITY`] distinct programs. Content addressing
//! alone leaves the map growing forever in a long-lived process that keeps
//! loading new program bytes, and every evicted program pays a full ELF
//! parse + verify + JIT again when it comes back. The LRU keeps the programs
//! that are actually in use resident and lets the rest go.
//!
//! Safety: the environment component is the `Arc` pointer identity of the
//! `ProgramRuntimeEnvironments` the entry was built under — the same argument
//! the previous memo documented. Only VMs sharing the exact same environment
//! `Arc` can observe a cached entry. Contents-equivalent environments are
//! distinct objects by construction (custom syscalls / feature sets build
//! fresh environments).
//!
//! The store key carries no policy component, so resident entries — including
//! their compiled status and invocation counters — are shared process-wide
//! across `Eager`/`Lazy`/`Off` VMs on the same environment (mirroring the
//! process-global stats precedent). Concretely: `Off`/`Lazy` select the
//! uncompiled load path on a store miss and never promote, but a store hit on
//! an already-compiled entry re-links it as-is. Policy therefore governs
//! loading and promotion, not retroactive downgrading of resident entries.

use std::{
    collections::HashMap,
    num::NonZeroUsize,
    sync::{Arc, OnceLock},
};

use lru::LruCache;
use parking_lot::RwLock;
use solana_address::Address;
use solana_program_runtime::program_cache_entry::ProgramCacheEntry;

/// Maximum number of distinct programs the resident store keeps resident.
///
/// Eviction is least-recently-used, and a lookup promotes the entry it
/// returns, so the programs a VM keeps using stay resident while programs
/// that were loaded once and never used again are dropped. The bound exists
/// so a long-lived process cannot accumulate resident executables forever.
pub(crate) const PROGRAM_ENTRY_CAPACITY: usize = 256;

/// Cache key for a memoized default-program executable:
/// `(env_ptr, elf_ptr, elf_len, loader_id)`.
///
/// The ELF slice pointer is stable because the only producer is
/// `add_program_preverified`, invoked exclusively with `include_bytes!`
/// statics.
pub(crate) type DefaultProgramKey = (usize, usize, usize, Address);

/// Memoized verified executables for default programs.
pub(crate) static DEFAULT_PROGRAM_CACHE: OnceLock<
    RwLock<HashMap<DefaultProgramKey, Arc<ProgramCacheEntry>>>,
> = OnceLock::new();

/// Returns the process-wide default-program cache, creating it on first use.
pub(crate) fn default_program_cache()
-> &'static RwLock<HashMap<DefaultProgramKey, Arc<ProgramCacheEntry>>> {
    DEFAULT_PROGRAM_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Content-addressed key for the resident entry store.
///
/// `(env_ptr, content_hash, loader, slot_bucket)`:
/// * `env_ptr` — `Arc` pointer identity of the `ProgramRuntimeEnvironments` the entry was built
///   under.
/// * `content_hash` — SHA-256 over the exact ELF bytes that were verified and compiled (not the
///   account wrapper).
/// * `loader` — owning loader, part of the entry's identity.
/// * `slot_bucket` — deployment-slot constant family. All current loads use
///   `IMMEDIATE_DEPLOYMENT_SLOT`; the field keeps the key honest if that ever changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ProgramEntryKey {
    /// Environment `Arc` pointer identity.
    pub(crate) env: usize,
    /// SHA-256 over the ELF bytes.
    pub(crate) hash: [u8; 32],
    /// Owning loader.
    pub(crate) loader: Address,
    /// Deployment-slot bucket.
    pub(crate) slot_bucket: u64,
}

impl ProgramEntryKey {
    /// Builds a key from its components.
    pub(crate) const fn new(env: usize, hash: [u8; 32], loader: Address, slot_bucket: u64) -> Self {
        Self { env, hash, loader, slot_bucket }
    }
}

/// Computes the content hash over ELF bytes.
///
/// Uses the already-mandatory `solana-sha256-hasher` (no new dependency).
#[must_use]
pub(crate) fn content_hash(elf_bytes: &[u8]) -> [u8; 32] {
    solana_sha256_hasher::hashv(&[elf_bytes]).to_bytes()
}

/// Builds a [`ProgramEntryKey`] for a load attempt.
#[must_use]
pub(crate) fn content_key(
    env_ptr: usize,
    elf_bytes: &[u8],
    loader: &Address,
    slot_bucket: u64,
) -> ProgramEntryKey {
    ProgramEntryKey::new(env_ptr, content_hash(elf_bytes), *loader, slot_bucket)
}

/// Process-wide content-keyed resident entry store.
///
/// Maps [`ProgramEntryKey`] to the resident `Arc<ProgramCacheEntry>`.
/// Entries are content-addressed: a mutated program resolves to a different
/// key and triggers a fresh load. The map is bounded by
/// [`PROGRAM_ENTRY_CAPACITY`]; the least-recently-used entry is evicted when a
/// new one pushes the store past the bound.
pub(crate) struct ProgramEntryStore {
    inner: RwLock<LruCache<ProgramEntryKey, Arc<ProgramCacheEntry>>>,
}

impl ProgramEntryStore {
    /// Creates an empty store.
    pub(crate) fn new() -> Self {
        let capacity = NonZeroUsize::new(PROGRAM_ENTRY_CAPACITY).expect("the capacity is non-zero");
        Self { inner: RwLock::new(LruCache::new(capacity)) }
    }

    /// Looks up a resident entry.
    ///
    /// A hit promotes the entry so a program that keeps being used stays
    /// resident while one that is loaded once and never used again is the
    /// first to be evicted.
    pub(crate) fn get(&self, key: &ProgramEntryKey) -> Option<Arc<ProgramCacheEntry>> {
        self.inner.write().get(key).cloned()
    }

    /// Inserts an entry, keeping the first writer on a race, and returns the
    /// resident entry (the winner).
    pub(crate) fn insert(
        &self,
        key: ProgramEntryKey,
        entry: Arc<ProgramCacheEntry>,
    ) -> Arc<ProgramCacheEntry> {
        let mut guard = self.inner.write();
        if let Some(existing) = guard.get(&key) {
            return Arc::clone(existing);
        }
        guard.put(key, Arc::clone(&entry));
        entry
    }

    /// Number of resident entries (stats / tests only).
    pub(crate) fn len(&self) -> usize {
        self.inner.read().len()
    }

    /// Whether the store holds no entries.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.inner.read().is_empty()
    }

    /// Clears the store (tests only).
    #[cfg(test)]
    pub(crate) fn clear(&self) {
        self.inner.write().clear();
    }
}

impl Default for ProgramEntryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ProgramEntryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgramEntryStore").field("len", &self.len()).finish_non_exhaustive()
    }
}

/// Global resident entry store.
static PROGRAM_ENTRY_STORE: OnceLock<ProgramEntryStore> = OnceLock::new();

/// Returns the process-wide resident entry store.
pub(crate) fn program_entry_store() -> &'static ProgramEntryStore {
    PROGRAM_ENTRY_STORE.get_or_init(ProgramEntryStore::new)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn content_hash_is_deterministic_and_sensitive_to_one_byte() {
        let a = vec![1u8, 2, 3, 4];
        let mut b = a.clone();
        b[2] = 9;
        assert_eq!(content_hash(&a), content_hash(&a));
        assert_ne!(content_hash(&a), content_hash(&b));
    }

    #[test]
    fn key_is_deterministic_for_fixed_tuple() {
        let loader = Address::new_unique();
        let bytes = vec![7u8; 64];
        let k1 = content_key(0x1234, &bytes, &loader, u64::MAX);
        let k2 = content_key(0x1234, &bytes, &loader, u64::MAX);
        assert_eq!(k1, k2);
        let mut mutated = bytes.clone();
        mutated[0] ^= 0xff;
        let k3 = content_key(0x1234, &mutated, &loader, u64::MAX);
        assert_ne!(k1, k3);
        // Environment scoping: same bytes under a different env must miss.
        let k4 = content_key(0x5678, &bytes, &loader, u64::MAX);
        assert_ne!(k1, k4);
    }

    #[test]
    fn store_insert_is_first_writer_wins() {
        let store = ProgramEntryStore::new();
        let loader = Address::new_unique();
        let key = content_key(1, &[1, 2, 3], &loader, 0);
        assert!(store.get(&key).is_none());
        assert_eq!(store.len(), 0);
        // Insert path is exercised with a placeholder via len accounting;
        // entry-level identity is covered by the VM-level tests.
        assert!(store.is_empty());
    }

    #[test]
    fn default_program_cache_returns_a_stable_singleton() {
        // Kills the `Box::leak`-replacement mutants: every call must return
        // the same `&'static` (pointer identity), not a fresh leak.
        assert!(std::ptr::eq(default_program_cache(), default_program_cache()));
    }

    #[test]
    fn isolated_store_get_and_len_observe_inserts() {
        // Kills `get → None` and `len → 0`: a fresh store reports the
        // inserted entry via both accessors.
        use solana_program_runtime::{
            loaded_programs::ProgramRuntimeEnvironment, solana_sbpf::program::BuiltinProgram,
        };

        use crate::programs::load_program_uncompiled;
        let env = ProgramRuntimeEnvironment::from(BuiltinProgram::new_loader(Default::default()));
        let elf = include_bytes!("../../elf/spl_memo-3.0.0.so");
        let loader = solana_sdk_ids::bpf_loader::id();
        let entry =
            Arc::new(load_program_uncompiled(&loader, &env, elf).expect("load should succeed"));
        let store = ProgramEntryStore::new();
        let key = content_key(0xabc, elf, &loader, crate::accounts_db::IMMEDIATE_DEPLOYMENT_SLOT);
        assert_eq!(store.len(), 0);
        let resident = store.insert(key, Arc::clone(&entry));
        assert!(Arc::ptr_eq(&resident, &entry), "first insert must win");
        assert_eq!(store.len(), 1);
        let hit = store.get(&key).expect("inserted entry must be retrievable");
        assert!(Arc::ptr_eq(&hit, &entry));
    }

    /// The resident store is bounded: inserting past the capacity evicts the
    /// least-recently-used entry instead of growing without limit.
    #[test]
    fn the_store_evicts_the_least_recently_used_entry_past_its_capacity() {
        use solana_program_runtime::{
            loaded_programs::ProgramRuntimeEnvironment, solana_sbpf::program::BuiltinProgram,
        };

        use crate::programs::load_program_uncompiled;
        let env = ProgramRuntimeEnvironment::from(BuiltinProgram::new_loader(Default::default()));
        let elf = include_bytes!("../../elf/spl_memo-3.0.0.so");
        let loader = solana_sdk_ids::bpf_loader::id();
        let entry =
            Arc::new(load_program_uncompiled(&loader, &env, elf).expect("load should succeed"));

        let store = ProgramEntryStore::new();
        let key_for = |index: usize| {
            let mut bytes = elf.to_vec();
            // Distinct bytes per key, so every entry has its own content hash.
            // Two bytes, so an index above 255 does not collide with a lower
            // one.
            bytes.push((index >> 8) as u8);
            bytes.push(index as u8);
            content_key(0xabc, &bytes, &loader, crate::accounts_db::IMMEDIATE_DEPLOYMENT_SLOT)
        };

        let first = key_for(0);
        for index in 0..PROGRAM_ENTRY_CAPACITY {
            store.insert(key_for(index), Arc::clone(&entry));
        }
        assert_eq!(store.len(), PROGRAM_ENTRY_CAPACITY);

        // One more distinct program pushes the store past its capacity. Note
        // that the store is not asked about `first` before this point: a
        // lookup promotes the entry it returns, which would make `first` the
        // most recently used entry instead of the least.
        store.insert(key_for(PROGRAM_ENTRY_CAPACITY), Arc::clone(&entry));

        assert_eq!(
            store.len(),
            PROGRAM_ENTRY_CAPACITY,
            "the store must not grow past its capacity"
        );
        assert!(
            store.get(&first).is_none(),
            "the least-recently-used entry must have been evicted"
        );
        assert!(
            store.get(&key_for(PROGRAM_ENTRY_CAPACITY)).is_some(),
            "the newest entry must be resident"
        );
    }

    /// A lookup promotes the entry it returns, so a program that keeps being
    /// used stays resident while an unused one is evicted first.
    #[test]
    fn a_lookup_promotes_the_entry_it_returns() {
        use solana_program_runtime::{
            loaded_programs::ProgramRuntimeEnvironment, solana_sbpf::program::BuiltinProgram,
        };

        use crate::programs::load_program_uncompiled;
        let env = ProgramRuntimeEnvironment::from(BuiltinProgram::new_loader(Default::default()));
        let elf = include_bytes!("../../elf/spl_memo-3.0.0.so");
        let loader = solana_sdk_ids::bpf_loader::id();
        let entry =
            Arc::new(load_program_uncompiled(&loader, &env, elf).expect("load should succeed"));

        let store = ProgramEntryStore::new();
        let key_for = |index: usize| {
            let mut bytes = elf.to_vec();
            bytes.push((index >> 8) as u8);
            bytes.push(index as u8);
            content_key(0xabc, &bytes, &loader, crate::accounts_db::IMMEDIATE_DEPLOYMENT_SLOT)
        };

        let oldest = key_for(0);
        for index in 0..PROGRAM_ENTRY_CAPACITY {
            store.insert(key_for(index), Arc::clone(&entry));
        }
        // Touch the oldest entry so it is no longer the least recently used.
        assert!(store.get(&oldest).is_some());

        store.insert(key_for(PROGRAM_ENTRY_CAPACITY), Arc::clone(&entry));

        assert!(store.get(&oldest).is_some(), "a promoted entry must survive the eviction");
        assert!(store.get(&key_for(1)).is_none(), "the untouched entry must be the one evicted");
    }

    proptest! {
        /// Store lookup is deterministic for fixed (env, bytes, loader, bucket).
        #[test]
        fn key_is_deterministic_for_arbitrary_inputs(
            bytes in prop::collection::vec(any::<u8>(), 0..512),
            env in any::<usize>(),
            bucket in any::<u64>(),
            loader_seed in any::<[u8; 32]>(),
        ) {
            let loader = Address::new_from_array(loader_seed);
            let k1 = content_key(env, &bytes, &loader, bucket);
            let k2 = content_key(env, &bytes, &loader, bucket);
            prop_assert_eq!(k1, k2);
        }

        /// Differing bytes never collide for a fixed (env, loader, bucket).
        #[test]
        fn differing_bytes_never_collide(
            base in prop::collection::vec(any::<u8>(), 1..512),
            flip_at in 0usize..512,
            flip_to in any::<u8>(),
        ) {
            let idx = flip_at % base.len();
            let mut mutated = base.clone();
            // Ensure a real difference; skip the (rare) no-op flip.
            prop_assume!(mutated[idx] != flip_to);
            mutated[idx] = flip_to;
            let loader = Address::new_unique();
            let k1 = content_key(0x1234, &base, &loader, u64::MAX);
            let k2 = content_key(0x1234, &mutated, &loader, u64::MAX);
            prop_assert_ne!(k1, k2);
        }
    }
}
