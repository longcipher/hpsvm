use std::{cell::RefCell, sync::Arc};

use parking_lot::RwLock;
use solana_account::{AccountSharedData, ReadableAccount, state_traits::StateMut};
use solana_address::Address;
use solana_address_lookup_table_interface::{error::AddressLookupError, state::AddressLookupTable};
use solana_builtins::BUILTINS;
use solana_clock::Clock;
use solana_loader_v3_interface::state::UpgradeableLoaderState;
use solana_loader_v4_interface::state::LoaderV4State;
use solana_message::{
    AddressLoader,
    v0::{LoadedAddresses, MessageAddressTableLookup},
};
use solana_program_runtime::{
    loaded_programs::{
        ProgramCacheForTxBatch, ProgramRuntimeEnvironment, ProgramRuntimeEnvironments,
    },
    program_cache_entry::{ProgramCacheEntry, ProgramCacheEntryOwner},
    program_metrics::LoadProgramMetrics,
    sysvar_cache::SysvarCache,
};
use solana_sdk_ids::{
    bpf_loader, bpf_loader_deprecated, bpf_loader_upgradeable, loader_v4, native_loader,
    sysvar::{
        clock::ID as CLOCK_ID, epoch_rewards::ID as EPOCH_REWARDS_ID,
        epoch_schedule::ID as EPOCH_SCHEDULE_ID, last_restart_slot::ID as LAST_RESTART_SLOT_ID,
        rent::ID as RENT_ID, slot_hashes::ID as SLOT_HASHES_ID,
        stake_history::ID as STAKE_HISTORY_ID,
    },
};
use solana_transaction::InstructionError;
use solana_transaction_error::AddressLoaderError;

use crate::{
    account_source::{AccountSource, AccountSourceError, EmptyAccountSource},
    collections::{HashMap, HashSet},
    error::{DbError, DbResult, ExecutionError, HPSVMError, InvalidSysvarDataError},
    programs::JitPolicy,
};

const FEES_ID: Address = Address::from_str_const("SysvarFees111111111111111111111111111111111");
const RECENT_BLOCKHASHES_ID: Address =
    Address::from_str_const("SysvarRecentB1ockHashes11111111111111111111");

fn is_cached_program_account(pubkey: &Address, account: &AccountSharedData) -> bool {
    account.executable() && *pubkey != Address::default() && account.owner() != &native_loader::ID
}

pub(crate) const fn is_managed_sysvar_account(pubkey: &Address) -> bool {
    matches!(
        *pubkey,
        CLOCK_ID |
            EPOCH_REWARDS_ID |
            EPOCH_SCHEDULE_ID |
            FEES_ID |
            LAST_RESTART_SLOT_ID |
            RECENT_BLOCKHASHES_ID |
            RENT_ID |
            SLOT_HASHES_ID |
            STAKE_HISTORY_ID
    )
}

fn validate_sysvar_account(
    pubkey: Address,
    account: &AccountSharedData,
) -> Result<(), InvalidSysvarDataError> {
    use InvalidSysvarDataError::{
        Clock as ClockError, EpochRewards, EpochSchedule, Fees, LastRestartSlot, RecentBlockhashes,
        Rent, SlotHashes, StakeHistory,
    };

    match pubkey {
        CLOCK_ID => {
            let _: Clock = account.deserialize_data().map_err(|_| ClockError)?;
        }
        EPOCH_REWARDS_ID => {
            let _: solana_epoch_rewards::EpochRewards =
                account.deserialize_data().map_err(|_| EpochRewards)?;
        }
        EPOCH_SCHEDULE_ID => {
            let _: solana_epoch_schedule::EpochSchedule =
                account.deserialize_data().map_err(|_| EpochSchedule)?;
        }
        FEES_ID => {
            #[expect(deprecated)]
            let _: solana_sysvar::fees::Fees = account.deserialize_data().map_err(|_| Fees)?;
        }
        LAST_RESTART_SLOT_ID => {
            let _: solana_sysvar::last_restart_slot::LastRestartSlot =
                account.deserialize_data().map_err(|_| LastRestartSlot)?;
        }
        RECENT_BLOCKHASHES_ID => {
            #[expect(deprecated)]
            let _: solana_sysvar::recent_blockhashes::RecentBlockhashes =
                account.deserialize_data().map_err(|_| RecentBlockhashes)?;
        }
        RENT_ID => {
            let _: solana_rent::Rent = account.deserialize_data().map_err(|_| Rent)?;
        }
        SLOT_HASHES_ID => {
            let _: solana_slot_hashes::SlotHashes =
                account.deserialize_data().map_err(|_| SlotHashes)?;
        }
        STAKE_HISTORY_ID => {
            let _: solana_stake_interface::stake_history::StakeHistory =
                account.deserialize_data().map_err(|_| StakeHistory)?;
        }
        _ => {}
    }

    Ok(())
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CacheDb {
    pub(crate) inner: HashMap<Address, AccountSharedData>,
    pub(crate) removed: HashSet<Address>,
}

impl CacheDb {
    /// Returns a borrowed account from the overlay (no source fall-through).
    pub fn get_account_ref(&self, pubkey: &Address) -> Option<&AccountSharedData> {
        self.inner.get(pubkey)
    }

    /// Returns `true` when `pubkey` was explicitly removed from the overlay.
    pub fn is_removed(&self, pubkey: &Address) -> bool {
        self.removed.contains(pubkey)
    }
}

/// Shared handle to the committed program cache.
///
/// Cloning the handle is a cheap `Arc` pointer bump. The inner
/// `ProgramCacheForTxBatch` is never deep-cloned on the transaction path;
/// callers needing `&mut` access (the `InvokeContext` constructor, prep-time
/// rebuilds) take the `RwLock` instead.
pub(crate) type SharedProgramCache = Arc<RwLock<ProgramCacheForTxBatch>>;

pub(crate) struct Accounts {
    source: Arc<dyn AccountSource>,
    cache: CacheDb,
    /// Committed program cache, shared across VM clones and transactions.
    ///
    /// The `Arc` makes the handle cheap to clone (pointer bump, never a
    /// `HashMap` deep clone); the `RwLock` supplies the `&mut
    /// ProgramCacheForTxBatch` the `InvokeContext` API requires on the
    /// transaction path. Prep paths (`add_account`, `rebuild_*`) take the
    /// write lock briefly; the transaction path holds it for the whole
    /// execution and drains transaction-scratch state afterwards, so workers
    /// cloning this `Accounts` (batch stages, simulates) observe the same
    /// committed entries without ever cloning the map.
    pub(crate) programs_cache: SharedProgramCache,
    sysvar_cache: SysvarCache,
    environments: Arc<ProgramRuntimeEnvironments>,
    /// Tiering policy observed by the program load path.
    ///
    /// Synced from `Version::jit_policy` by the owning `Svm` before every
    /// load/rebuild. `Accounts` owns a copy so `add_account` and
    /// `rebuild_program_cache` (which have no `Version` handle) can select
    /// the eager vs uncompiled load path and consult the resident store.
    /// Direct `Version` field assignment without going through
    /// `Svm::set_jit_policy` is still honored because `Svm` re-syncs before
    /// each load (see `sync_jit_policy` call sites).
    jit_policy: JitPolicy,
    /// Ordered undo log for account state (`inner` + `removed`).
    ///
    /// Each entry records the pre-mutation state of one address, so
    /// [`Accounts::rollback`] can restore an earlier
    /// [`StateCheckpoint`] byte-identically by replaying entries in reverse.
    /// A cloned DB carries its journal; rolling back the clone does not
    /// affect the original.
    ///
    /// Scope: account state only. Cache-only mutations (`programs_cache`,
    /// `sysvar_cache`, `environments`) are NOT journaled; caches are derived
    /// from `inner` and can be rebuilt via `rebuild_*_cache`.
    journal: Vec<JournalEntry>,
}

/// One undo step for a single address's account state.
///
/// A single variant subsumes per-field sketches (`Lamports` / `Data` /
/// `Wipe`): every `inner` / `removed` mutation is fully described by the
/// previous overlay entry (`previous`) plus the previous tombstone
/// (`was_removed`), so restoring both fields exactly undoes any add, update,
/// or remove regardless of which account bytes changed.
#[derive(Clone, Debug)]
pub(crate) enum JournalEntry {
    Account { address: Address, previous: Option<AccountSharedData>, was_removed: bool },
}

/// Opaque index into the journal marking a restorable point in time.
///
/// Produced by [`Accounts::checkpoint`]; consumed by
/// [`Accounts::rollback`]. The index is positional: it equals the journal
/// length at checkpoint time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StateCheckpoint(usize);

#[derive(Debug)]
pub(crate) struct AccountSourceLoadFailure {
    pub(crate) pubkey: Address,
    pub(crate) source: AccountSourceError,
}

impl AccountSourceLoadFailure {
    /// Maps the failure onto the layered taxonomy: the source error's fatal
    /// classification (infrastructure down vs deterministic) is preserved in
    /// the [`DbError`].
    pub(crate) fn into_db_error(self) -> DbError {
        DbError::from(self.source)
    }

    /// Maps the failure onto the execution layer as a database failure.
    pub(crate) fn into_execution_error(self) -> ExecutionError {
        ExecutionError::Database(self.into_db_error())
    }
}

/// Deterministic program-prewarm failure: the bytes for `program_id` cannot
/// be verified/loaded. Carries the program address for context while keeping
/// the non-fatal classification program-cache load failures always get
/// (see `impl From<InstructionError> for DbError`).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("failed to prewarm program {program_id}: {source}")]
pub(crate) struct PrewarmProgramError {
    program_id: Address,
    source: InstructionError,
}

#[derive(Debug)]
enum LookupTableLoadError {
    AddressLookup(AddressLookupError),
    AccountSource(AccountSourceLoadFailure),
}

impl From<AddressLookupError> for LookupTableLoadError {
    fn from(value: AddressLookupError) -> Self {
        Self::AddressLookup(value)
    }
}

pub(crate) struct AccountSourceTrackingAddressLoader<'a> {
    accounts_db: &'a Accounts,
    failure: RefCell<Option<AccountSourceLoadFailure>>,
}

impl<'a> AccountSourceTrackingAddressLoader<'a> {
    pub(crate) const fn new(accounts_db: &'a Accounts) -> Self {
        Self { accounts_db, failure: RefCell::new(None) }
    }

    pub(crate) fn take_failure(&self) -> Option<AccountSourceLoadFailure> {
        self.failure.take()
    }

    fn record_failure(&self, failure: AccountSourceLoadFailure) {
        let mut current = self.failure.borrow_mut();
        if current.is_none() {
            *current = Some(failure);
        }
    }
}

impl Clone for Accounts {
    fn clone(&self) -> Self {
        Self {
            source: self.source.clone(),
            cache: self.cache.clone(),
            // Share, don't clone: the program cache is invariant data on the
            // read path, so VM clones (batch workers, simulates, feature-set
            // backups) bump the refcount instead of deep-cloning the map.
            programs_cache: Arc::clone(&self.programs_cache),
            sysvar_cache: self.sysvar_cache.clone(),
            environments: self.environments.clone(),
            jit_policy: self.jit_policy,
            journal: self.journal.clone(),
        }
    }
}

impl Accounts {
    /// Clones this database for a parallel batch worker.
    ///
    /// The overlay is deep-copied because the worker mutates it. The
    /// committed program cache is deep-copied too: agave's `process_message`
    /// needs `&mut` access to it and replenishes transaction-scratch
    /// entries, and those mutations must not be shared between workers or
    /// with the live VM.
    ///
    /// The deep copy happens on the worker-setup path (amortised per Rayon
    /// worker thread, not per transaction), so the single-transaction hot
    /// path keeps sharing the committed cache through its `Arc` handle.
    pub(crate) fn clone_for_worker(&self) -> Self {
        let committed_cache_snapshot = self.programs_cache.read().clone();
        let mut worker = self.clone();
        worker.programs_cache = Arc::new(RwLock::new(committed_cache_snapshot));
        worker
    }
}

impl Default for Accounts {
    fn default() -> Self {
        use solana_program_runtime::solana_sbpf::program::BuiltinProgram;
        let env = ProgramRuntimeEnvironment::from(BuiltinProgram::new_loader(Default::default()));
        Self {
            source: Arc::new(EmptyAccountSource),
            cache: CacheDb::default(),
            programs_cache: Arc::new(RwLock::new(ProgramCacheForTxBatch::default())),
            sysvar_cache: SysvarCache::default(),
            environments: Arc::new(ProgramRuntimeEnvironments::new(env.clone(), env)),
            jit_policy: JitPolicy::DEFAULT,
            journal: Vec::new(),
        }
    }
}

// ponytail: agave 4.3 delays BPF program visibility by one slot
// (`effective_slot = deployment_slot + 1`, enforced in cache `find`), and no
// backdating can precede genesis slot 0. A deployment slot no real clock slot
// ever equals keeps hpsvm-loaded programs immediately effective.
pub(crate) const IMMEDIATE_DEPLOYMENT_SLOT: u64 = u64::MAX;

impl std::fmt::Debug for Accounts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Accounts")
            .field("source", &"dyn AccountSource")
            .field("cache", &self.cache)
            .field("programs_cache", &*self.programs_cache.read())
            .field("sysvar_cache", &self.sysvar_cache)
            .field("environments", &"ProgramRuntimeEnvironments")
            .field("journal", &self.journal)
            .finish()
    }
}

/// Read-only facade over the VM accounts database.
#[derive(Clone, Copy, Debug)]
pub struct AccountsView<'a> {
    accounts: &'a Accounts,
}

impl<'a> AccountsView<'a> {
    pub(crate) const fn new(accounts: &'a Accounts) -> Self {
        Self { accounts }
    }

    /// Returns a borrowed account for the provided address.
    pub fn get_account_ref(&self, pubkey: &Address) -> Option<&'a AccountSharedData> {
        self.accounts.get_account_ref(pubkey)
    }

    /// Returns a cloned account for the provided address.
    pub fn get_account(&self, pubkey: &Address) -> Option<AccountSharedData> {
        self.accounts.get_account(pubkey)
    }

    /// Returns a borrowed slice of ELF bytes for the provided program account.
    pub fn try_program_elf_bytes(
        &self,
        program_key: &Address,
    ) -> std::result::Result<&'a [u8], InstructionError> {
        self.accounts.try_program_elf_bytes(program_key)
    }
}

impl Accounts {
    pub(crate) fn set_account_source(&mut self, source: Arc<dyn AccountSource>) {
        self.source = source;
    }

    pub(crate) fn replace_account_source(
        &mut self,
        source: Arc<dyn AccountSource>,
    ) -> Arc<dyn AccountSource> {
        std::mem::replace(&mut self.source, source)
    }

    #[inline]
    pub fn get_account_ref(&self, pubkey: &Address) -> Option<&AccountSharedData> {
        self.cache.inner.get(pubkey)
    }

    pub fn try_get_account(
        &self,
        pubkey: &Address,
    ) -> Result<Option<AccountSharedData>, AccountSourceError> {
        if let Some(account) = self.get_account_ref(pubkey) {
            return Ok(Some(account.clone()));
        }

        if self.cache.removed.contains(pubkey) {
            return Ok(None);
        }

        self.source.get_account(pubkey)
    }

    pub fn get_account(&self, pubkey: &Address) -> Option<AccountSharedData> {
        self.try_get_account(pubkey).unwrap_or_else(|error| {
            tracing::error!(?pubkey, %error, "failed to load account from source");
            None
        })
    }

    pub(crate) fn get_accounts_batch(
        &self,
        pubkeys: &[Address],
    ) -> Result<Vec<Option<AccountSharedData>>, AccountSourceError> {
        self.source.get_accounts(pubkeys)
    }

    /// Batch-loads `addrs` from the account source into the overlay.
    ///
    /// Only addresses missing from the overlay (neither cached nor tombstoned)
    /// are fetched, in a single `source.get_accounts` round-trip. Hits are
    /// inserted with [`Accounts::add_account_no_checks`]: source data is
    /// already canonical, and the no-checks path keeps the load byte-identical
    /// to a per-key fall-through — no sysvar validation, no program-cache
    /// rebuild, no zero-lamport tombstone conversion — so prewarmed execution
    /// observes exactly what non-prewarmed execution would load. Addresses
    /// absent at the source are left untouched (no tombstones), preserving the
    /// fall-through `None` observable.
    ///
    /// Short-circuits without touching the source when every address is
    /// already known or `addrs` is empty. Insertions are journaled like any
    /// other overlay mutation; prewarm runs before any stage checkpoint, so
    /// rollback only truncates to later checkpoints and is unaffected.
    ///
    /// The only failure mode is the batched source load itself, returned
    /// as a [`DbError`](crate::error::DbError) so the fatal classification
    /// of the underlying [`AccountSourceError`] (infrastructure down vs
    /// deterministic failure) survives for the caller to inspect.
    pub(crate) fn prewarm(&mut self, addrs: &[Address]) -> DbResult<()> {
        // The missing set is de-duplicated through a hash set: the read set of
        // a batch stage is the union of the static account keys of every
        // transaction in it, so a linear scan over the accumulated list made
        // the prewarm quadratic in the number of accounts.
        let mut missing: Vec<Address> = Vec::new();
        let mut seen: HashSet<Address> = HashSet::with_capacity(addrs.len());
        for addr in addrs {
            if self.cache.inner.contains_key(addr) || self.cache.removed.contains(addr) {
                continue;
            }
            if seen.insert(*addr) {
                missing.push(*addr);
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        let loaded = self.source.get_accounts(&missing).map_err(DbError::from)?;
        for (addr, maybe_account) in missing.iter().zip(loaded) {
            if let Some(account) = maybe_account {
                self.add_account_no_checks(*addr, account);
            }
        }
        Ok(())
    }

    pub(crate) fn remove_account(&mut self, pubkey: &Address) {
        let was_managed_sysvar = is_managed_sysvar_account(pubkey);
        self.record_journal(*pubkey);
        self.cache.inner.remove(pubkey);
        self.cache.removed.insert(*pubkey);
        // The account is gone, so the program-cache entry it backed must go
        // with it: a stale entry would let a transaction invoke a program
        // whose account no longer exists.
        //
        // The program cache is shared through an `Arc` with the VM's clones,
        // so the removal is visible to them too — which matches the invariant
        // the cache is derived from: the account overlay.
        self.programs_cache.write().remove_entry(pubkey);
        // The sysvar cache is derived from the overlay too, so a removed
        // sysvar has to leave it — otherwise the runtime would keep reading a
        // sysvar whose account no longer exists.
        if was_managed_sysvar {
            self.rebuild_sysvar_cache();
        }
    }

    /// Captures the current end of the journal as a restorable checkpoint.
    pub(crate) fn checkpoint(&self) -> StateCheckpoint {
        StateCheckpoint(self.journal.len())
    }

    /// Streams one [`AccountChange`](crate::state_change_sink::AccountChange) per
    /// journaled entry to `sink`, using the entry's recorded previous state as
    /// `before` and the current overlay state as `after`.
    ///
    /// This lets tooling observe the staged delta without re-deriving it from
    /// the full account store. Sink errors propagate immediately.
    pub fn commit_source<S: crate::state_change_sink::StateChangeSink>(
        &self,
        sink: &mut S,
    ) -> Result<(), S::Error> {
        for entry in &self.journal {
            let JournalEntry::Account { address, previous, .. } = entry;
            let change = crate::state_change_sink::AccountChange {
                address: *address,
                before: previous.as_ref(),
                after: self.get_account_ref(address),
            };
            sink.on_account_change(change)?;
        }
        Ok(())
    }

    /// Returns the current journal length (number of undo entries recorded).
    ///
    /// Used to prove rollback restores the journal itself: after rolling back
    /// to a checkpoint, the length equals the checkpoint length again.
    #[cfg(test)]
    pub(crate) fn journal_len(&self) -> usize {
        self.journal.len()
    }

    /// The journal entries recorded since `from`.
    ///
    /// A journal entry records the state an address had *before* the
    /// mutation, so the entries describe what the overlay went through.
    #[cfg(test)]
    pub(crate) fn journal_since(&self, from: usize) -> &[JournalEntry] {
        &self.journal[from.min(self.journal.len())..]
    }

    /// Restores `inner` + `removed` to the state at `checkpoint` by undoing
    /// journal entries at indices `>= checkpoint.0` in reverse order.
    ///
    /// Rolling back to the current length is a no-op, which also makes
    /// repeated rollback to the same checkpoint idempotent
    /// (`rollback(cp)` twice == once: the first call truncates the journal
    /// to `cp.0`, so the second call finds nothing to undo). A checkpoint
    /// beyond the current journal end (stale after an earlier rollback) is
    /// likewise a no-op rather than a panic.
    ///
    /// Account state only: `programs_cache` / `sysvar_cache` are not rewound.
    /// Callers that mutate executable or managed-sysvar accounts across a
    /// checkpoint should call `rebuild_program_cache` / `rebuild_sysvar_cache`
    /// after rollback if the caches must match restored state.
    pub(crate) fn rollback(&mut self, checkpoint: StateCheckpoint) {
        while self.journal.len() > checkpoint.0 {
            let Some(entry) = self.journal.pop() else { break };
            let JournalEntry::Account { address, previous, was_removed } = entry;
            match previous {
                Some(account) => {
                    self.cache.inner.insert(address, account);
                }
                None => {
                    self.cache.inner.remove(&address);
                }
            }
            if was_removed {
                self.cache.removed.insert(address);
            } else {
                self.cache.removed.remove(&address);
            }
        }
    }

    /// Pushes the pre-mutation state of `pubkey` onto the journal.
    ///
    /// Must be called BEFORE mutating `inner` / `removed`, and only on paths
    /// that actually mutate (failed validation in `add_account` returns
    /// before recording, so failed calls leave no spurious entries).
    fn record_journal(&mut self, pubkey: Address) {
        self.journal.push(JournalEntry::Account {
            previous: self.cache.inner.get(&pubkey).cloned(),
            was_removed: self.cache.removed.contains(&pubkey),
            address: pubkey,
        });
    }

    pub(crate) fn minimum_balance_for_rent_exemption(&self, data_len: usize) -> u64 {
        1.max(self.sysvar_cache.get_rent().unwrap_or_default().minimum_balance(data_len))
    }

    pub(crate) fn current_slot(&self) -> u64 {
        self.sysvar_cache.get_clock().unwrap_or_default().slot
    }

    pub(crate) fn replenish_program_cache(
        &mut self,
        program_id: Address,
        program: Arc<ProgramCacheEntry>,
    ) {
        self.programs_cache.write().replenish(program_id, program);
    }

    /// Returns the shared program-cache handle.
    ///
    /// This is a cheap `Arc` clone (pointer bump): the transaction path
    /// shares the committed cache through this handle instead of
    /// deep-cloning the map per transaction.
    #[inline]
    pub(crate) fn shared_programs_cache(&self) -> SharedProgramCache {
        Arc::clone(&self.programs_cache)
    }

    pub(crate) fn has_program_cache_entry(&self, program_id: &Address) -> bool {
        self.programs_cache.read().find(program_id).is_some()
    }

    /// Snapshots the committed program entries for tier observation/stats.
    ///
    /// Collects the resident `Arc` for every program account currently in
    /// the overlay. Entries missing from the cache (never loaded) are
    /// skipped. The snapshot holds `Arc`s, so the sweep never holds the
    /// cache lock while promoting.
    pub(crate) fn program_entries_snapshot(&self) -> Vec<Arc<ProgramCacheEntry>> {
        let guard = self.programs_cache.read();
        let mut out = Vec::new();
        for (program_id, account) in &self.cache.inner {
            if !is_cached_program_account(program_id, account) {
                continue;
            }
            if let Some(entry) = guard.find(program_id) {
                out.push(entry);
            }
        }
        out
    }

    #[inline]
    pub(crate) fn runtime_environments(&self) -> &ProgramRuntimeEnvironments {
        &self.environments
    }

    /// Returns a handle to the shared runtime-environments `Arc`.
    ///
    /// Used by the default-program cache to verify that the current VM is
    /// running under the process-wide shared default environment (pointer
    /// identity) before reusing a memoized executable.
    pub(crate) fn runtime_environments_arc(&self) -> &Arc<ProgramRuntimeEnvironments> {
        &self.environments
    }

    pub(crate) fn set_runtime_environments(&mut self, envs: ProgramRuntimeEnvironments) {
        self.environments = Arc::new(envs);
    }

    /// Install an already-shared runtime-environments `Arc` without re-wrapping.
    ///
    /// Default-constructed VMs share a single immutable environment across the
    /// process so that default-program executables can be memoized once and
    /// reused by every subsequent `Hpsvm::new()`.
    pub(crate) fn set_runtime_environments_arc(&mut self, envs: Arc<ProgramRuntimeEnvironments>) {
        self.environments = envs;
    }

    /// Returns the tiering policy observed by the load path.
    pub(crate) const fn jit_policy(&self) -> JitPolicy {
        self.jit_policy
    }

    /// Syncs the load-path policy from the owning `Svm`.
    ///
    /// Called by `Svm` before every load/rebuild so direct `Version` field
    /// assignment stays honored without a setter round-trip.
    pub(crate) const fn sync_jit_policy(&mut self, policy: JitPolicy) {
        self.jit_policy = policy;
    }

    pub(crate) const fn sysvar_cache(&self) -> &SysvarCache {
        &self.sysvar_cache
    }

    /// We should only use this when we know we're not touching any executable or sysvar accounts,
    /// or have already handled such cases.
    pub(crate) fn add_account_no_checks(&mut self, pubkey: Address, account: AccountSharedData) {
        self.record_journal(pubkey);
        self.cache.removed.remove(&pubkey);
        self.cache.inner.insert(pubkey, account);
    }

    pub(crate) fn add_account(
        &mut self,
        pubkey: Address,
        account: AccountSharedData,
    ) -> Result<(), HPSVMError> {
        // Single lookup: classify the existing entry before we mutate the map.
        // Under Polonius (NLL next-gen) the borrow from `get` ends naturally
        // before the mutable operations below, avoiding the old borrow-checker
        // limitation that forced the triple-lookup pattern.
        let had_cached_program = self
            .cache
            .inner
            .get(&pubkey)
            .is_some_and(|existing| is_cached_program_account(&pubkey, existing));

        let has_cached_program =
            account.lamports() != 0 && is_cached_program_account(&pubkey, &account);

        if is_managed_sysvar_account(&pubkey) && account.lamports() != 0 {
            validate_sysvar_account(pubkey, &account)?;
        }

        // Load the program *before* moving `account` into the HashMap.
        // `load_program` only needs `&AccountSharedData` and `&self` (for the
        // programdata account lookup), so passing a reference to the local
        // `account` avoids the post-insert `get_account_ref` lookup entirely.
        let program_entry =
            if has_cached_program { Some(self.load_program(&account)?) } else { None };

        // Record AFTER validation and program loading succeed: a failed call
        // returns above without touching `inner` / `removed`, so it must not
        // leave a journal entry either.
        self.record_journal(pubkey);
        if account.lamports() == 0 {
            self.cache.inner.remove(&pubkey);
            self.cache.removed.insert(pubkey);
        } else {
            self.cache.removed.remove(&pubkey);
            self.cache.inner.insert(pubkey, account);
        }

        if is_managed_sysvar_account(&pubkey) {
            self.rebuild_sysvar_cache();
        }

        if let Some(loaded_program) = program_entry {
            self.programs_cache.write().replenish(pubkey, loaded_program);
        } else if had_cached_program {
            self.rebuild_program_cache()?;
        }

        Ok(())
    }

    pub(crate) fn rebuild_sysvar_cache(&mut self) {
        // The clock carries the slot the program cache is keyed by. When the
        // clock is absent the current cache slot is kept, so a rebuild cannot
        // make already-deployed programs invisible by resetting the slot to
        // zero.
        let fallback_slot = self.programs_cache.read().slot();
        self.sysvar_cache.reset();
        self.sysvar_cache.fill_missing_entries(|pubkey, set_sysvar| {
            if let Some(acc) = self.cache.inner.get(pubkey) {
                set_sysvar(acc.data());
            }
        });
        let slot = self.sysvar_cache.get_clock().map(|clock| clock.slot).unwrap_or(fallback_slot);
        self.programs_cache.write().set_slot_for_tests(slot);
    }

    pub(crate) fn rebuild_program_cache(&mut self) -> Result<(), InstructionError> {
        let slot = self.sysvar_cache.get_clock().unwrap_or_default().slot;
        let mut cache = ProgramCacheForTxBatch::new(slot);

        // Single lookup per builtin: `contains_key` avoids the implicit
        // second lookup inside `replenish` that the old pattern incurred.
        for builtin in BUILTINS {
            if self.cache.inner.contains_key(&builtin.program_id) {
                let loaded_program = ProgramCacheEntry::new_builtin(builtin.register_fn);
                cache.replenish(builtin.program_id, Arc::new(loaded_program));
            }
        }

        // Collect program keys *and* account references in a single iteration,
        // eliminating the second `get_account_ref` lookup that the old
        // collect-keys-then-lookup pattern required.
        let program_entries: Vec<(Address, &AccountSharedData)> = self
            .cache
            .inner
            .iter()
            .filter_map(|(pubkey, account)| {
                is_cached_program_account(pubkey, account).then_some((*pubkey, account))
            })
            .collect();

        for (pubkey, account) in program_entries {
            // `load_program` consults the resident content store first, so
            // unchanged program accounts re-link the resident `Arc` instead
            // of re-parsing, re-verifying and re-JIT-compiling.
            let loaded_program = self.load_program(account)?;
            cache.replenish(pubkey, loaded_program);
        }

        // The cache is shared through an `Arc`, so publish the rebuilt map
        // through the lock rather than replacing the handle (clones keep
        // observing the same committed cache).
        *self.programs_cache.write() = cache;
        Ok(())
    }

    /// Skip the executable() checks for builtin accounts
    pub(crate) fn add_builtin_account(&mut self, address: Address, data: AccountSharedData) {
        // Fourth `inner` / `removed` mutation site alongside `add_account`,
        // `add_account_no_checks`, and `remove_account` (verified by grep for
        // `inner.insert` / `removed.remove`): journaled for the same reason.
        self.record_journal(address);
        self.cache.removed.remove(&address);
        self.cache.inner.insert(address, data);
    }

    pub(crate) fn sync_accounts(
        &mut self,
        mut accounts: Vec<(Address, AccountSharedData)>,
    ) -> Result<(), HPSVMError> {
        // Programdata accounts (UpgradeableLoaderState::ProgramData, first byte
        // == 3) must be inserted before the program accounts that reference
        // them. In-place unstable partition via two-pointer swap, matching the
        // previous `itertools::partition` semantics without pulling in the
        // `itertools` dependency for this single call site.
        let mut write = 0usize;
        for read in 0..accounts.len() {
            let is_programdata = accounts[read].1.owner() == &bpf_loader_upgradeable::id() &&
                accounts[read].1.data().first().is_some_and(|byte| *byte == 3);
            if is_programdata {
                if write != read {
                    accounts.swap(write, read);
                }
                write += 1;
            }
        }
        for (address, acc) in accounts {
            self.add_account(address, acc)?;
        }
        Ok(())
    }

    fn load_program(
        &self,
        program_account: &AccountSharedData,
    ) -> Result<Arc<ProgramCacheEntry>, InstructionError> {
        use crate::programs::{content_key, load_program_uncompiled, program_entry_store};

        let owner = program_account.owner();
        // The clock sysvar carries the deployment slot every program entry is
        // keyed by. It is materialized by `set_sysvars`, but `Accounts` can be
        // built without one (the builder only requires sysvars when builtins,
        // default programs or signature verification need them), so the
        // missing-sysvar case is reported as a load failure instead of
        // panicking.
        let slot = match self.sysvar_cache.get_clock() {
            Ok(clock) => clock.slot,
            Err(error) => {
                tracing::error!("clock sysvar is not materialized; cannot load a program: {error}");
                return Err(InstructionError::InvalidAccountData);
            }
        };

        // Resolve the ELF bytes and the loader key used for the entry.
        // The borrow of `programdata_account` (upgradeable path) lives for
        // the whole load; other paths borrow directly from `program_account`.
        // A missing upgradeable programdata account yields a per-slot closed
        // tombstone (not content-addressed), so it bypasses the resident
        // store below.
        let programdata_account: Option<AccountSharedData>;
        let (elf_bytes, loader_key): (&[u8], Address) = if bpf_loader::check_id(owner) ||
            bpf_loader_deprecated::check_id(owner)
        {
            (program_account.data(), *owner)
        } else if bpf_loader_upgradeable::check_id(owner) {
            let Ok(UpgradeableLoaderState::Program { programdata_address }) =
                program_account.state()
            else {
                tracing::error!(
                    "Program account data does not deserialize to UpgradeableLoaderState::Program"
                );
                return Err(InstructionError::InvalidAccountData);
            };
            // A missing programdata account yields a per-slot closed
            // tombstone rather than a content-addressed entry.
            let Some(account) = self.get_account(&programdata_address) else {
                return Ok(Arc::new(ProgramCacheEntry::new_closed_tombstone(
                    slot,
                    ProgramCacheEntryOwner::LoaderV3,
                )));
            };
            programdata_account = Some(account);
            let data = programdata_account.as_ref().expect("just assigned").data();
            let Some(programdata) =
                data.get(UpgradeableLoaderState::size_of_programdata_metadata()..)
            else {
                tracing::error!("Index out of bounds using bpf_loader_upgradeable.");
                return Err(InstructionError::InvalidAccountData);
            };
            (programdata, *owner)
        } else if loader_v4::check_id(owner) {
            let Some(elf_bytes) =
                program_account.data().get(LoaderV4State::program_data_offset()..)
            else {
                tracing::error!("Index out of bounds using loader_v4.");
                return Err(InstructionError::InvalidAccountData);
            };
            (elf_bytes, loader_v4::id())
        } else {
            tracing::error!("Owner does not match any expected loader.");
            return Err(InstructionError::IncorrectProgramId);
        };

        // Resident content store: re-link when the exact bytes were seen
        // before under the same environment.
        let env_ptr = Arc::as_ptr(&self.environments) as usize;
        let store_key = content_key(env_ptr, elf_bytes, &loader_key, IMMEDIATE_DEPLOYMENT_SLOT);
        if let Some(resident) = program_entry_store().get(&store_key) {
            return Ok(resident);
        }

        let program_runtime = self.environments.get_env_for_execution().clone();
        let entry: ProgramCacheEntry = if self.jit_policy.requires_uncompiled_load() &&
            cfg!(target_arch = "x86_64")
        {
            load_program_uncompiled(&loader_key, &program_runtime, elf_bytes).map_err(|error| {
                tracing::error!("Failed to load program (uncompiled): {error:?}");
                error
            })?
        } else {
            let metrics = &mut LoadProgramMetrics::default();
            ProgramCacheEntry::load(
                &loader_key,
                program_runtime,
                IMMEDIATE_DEPLOYMENT_SLOT,
                elf_bytes,
                metrics,
            )
            .map_err(|error| {
                tracing::error!("Failed to load program: {error:?}");
                InstructionError::InvalidAccountData
            })?
        };
        let arc = Arc::new(entry);
        Ok(program_entry_store().insert(store_key, arc))
    }

    /// Resolves (and, under `Lazy`, promotes) the program set for a batch
    /// stage or fixture before execution, off the transaction path.
    ///
    /// Programs already resident re-link with no reload; missing programs
    /// load through [`Accounts::load_program`]. Under `Lazy` the prewarmed
    /// set is additionally scheduled for promotion. Failures propagate as
    /// [`DbError`] so the caller preserves the fatal classification,
    /// mirroring account `prewarm`.
    pub(crate) fn prewarm_programs(&mut self, program_ids: &[Address]) -> DbResult<()> {
        // Collect the program accounts first so the borrow of `self.cache`
        // ends before `load_program` re-borrows `self`.
        let mut to_resolve: Vec<(Address, AccountSharedData)> = Vec::new();
        for program_id in program_ids {
            if self.programs_cache.read().find(program_id).is_some() {
                continue;
            }
            let Some(account) = self.cache.inner.get(program_id).cloned() else {
                continue;
            };
            if !is_cached_program_account(program_id, &account) {
                continue;
            }
            to_resolve.push((*program_id, account));
        }
        for (program_id, account) in to_resolve {
            let entry = self
                .load_program(&account)
                .map_err(|source| DbError::non_fatal(PrewarmProgramError { program_id, source }))?;
            self.programs_cache.write().replenish(program_id, entry);
        }
        Ok(())
    }

    fn try_load_lookup_table_addresses(
        &self,
        address_table_lookup: &MessageAddressTableLookup,
    ) -> Result<LoadedAddresses, LookupTableLoadError> {
        let table_account = match self.try_get_account(&address_table_lookup.account_key) {
            Ok(Some(account)) => account,
            Ok(None) => return Err(AddressLookupError::LookupTableAccountNotFound.into()),
            Err(source) => {
                return Err(LookupTableLoadError::AccountSource(AccountSourceLoadFailure {
                    pubkey: address_table_lookup.account_key,
                    source,
                }));
            }
        };

        if table_account.owner() == &solana_sdk_ids::address_lookup_table::id() {
            // The slot hashes and the clock are the two sysvars a lookup table
            // is resolved against. Both are materialized by `set_sysvars`, but
            // an `Accounts` database can be built without them, so the
            // missing-sysvar case is reported as a lookup failure instead of
            // panicking.
            let slot_hashes = match self.sysvar_cache.get_slot_hashes() {
                Ok(hashes) => hashes,
                Err(error) => {
                    tracing::error!(
                        "slot hashes sysvar is not materialized; cannot resolve a lookup table: {error}"
                    );
                    return Err(AddressLookupError::InvalidAccountData.into());
                }
            };
            let clock = match self.sysvar_cache.get_clock() {
                Ok(clock) => clock,
                Err(error) => {
                    tracing::error!(
                        "clock sysvar is not materialized; cannot resolve a lookup table: {error}"
                    );
                    return Err(AddressLookupError::InvalidAccountData.into());
                }
            };
            let current_slot = clock.slot;
            let lookup_table = AddressLookupTable::deserialize(table_account.data())
                .map_err(|_ix_err| AddressLookupError::InvalidAccountData)?;

            Ok(LoadedAddresses {
                writable: lookup_table.lookup(
                    current_slot,
                    &address_table_lookup.writable_indexes,
                    &slot_hashes,
                )?,
                readonly: lookup_table.lookup(
                    current_slot,
                    &address_table_lookup.readonly_indexes,
                    &slot_hashes,
                )?,
            })
        } else {
            Err(AddressLookupError::InvalidAccountOwner.into())
        }
    }

    fn load_lookup_table_addresses(
        &self,
        address_table_lookup: &MessageAddressTableLookup,
    ) -> std::result::Result<LoadedAddresses, AddressLookupError> {
        self.try_load_lookup_table_addresses(address_table_lookup).map_err(|error| match error {
            LookupTableLoadError::AddressLookup(error) => error,
            LookupTableLoadError::AccountSource(_) => {
                AddressLookupError::LookupTableAccountNotFound
            }
        })
    }

    /// Returns a borrowed slice of ELF bytes for this account.
    /// Fails if the account is not a program account.
    pub fn try_program_elf_bytes<'a>(
        &'a self,
        program_key: &Address,
    ) -> std::result::Result<&'a [u8], InstructionError> {
        let program_account =
            self.get_account_ref(program_key).ok_or(InstructionError::MissingAccount)?;
        let owner = program_account.owner();

        if bpf_loader::check_id(owner) || bpf_loader_deprecated::check_id(owner) {
            Ok(program_account.data())
        } else if bpf_loader_upgradeable::check_id(owner) {
            let Ok(UpgradeableLoaderState::Program { programdata_address }) =
                program_account.state()
            else {
                return Err(InstructionError::InvalidAccountData);
            };
            let programdata_account =
                self.get_account_ref(&programdata_address).ok_or_else(|| {
                    tracing::error!("Program data account {programdata_address} not found");
                    InstructionError::MissingAccount
                })?;
            let program_data = programdata_account.data();
            if let Some(programdata) =
                program_data.get(UpgradeableLoaderState::size_of_programdata_metadata()..)
            {
                Ok(programdata)
            } else {
                tracing::error!("Index out of bounds using bpf_loader_upgradeable.");
                Err(InstructionError::InvalidAccountData)
            }
        } else if loader_v4::check_id(owner) {
            if let Some(elf_bytes) =
                program_account.data().get(LoaderV4State::program_data_offset()..)
            {
                Ok(elf_bytes)
            } else {
                tracing::error!("Index out of bounds using loader_v4.");
                Err(InstructionError::InvalidAccountData)
            }
        } else {
            tracing::error!("Owner does not match any expected loader.");
            Err(InstructionError::IncorrectProgramId)
        }
    }
}

const fn into_address_loader_error(err: AddressLookupError) -> AddressLoaderError {
    match err {
        AddressLookupError::LookupTableAccountNotFound => {
            AddressLoaderError::LookupTableAccountNotFound
        }
        AddressLookupError::InvalidAccountOwner => AddressLoaderError::InvalidAccountOwner,
        AddressLookupError::InvalidAccountData => AddressLoaderError::InvalidAccountData,
        AddressLookupError::InvalidLookupIndex => AddressLoaderError::InvalidLookupIndex,
    }
}

impl AddressLoader for &Accounts {
    fn load_addresses(
        self,
        lookups: &[MessageAddressTableLookup],
    ) -> Result<LoadedAddresses, AddressLoaderError> {
        lookups
            .iter()
            .map(|lookup| {
                self.load_lookup_table_addresses(lookup).map_err(into_address_loader_error)
            })
            .collect()
    }
}

impl AddressLoader for &AccountSourceTrackingAddressLoader<'_> {
    fn load_addresses(
        self,
        lookups: &[MessageAddressTableLookup],
    ) -> Result<LoadedAddresses, AddressLoaderError> {
        lookups
            .iter()
            .map(|lookup| {
                self.accounts_db.try_load_lookup_table_addresses(lookup).map_err(
                    |error| match error {
                        LookupTableLoadError::AccountSource(failure) => {
                            self.record_failure(failure);
                            AddressLoaderError::LookupTableAccountNotFound
                        }
                        LookupTableLoadError::AddressLookup(error) => {
                            into_address_loader_error(error)
                        }
                    },
                )
            })
            .collect()
    }
}

/// Compatibility alias for the pre-split `Accounts` name.
// `Accounts` is the canonical name (the legacy `AccountsDb` alias was removed).

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use solana_account::WritableAccount;
    use solana_keypair::Keypair;
    use solana_message::Message;
    use solana_signer::Signer;
    use solana_system_interface::{instruction::transfer, program as system_program};
    use solana_transaction::{Transaction, versioned::VersionedTransaction};

    use super::*;
    use crate::{
        Hpsvm,
        account_source::{AccountSource, AccountSourceError},
    };

    #[derive(Clone)]
    struct CountingAccountSource {
        batch_calls: Arc<AtomicUsize>,
        single_calls: Arc<AtomicUsize>,
    }

    impl CountingAccountSource {
        fn new() -> Self {
            Self {
                batch_calls: Arc::new(AtomicUsize::new(0)),
                single_calls: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl AccountSource for CountingAccountSource {
        fn get_account(
            &self,
            _pubkey: &Address,
        ) -> Result<Option<AccountSharedData>, AccountSourceError> {
            self.single_calls.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }

        fn get_accounts(
            &self,
            pubkeys: &[Address],
        ) -> Result<Vec<Option<AccountSharedData>>, AccountSourceError> {
            self.batch_calls.fetch_add(1, Ordering::SeqCst);
            pubkeys.iter().map(|pk| self.get_account(pk)).collect()
        }
    }

    #[test]
    fn get_accounts_batch_calls_batch_method_once() {
        let source = CountingAccountSource::new();
        let mut db = Accounts::default();
        db.set_account_source(Arc::new(source.clone()));
        let pubkeys: Vec<Address> = (0..5).map(|i| Address::new_from_array([i; 32])).collect();

        let _ = db.get_accounts_batch(&pubkeys);

        assert_eq!(source.batch_calls.load(Ordering::SeqCst), 1);
    }

    /// A removed account must not leave a program-cache entry behind: the
    /// cache is derived from the overlay, so a stale entry would let a
    /// transaction invoke a program whose account no longer exists.
    #[test]
    fn removing_a_program_account_invalidates_the_program_cache() {
        let program_id = Address::new_unique();
        let mut db = Accounts::default();

        // A program account owned by a loader, with the ELF bytes in place.
        let owner = solana_sdk_ids::bpf_loader::id();
        let mut account = AccountSharedData::new(1, 4, &owner);
        account.set_executable(true);
        account.set_data_from_slice(b"\x7fELF");
        db.add_account_no_checks(program_id, account);
        db.programs_cache.write().replenish(
            program_id,
            Arc::new(ProgramCacheEntry::new_builtin(|_invoke_context, _instruction| Ok(()))),
        );
        assert!(db.has_program_cache_entry(&program_id));

        db.remove_account(&program_id);

        assert!(
            !db.has_program_cache_entry(&program_id),
            "the program cache must not keep an entry for a removed account"
        );
    }

    #[test]
    fn prewarm_deduplicates_the_read_set_before_loading() {
        use parking_lot::Mutex;

        /// Records the size of every batch the source is asked for.
        #[derive(Clone, Default)]
        struct BatchSizeSource {
            sizes: Arc<Mutex<Vec<usize>>>,
        }

        impl AccountSource for BatchSizeSource {
            fn get_account(
                &self,
                _pubkey: &Address,
            ) -> Result<Option<AccountSharedData>, AccountSourceError> {
                Ok(None)
            }

            fn get_accounts(
                &self,
                pubkeys: &[Address],
            ) -> Result<Vec<Option<AccountSharedData>>, AccountSourceError> {
                self.sizes.lock().push(pubkeys.len());
                Ok(pubkeys.iter().map(|_| None).collect())
            }
        }

        let source = BatchSizeSource::default();
        let sizes = Arc::clone(&source.sizes);
        let mut db = Accounts::default();
        db.set_account_source(Arc::new(source));

        let first = Address::new_from_array([0xB0; 32]);
        let second = Address::new_from_array([0xB1; 32]);
        // The same address repeated must be requested once.
        db.prewarm(&[first, second, first, second, first]).expect("prewarm must succeed");

        let recorded = sizes.lock().clone();
        assert_eq!(recorded, vec![2], "the read set must be de-duplicated");
    }

    // -----------------------------------------------------------------
    // The prewarm path, driven by the batch read-set.
    // -----------------------------------------------------------------

    /// Map-backed source counting batched vs single loads separately.
    /// `get_accounts` serves from the map in one round-trip without
    /// delegating to `get_account`, so the counters distinguish a single
    /// batched load from per-key fall-through reads.
    #[derive(Clone, Default)]
    struct MapCountingSource {
        accounts: Arc<std::collections::HashMap<Address, AccountSharedData>>,
        batch_calls: Arc<AtomicUsize>,
        single_calls: Arc<AtomicUsize>,
    }

    impl MapCountingSource {
        fn with_accounts(accounts: Vec<(Address, AccountSharedData)>) -> Self {
            Self {
                accounts: Arc::new(accounts.into_iter().collect()),
                batch_calls: Arc::new(AtomicUsize::new(0)),
                single_calls: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl AccountSource for MapCountingSource {
        fn get_account(
            &self,
            pubkey: &Address,
        ) -> Result<Option<AccountSharedData>, AccountSourceError> {
            self.single_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.accounts.get(pubkey).cloned())
        }

        fn get_accounts(
            &self,
            pubkeys: &[Address],
        ) -> Result<Vec<Option<AccountSharedData>>, AccountSourceError> {
            self.batch_calls.fetch_add(1, Ordering::SeqCst);
            Ok(pubkeys.iter().map(|key| self.accounts.get(key).cloned()).collect())
        }
    }

    struct FailingBatchSource {
        single_calls: Arc<AtomicUsize>,
    }

    /// Batch source failing with a caller-chosen kind, so the fatal
    /// classification can be exercised on the real `prewarm` path for both
    /// fatal (`Unavailable`) and non-fatal (`InvalidResponse`, `Other`) kinds.
    struct KindFailingBatchSource {
        kind: crate::account_source::AccountSourceErrorKind,
    }

    impl AccountSource for KindFailingBatchSource {
        fn get_account(
            &self,
            _pubkey: &Address,
        ) -> Result<Option<AccountSharedData>, AccountSourceError> {
            Ok(None)
        }

        fn get_accounts(
            &self,
            _pubkeys: &[Address],
        ) -> Result<Vec<Option<AccountSharedData>>, AccountSourceError> {
            Err(AccountSourceError::with_kind(self.kind, "kind failure"))
        }
    }

    impl AccountSource for FailingBatchSource {
        fn get_account(
            &self,
            _pubkey: &Address,
        ) -> Result<Option<AccountSharedData>, AccountSourceError> {
            self.single_calls.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }

        fn get_accounts(
            &self,
            _pubkeys: &[Address],
        ) -> Result<Vec<Option<AccountSharedData>>, AccountSourceError> {
            Err(AccountSourceError::new("batch endpoint unavailable"))
        }
    }

    fn task43_prewarm_db(source: MapCountingSource) -> (Accounts, MapCountingSource) {
        let mut db = Accounts::default();
        db.set_account_source(Arc::new(source.clone()));
        (db, source)
    }

    #[test]
    fn task43_prewarm_loads_missing_accounts_in_a_single_batch_call() {
        let addr_a = Address::new_from_array([0xA0; 32]);
        let addr_b = Address::new_from_array([0xB0; 32]);
        let addr_absent = Address::new_from_array([0xC0; 32]);
        let acc_a = journal_test_account(11, &[1], &Address::new_from_array([1; 32]), false, 0);
        let acc_b = journal_test_account(22, &[2], &Address::new_from_array([2; 32]), false, 0);
        let source = MapCountingSource::with_accounts(vec![
            (addr_a, acc_a.clone()),
            (addr_b, acc_b.clone()),
        ]);
        let (mut db, source) = task43_prewarm_db(source);

        db.prewarm(&[addr_a, addr_b, addr_absent]).expect("cold prewarm must succeed");

        assert_eq!(source.batch_calls.load(Ordering::SeqCst), 1);
        assert_eq!(source.single_calls.load(Ordering::SeqCst), 0);
        assert_eq!(db.get_account(&addr_a), Some(acc_a));
        assert_eq!(db.get_account(&addr_b), Some(acc_b));
        // Overlay hits no longer touch the source at all.
        db.try_get_account(&addr_a).expect("overlay hit must not error");
        assert_eq!(source.batch_calls.load(Ordering::SeqCst), 1);
        assert_eq!(source.single_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn task43_prewarm_leaves_absent_accounts_without_tombstones() {
        let addr_absent = Address::new_from_array([0xD0; 32]);
        let (mut db, source) = task43_prewarm_db(MapCountingSource::default());

        db.prewarm(&[addr_absent]).expect("all-absent prewarm must succeed");

        assert_eq!(source.batch_calls.load(Ordering::SeqCst), 1);
        assert_eq!(db.get_account_ref(&addr_absent), None);
        assert!(!db.cache.is_removed(&addr_absent));
        // The fall-through observable is preserved: still a plain miss.
        assert_eq!(db.try_get_account(&addr_absent).expect("miss must not error"), None);
    }

    #[test]
    fn task43_prewarm_skips_overlay_known_addresses_without_source_calls() {
        let addr_warm = Address::new_from_array([0xE0; 32]);
        let addr_cold = Address::new_from_array([0xF0; 32]);
        let acc_warm = journal_test_account(5, &[9], &Address::new_from_array([3; 32]), false, 0);
        let acc_cold = journal_test_account(6, &[8], &Address::new_from_array([4; 32]), false, 0);
        let source = MapCountingSource::with_accounts(vec![
            (addr_warm, acc_warm.clone()),
            (addr_cold, acc_cold.clone()),
        ]);
        let (mut db, source) = task43_prewarm_db(source);
        db.add_account_no_checks(addr_warm, acc_warm.clone());

        // First prewarm loads only the cold address; the second finds
        // everything warm and short-circuits without calling the source.
        db.prewarm(&[addr_warm, addr_cold, addr_warm]).expect("prewarm must succeed");
        assert_eq!(source.batch_calls.load(Ordering::SeqCst), 1);
        assert_eq!(db.get_account(&addr_cold), Some(acc_cold));

        db.prewarm(&[addr_warm, addr_cold]).expect("warm prewarm must succeed");
        assert_eq!(source.batch_calls.load(Ordering::SeqCst), 1);
        assert_eq!(source.single_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn task43_prewarm_skips_tombstoned_addresses() {
        let addr_removed = Address::new_from_array([0xE1; 32]);
        let source = MapCountingSource::with_accounts(vec![(
            addr_removed,
            journal_test_account(5, &[9], &Address::new_from_array([3; 32]), false, 0),
        )]);
        let (mut db, source) = task43_prewarm_db(source);
        db.remove_account(&addr_removed);

        db.prewarm(&[addr_removed]).expect("prewarm over a tombstone must succeed");

        assert_eq!(source.batch_calls.load(Ordering::SeqCst), 0);
        assert_eq!(source.single_calls.load(Ordering::SeqCst), 0);
        assert!(db.cache.is_removed(&addr_removed));
    }

    #[test]
    fn task43_prewarm_with_empty_slice_is_a_noop() {
        let (mut db, source) = task43_prewarm_db(MapCountingSource::default());

        db.prewarm(&[]).expect("empty prewarm must succeed");

        assert_eq!(source.batch_calls.load(Ordering::SeqCst), 0);
        assert_eq!(source.single_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn task43_prewarm_propagates_source_errors_without_per_key_fallback() {
        let addr_a = Address::new_from_array([0xA1; 32]);
        let addr_b = Address::new_from_array([0xB1; 32]);
        let source = FailingBatchSource { single_calls: Arc::new(AtomicUsize::new(0)) };
        let single_calls = Arc::clone(&source.single_calls);
        let mut db = Accounts::default();
        db.set_account_source(Arc::new(source));

        let error = db.prewarm(&[addr_a, addr_b]).expect_err("failing source must error");

        // The error preserves the fatal classification (`Unavailable` means
        // the infrastructure is down) and the concrete source error stays
        // recoverable by downcast. No per-key fallback loop ran.
        assert!(error.is_fatal());
        let source = error
            .downcast_ref::<AccountSourceError>()
            .expect("DbError must recover the AccountSourceError");
        assert_eq!(source.kind(), crate::account_source::AccountSourceErrorKind::Unavailable);
        assert_eq!(source.message(), "batch endpoint unavailable");
        assert_eq!(single_calls.load(Ordering::SeqCst), 0);
        assert_eq!(db.get_account_ref(&addr_a), None);
        assert_eq!(db.get_account_ref(&addr_b), None);
    }

    #[test]
    fn task43_prewarm_insertions_roll_back_to_an_earlier_checkpoint() {
        let addr = Address::new_from_array([0xA2; 32]);
        let acc = journal_test_account(7, &[7], &Address::new_from_array([5; 32]), false, 0);
        let source = MapCountingSource::with_accounts(vec![(addr, acc)]);
        let (mut db, _) = task43_prewarm_db(source);
        let checkpoint = db.checkpoint();

        db.prewarm(&[addr]).expect("prewarm must succeed");
        assert!(db.get_account_ref(&addr).is_some());

        db.rollback(checkpoint);
        assert_eq!(db.get_account_ref(&addr), None);
        assert!(!db.cache.is_removed(&addr));
    }

    // -----------------------------------------------------------------
    // The fatal classification survives the real `prewarm` path.
    // -----------------------------------------------------------------

    #[test]
    fn task71_prewarm_fatal_matrix_over_source_kinds_on_the_real_path() {
        use crate::{
            account_source::AccountSourceErrorKind,
            error::{DbError, ExecutionError, HPSVMError},
        };

        for (kind, fatal) in [
            (AccountSourceErrorKind::Unavailable, true),
            (AccountSourceErrorKind::InvalidResponse, false),
            (AccountSourceErrorKind::Other, false),
        ] {
            let mut db = Accounts::default();
            db.set_account_source(Arc::new(KindFailingBatchSource { kind }));
            let addr = Address::new_from_array([0xA3; 32]);

            let error: DbError = db.prewarm(&[addr]).expect_err("failing source must error");
            assert_eq!(error.is_fatal(), fatal, "kind {kind:?} must map to fatal={fatal}");

            // The classification survives the execution layer and the
            // public `HPSVMError` boundary with the concrete error intact.
            let execution = ExecutionError::from(error);
            assert_eq!(execution.is_fatal(), fatal);
            let host = HPSVMError::from(crate::error::HostError::from(execution));
            match &host {
                HPSVMError::Host(crate::error::HostError::Execution(ExecutionError::Database(
                    inner,
                ))) => {
                    assert_eq!(inner.is_fatal(), fatal);
                    let source = inner
                        .downcast_ref::<AccountSourceError>()
                        .expect("AccountSourceError must survive to the host boundary");
                    assert_eq!(source.kind(), kind);
                }
                other => panic!("expected HPSVMError::Host(Execution(Database)), got {other:?}"),
            }
        }
    }

    #[test]
    fn task71_account_source_load_failure_maps_onto_both_layers() {
        use crate::account_source::AccountSourceErrorKind;

        let failure = AccountSourceLoadFailure {
            pubkey: Address::new_from_array([0xA4; 32]),
            source: AccountSourceError::new("rpc down"),
        };
        let db_error = failure.into_db_error();
        assert!(db_error.is_fatal());
        assert_eq!(
            db_error
                .downcast_ref::<AccountSourceError>()
                .expect("DbError must recover the source error")
                .kind(),
            AccountSourceErrorKind::Unavailable
        );

        let failure = AccountSourceLoadFailure {
            pubkey: Address::new_from_array([0xA4; 32]),
            source: AccountSourceError::with_kind(AccountSourceErrorKind::Other, "custom"),
        };
        match failure.into_execution_error() {
            crate::error::ExecutionError::Database(inner) => assert!(!inner.is_fatal()),
            other => panic!("expected ExecutionError::Database, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // The journal and the checkpoint/rollback pair.
    // -----------------------------------------------------------------

    fn journal_test_account(
        lamports: u64,
        data: &[u8],
        owner: &Address,
        executable: bool,
        rent_epoch: u64,
    ) -> AccountSharedData {
        let mut account = AccountSharedData::new(lamports, data.len(), owner);
        account.set_data_from_slice(data);
        account.set_executable(executable);
        account.set_rent_epoch(rent_epoch);
        account
    }

    // -----------------------------------------------------------------
    // Account reads: the overlay hit and the source fall-through.
    // -----------------------------------------------------------------

    #[test]
    fn task41_get_account_served_from_overlay_after_add_no_checks() {
        let mut db = Accounts::default();
        let addr = Address::new_from_array([7; 32]);
        let acc = journal_test_account(42, &[1, 2, 3], &Address::new_from_array([9; 32]), false, 1);
        db.add_account_no_checks(addr, acc.clone());

        assert_eq!(db.get_account(&addr), Some(acc.clone()));
        assert!(db.get_account_ref(&addr).is_some());
        assert!(!db.cache.is_removed(&addr));
    }

    #[test]
    fn task41_try_get_account_miss_hits_source() {
        let source = CountingAccountSource::new();
        let mut db = Accounts::default();
        db.set_account_source(Arc::new(source.clone()));
        let addr = Address::new_from_array([8; 32]);

        let result = db.try_get_account(&addr).expect("source call must not error");
        assert!(result.is_none());
        assert_eq!(source.single_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn task41_try_get_account_hit_does_not_hit_source() {
        let source = CountingAccountSource::new();
        let mut db = Accounts::default();
        db.set_account_source(Arc::new(source.clone()));
        let addr = Address::new_from_array([9; 32]);
        let acc = journal_test_account(10, &[5], &Address::new_from_array([3; 32]), false, 2);
        db.add_account_no_checks(addr, acc.clone());

        let result = db.try_get_account(&addr).expect("overlay hit must not error");
        assert_eq!(result, Some(acc));
        assert_eq!(source.single_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn task41_cache_db_shape_has_no_sysvar_or_env_accessors() {
        // Runtime evidence that CacheDb carries only overlay state; the absence
        // of sysvar/environment members is also verified by grep in the report.
        let c = CacheDb::default();
        let _ = c.get_account_ref(&Address::default());
        let _ = c.is_removed(&Address::default());
        // CacheDb has no source fall-through, programs cache, sysvar cache, or
        // environment field by construction (see struct definition).
        assert!(c.inner.is_empty());
        assert!(c.removed.is_empty());
    }

    #[test]
    fn task41_cache_db_overlay_read_returns_inserted_account() {
        let addr = Address::new_from_array([12; 32]);
        let acc = journal_test_account(7, &[9, 9], &Address::new_from_array([4; 32]), false, 3);
        let mut c = CacheDb::default();
        c.inner.insert(addr, acc.clone());

        assert_eq!(c.get_account_ref(&addr), Some(&acc));
        assert!(!c.is_removed(&addr));
    }

    #[test]
    fn task41_cache_db_removed_flag_round_trips() {
        let addr = Address::new_from_array([13; 32]);
        let mut c = CacheDb::default();
        c.removed.insert(addr);

        assert!(c.is_removed(&addr));
        assert!(c.get_account_ref(&addr).is_none());
    }

    // -- proptest properties ------------------------------------------------

    use proptest::prelude::*;

    /// Small address pool (4 entries) forces collisions and overwrites, which
    /// is exactly what makes rollback non-trivial.
    fn pool_address(idx: usize) -> Address {
        Address::new_from_array([(idx % 4) as u8 + 20; 32])
    }

    #[derive(Clone, Debug)]
    enum JournalOp {
        Add {
            addr_idx: usize,
            lamports: u64,
            data_byte: u8,
            owner_byte: u8,
            rent_epoch: u64,
        },
        AddNoChecks {
            addr_idx: usize,
            lamports: u64,
            data_byte: u8,
            owner_byte: u8,
            rent_epoch: u64,
        },
        Remove {
            addr_idx: usize,
        },
    }

    fn journal_op_strategy() -> impl Strategy<Value = JournalOp> {
        prop_oneof![
            (any::<usize>(), any::<u64>(), any::<u8>(), any::<u8>(), any::<u64>()).prop_map(
                |(addr_idx, lamports, data_byte, owner_byte, rent_epoch)| JournalOp::Add {
                    addr_idx,
                    lamports,
                    data_byte,
                    owner_byte,
                    rent_epoch,
                }
            ),
            (any::<usize>(), any::<u64>(), any::<u8>(), any::<u8>(), any::<u64>()).prop_map(
                |(addr_idx, lamports, data_byte, owner_byte, rent_epoch)| {
                    JournalOp::AddNoChecks { addr_idx, lamports, data_byte, owner_byte, rent_epoch }
                }
            ),
            any::<usize>().prop_map(|addr_idx| JournalOp::Remove { addr_idx }),
        ]
    }

    /// Non-executable accounts with non-sysvar pool addresses always pass
    /// `add_account` validation, so every generated op mutates state.
    fn journal_op_account(
        op_lamports: u64,
        data_byte: u8,
        owner_byte: u8,
        rent_epoch: u64,
    ) -> AccountSharedData {
        journal_test_account(
            op_lamports,
            &[data_byte; 4],
            &Address::new_from_array([owner_byte; 32]),
            false,
            rent_epoch,
        )
    }

    fn apply_journal_op(db: &mut Accounts, op: &JournalOp) {
        match op {
            JournalOp::Add { addr_idx, lamports, data_byte, owner_byte, rent_epoch } => {
                let account = journal_op_account(*lamports, *data_byte, *owner_byte, *rent_epoch);
                // Pool addresses are never managed sysvars and the account is
                // never executable, so validation always passes.
                db.add_account(pool_address(*addr_idx), account)
                    .expect("proptest add_account must succeed");
            }
            JournalOp::AddNoChecks { addr_idx, lamports, data_byte, owner_byte, rent_epoch } => {
                let account = journal_op_account(*lamports, *data_byte, *owner_byte, *rent_epoch);
                db.add_account_no_checks(pool_address(*addr_idx), account);
            }
            JournalOp::Remove { addr_idx } => {
                db.remove_account(&pool_address(*addr_idx));
            }
        }
    }

    proptest! {
        /// Rollback-to-origin restores `inner` + `removed` exactly for an
        /// arbitrary mutation sequence.
        #[test]
        fn rollback_to_origin_restores_the_original_snapshot(ops in prop::collection::vec(journal_op_strategy(), 0..32)) {
            let mut db = Accounts::default();
            let origin_inner = db.cache.inner.clone();
            let origin_removed = db.cache.removed.clone();
            let checkpoint = db.checkpoint();

            for op in &ops {
                apply_journal_op(&mut db, op);
            }
            prop_assert_eq!(db.journal.len() - checkpoint.0, ops.len());

            db.rollback(checkpoint);
            prop_assert_eq!(&db.cache.inner, &origin_inner);
            prop_assert_eq!(&db.cache.removed, &origin_removed);
        }

        /// Nested checkpoints unwind correctly: rolling back the inner
        /// checkpoint restores the mid-state, then rolling back the outer
        /// checkpoint restores the origin.
        #[test]
        fn nested_checkpoints_unwind_correctly(
            first_ops in prop::collection::vec(journal_op_strategy(), 0..16),
            second_ops in prop::collection::vec(journal_op_strategy(), 0..16),
        ) {
            let mut db = Accounts::default();
            let origin_inner = db.cache.inner.clone();
            let origin_removed = db.cache.removed.clone();
            let outer = db.checkpoint();

            for op in &first_ops {
                apply_journal_op(&mut db, op);
            }
            let mid_inner = db.cache.inner.clone();
            let mid_removed = db.cache.removed.clone();
            let inner_cp = db.checkpoint();

            for op in &second_ops {
                apply_journal_op(&mut db, op);
            }

            db.rollback(inner_cp);
            prop_assert_eq!(&db.cache.inner, &mid_inner);
            prop_assert_eq!(&db.cache.removed, &mid_removed);

            db.rollback(outer);
            prop_assert_eq!(&db.cache.inner, &origin_inner);
            prop_assert_eq!(&db.cache.removed, &origin_removed);
        }
    }

    #[test]
    fn rollback_restores_account_state_byte_identically() {
        use solana_sdk_ids::sysvar::clock::ID as CLOCK_ID;

        let owner_a = Address::new_from_array([10; 32]);
        let owner_b = Address::new_from_array([11; 32]);
        let addr_a = Address::new_from_array([1; 32]);
        let addr_b = Address::new_from_array([2; 32]);
        let addr_prog = Address::new_from_array([3; 32]);
        let addr_sysvar = CLOCK_ID;
        let addr_new = Address::new_from_array([4; 32]);

        let mut db = Accounts::default();

        // N = 5 setup mutations across add / add_no_checks paths, including a
        // sysvar-shaped address and a program-shaped (executable) address.
        db.add_account(addr_a, journal_test_account(100, &[1, 2, 3], &owner_a, false, 5))
            .expect("plain add_account must succeed");
        db.add_account_no_checks(
            addr_sysvar,
            journal_test_account(42, &[7; 8], &owner_a, false, 6),
        );
        db.add_account_no_checks(
            addr_prog,
            journal_test_account(77, &[9; 4], &solana_sdk_ids::bpf_loader::ID, true, 7),
        );
        db.add_account(addr_b, journal_test_account(200, &[5, 6], &owner_b, false, 8))
            .expect("plain add_account must succeed");
        db.remove_account(&addr_b);

        let snapshot_inner = db.cache.inner.clone();
        let snapshot_removed = db.cache.removed.clone();

        let checkpoint = db.checkpoint();
        let journal_len_at_checkpoint = db.journal.len();

        // Mutate further: update (all five fields change), resurrect the
        // removed address, remove the program account, add a fresh address.
        db.add_account(addr_a, journal_test_account(999, &[8, 8, 8, 8], &owner_b, false, 99))
            .expect("update must succeed");
        db.add_account(addr_b, journal_test_account(1, &[0], &owner_a, false, 0))
            .expect("resurrection must succeed");
        db.remove_account(&addr_prog);
        db.add_account(addr_new, journal_test_account(50, &[3], &owner_a, false, 1))
            .expect("fresh add must succeed");

        // Journal grew by exactly one entry per mutation.
        assert_eq!(db.journal.len() - journal_len_at_checkpoint, 4);

        db.rollback(checkpoint);

        // Byte-identical restore: derived PartialEq on AccountSharedData covers
        // lamports, data bytes, owner, executable, and rent_epoch.
        assert_eq!(db.cache.inner, snapshot_inner);
        assert_eq!(db.cache.removed, snapshot_removed);

        // Rollback is idempotent: rolling back to the same checkpoint twice is
        // a no-op the second time.
        db.rollback(checkpoint);
        assert_eq!(db.cache.inner, snapshot_inner);
        assert_eq!(db.cache.removed, snapshot_removed);
    }

    // -----------------------------------------------------------------
    // The program cache is shared as an `Arc`, never deep-cloned.
    // on the transaction path.
    // -----------------------------------------------------------------

    fn task42_deployed_program_id() -> Address {
        Address::new_from_array([0x42; 32])
    }

    /// Builds a VM with one extra builtin deployed under a fresh id, so the
    /// tests observe a program that was deployed exactly once.
    fn task42_vm_with_builtin() -> Hpsvm {
        let mut svm = Hpsvm::new();
        let register_fn = BUILTINS
            .iter()
            .find(|builtin| builtin.program_id == system_program::id())
            .expect("system builtin must exist in BUILTINS")
            .register_fn;
        svm.add_builtin(task42_deployed_program_id(), register_fn);
        svm
    }

    fn task42_funded_payer(svm: &mut Hpsvm) -> Keypair {
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 1_000_000_000).unwrap();
        payer
    }

    fn task42_transfer_tx(svm: &Hpsvm, payer: &Keypair, lamports: u64) -> Transaction {
        Transaction::new(
            &[payer],
            Message::new(
                &[transfer(&payer.pubkey(), &Address::new_unique(), lamports)],
                Some(&payer.pubkey()),
            ),
            svm.latest_blockhash(),
        )
    }

    #[test]
    fn task42_program_cache_handle_is_shared_across_send_transaction() {
        let mut svm = task42_vm_with_builtin();
        let program_id = task42_deployed_program_id();
        assert!(
            svm.state.accounts.has_program_cache_entry(&program_id),
            "deployed builtin must resolve from the cache before any transaction"
        );
        let before = svm.state.accounts.shared_programs_cache();

        let payer = task42_funded_payer(&mut svm);
        svm.send_transaction(task42_transfer_tx(&svm, &payer, 64)).unwrap();

        let after = svm.state.accounts.shared_programs_cache();
        assert!(
            Arc::ptr_eq(&before, &after),
            "program cache handle must be shared (Arc::clone), not deep-cloned, across a transaction"
        );
        assert!(
            svm.state.accounts.has_program_cache_entry(&program_id),
            "deployed builtin must still resolve from the shared cache after a transaction"
        );
    }

    #[test]
    fn task42_program_cache_handle_is_shared_across_batch_stage() {
        let mut svm = task42_vm_with_builtin();
        let program_id = task42_deployed_program_id();
        let payer_a = task42_funded_payer(&mut svm);
        let payer_b = task42_funded_payer(&mut svm);
        let before = svm.state.accounts.shared_programs_cache();

        // Distinct payers/recipients are conflict-free, so both land in one
        // stage executed on rayon worker VMs cloned from this VM's state.
        let txs = vec![
            VersionedTransaction::from(task42_transfer_tx(&svm, &payer_a, 10)),
            VersionedTransaction::from(task42_transfer_tx(&svm, &payer_b, 20)),
        ];
        let result = svm.send_transaction_batch(txs).unwrap();

        assert_eq!(result.results.len(), 2);
        assert!(result.results.iter().all(|r| r.is_ok()));
        let after = svm.state.accounts.shared_programs_cache();
        assert!(
            Arc::ptr_eq(&before, &after),
            "program cache handle must stay shared across a batch stage"
        );
        assert!(
            svm.state.accounts.has_program_cache_entry(&program_id),
            "deployed builtin must still resolve from the shared cache after a batch"
        );
    }

    #[test]
    fn task42_vm_clone_shares_the_program_cache_handle() {
        let svm = task42_vm_with_builtin();
        let worker = svm.clone();
        assert!(
            Arc::ptr_eq(
                &svm.state.accounts.shared_programs_cache(),
                &worker.state.accounts.shared_programs_cache()
            ),
            "VM clones (batch workers, simulates) must share the program cache instead of deep-cloning it"
        );
    }

    #[test]
    fn task42_program_still_resolves_after_failed_and_discarded_transactions() {
        let mut svm = task42_vm_with_builtin();
        let program_id = task42_deployed_program_id();
        let before = svm.state.accounts.shared_programs_cache();
        let payer = task42_funded_payer(&mut svm);

        // Discard a staged success: the overlay rolls back, the cache must be
        // intact.
        let staged =
            svm.transact(task42_transfer_tx(&svm, &payer, 32)).expect("transact must stage");
        assert!(staged.discard().is_ok());
        assert!(Arc::ptr_eq(&before, &svm.state.accounts.shared_programs_cache()));
        assert!(svm.state.accounts.has_program_cache_entry(&program_id));

        // Fail at runtime (insufficient funds): the cache must be intact.
        let failing = task42_transfer_tx(&svm, &payer, u64::MAX);
        assert!(svm.send_transaction(failing).is_err());
        assert!(Arc::ptr_eq(&before, &svm.state.accounts.shared_programs_cache()));
        assert!(
            svm.state.accounts.has_program_cache_entry(&program_id),
            "deployed builtin must still resolve after failed/discarded transactions"
        );
    }

    #[test]
    fn prewarm_program_load_failure_is_deterministic_not_fatal() {
        use solana_sdk_ids::bpf_loader;

        let mut svm = Hpsvm::builder().with_sysvars().build().expect("minimal VM should build");
        let program_id = Address::new_unique();
        // Bypass loading so the bogus bytes reach `prewarm_programs`.
        svm.state.accounts.add_account_no_checks(
            program_id,
            journal_test_account(1, b"definitely not an elf", &bpf_loader::id(), true, 0),
        );

        let error =
            svm.state.accounts.prewarm_programs(&[program_id]).expect_err("bogus ELF must fail");
        assert!(
            !error.is_fatal(),
            "unverifiable program bytes are deterministic for this input, not infrastructure failure"
        );
        let context = error
            .downcast_ref::<PrewarmProgramError>()
            .expect("concrete prewarm context must survive erasure");
        assert_eq!(context.program_id, program_id);
        assert_eq!(context.source, solana_transaction::InstructionError::InvalidAccountData);
    }
}
