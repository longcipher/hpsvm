#[cfg(not(feature = "hashbrown"))]
use std::collections::{HashMap, HashSet};
use std::{cell::RefCell, sync::Arc};

#[cfg(feature = "hashbrown")]
use hashbrown::{HashMap, HashSet};
use solana_account::{AccountSharedData, ReadableAccount, state_traits::StateMut};
use solana_address::Address;
use solana_address_lookup_table_interface::{error::AddressLookupError, state::AddressLookupTable};
use solana_builtins::BUILTINS;
use solana_clock::Clock;
use solana_instruction::error::InstructionError;
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
    program_cache_entry::{ProgramCacheEntry, ProgramCacheEntryOwner, ProgramCacheEntryType},
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
use solana_transaction_error::AddressLoaderError;

use crate::{
    account_source::{AccountSource, AccountSourceError, EmptyAccountSource},
    error::{HPSVMError, InvalidSysvarDataError},
};

const FEES_ID: Address = Address::from_str_const("SysvarFees111111111111111111111111111111111");
const RECENT_BLOCKHASHES_ID: Address =
    Address::from_str_const("SysvarRecentB1ockHashes11111111111111111111");

fn is_cached_program_account(pubkey: &Address, account: &AccountSharedData) -> bool {
    account.executable() && *pubkey != Address::default() && account.owner() != &native_loader::ID
}

const fn is_managed_sysvar_account(pubkey: &Address) -> bool {
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

pub(crate) struct AccountsDb {
    source: Arc<dyn AccountSource>,
    inner: HashMap<Address, AccountSharedData>,
    removed: HashSet<Address>,
    programs_cache: ProgramCacheForTxBatch,
    sysvar_cache: SysvarCache,
    environments: Arc<ProgramRuntimeEnvironments>,
}

#[derive(Debug)]
pub(crate) struct AccountSourceLoadFailure {
    pub(crate) pubkey: Address,
    pub(crate) source: AccountSourceError,
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
    accounts_db: &'a AccountsDb,
    failure: RefCell<Option<AccountSourceLoadFailure>>,
}

impl<'a> AccountSourceTrackingAddressLoader<'a> {
    pub(crate) const fn new(accounts_db: &'a AccountsDb) -> Self {
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

impl Clone for AccountsDb {
    fn clone(&self) -> Self {
        Self {
            source: self.source.clone(),
            inner: self.inner.clone(),
            removed: self.removed.clone(),
            programs_cache: self.programs_cache.clone(),
            sysvar_cache: self.sysvar_cache.clone(),
            environments: self.environments.clone(),
        }
    }
}

impl Default for AccountsDb {
    fn default() -> Self {
        use solana_program_runtime::solana_sbpf::program::BuiltinProgram;
        let env = ProgramRuntimeEnvironment::from(BuiltinProgram::new_loader(Default::default()));
        Self {
            source: Arc::new(EmptyAccountSource),
            inner: HashMap::default(),
            removed: HashSet::default(),
            programs_cache: ProgramCacheForTxBatch::default(),
            sysvar_cache: SysvarCache::default(),
            environments: Arc::new(ProgramRuntimeEnvironments::new(env.clone(), env)),
        }
    }
}

impl std::fmt::Debug for AccountsDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountsDb")
            .field("source", &"dyn AccountSource")
            .field("inner", &self.inner)
            .field("removed", &self.removed)
            .field("programs_cache", &self.programs_cache)
            .field("sysvar_cache", &self.sysvar_cache)
            .field("environments", &"ProgramRuntimeEnvironments")
            .finish()
    }
}

/// Read-only facade over the VM accounts database.
#[derive(Clone, Copy, Debug)]
pub struct AccountsView<'a> {
    accounts_db: &'a AccountsDb,
}

impl<'a> AccountsView<'a> {
    pub(crate) const fn new(accounts_db: &'a AccountsDb) -> Self {
        Self { accounts_db }
    }

    /// Returns a borrowed account for the provided address.
    pub fn get_account_ref(&self, pubkey: &Address) -> Option<&'a AccountSharedData> {
        self.accounts_db.get_account_ref(pubkey)
    }

    /// Returns a cloned account for the provided address.
    pub fn get_account(&self, pubkey: &Address) -> Option<AccountSharedData> {
        self.accounts_db.get_account(pubkey)
    }

    /// Returns a borrowed slice of ELF bytes for the provided program account.
    pub fn try_program_elf_bytes(
        &self,
        program_key: &Address,
    ) -> std::result::Result<&'a [u8], InstructionError> {
        self.accounts_db.try_program_elf_bytes(program_key)
    }
}

impl AccountsDb {
    pub(crate) fn set_account_source(&mut self, source: Arc<dyn AccountSource>) {
        self.source = source;
    }

    pub(crate) fn replace_account_source(
        &mut self,
        source: Arc<dyn AccountSource>,
    ) -> Arc<dyn AccountSource> {
        std::mem::replace(&mut self.source, source)
    }

    pub fn get_account_ref(&self, pubkey: &Address) -> Option<&AccountSharedData> {
        self.inner.get(pubkey)
    }

    pub fn try_get_account(
        &self,
        pubkey: &Address,
    ) -> Result<Option<AccountSharedData>, AccountSourceError> {
        if let Some(account) = self.get_account_ref(pubkey) {
            return Ok(Some(account.clone()));
        }

        if self.removed.contains(pubkey) {
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

    pub(crate) fn remove_account(&mut self, pubkey: &Address) {
        self.inner.remove(pubkey);
        self.removed.insert(*pubkey);
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
        self.programs_cache.replenish(program_id, program);
    }

    pub(crate) fn cloned_programs_cache(&self) -> ProgramCacheForTxBatch {
        self.programs_cache.clone()
    }

    pub(crate) fn has_program_cache_entry(&self, program_id: &Address) -> bool {
        self.programs_cache.find(program_id).is_some()
    }

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
    /// reused by every subsequent `HPSVM::new()`.
    pub(crate) fn set_runtime_environments_arc(&mut self, envs: Arc<ProgramRuntimeEnvironments>) {
        self.environments = envs;
    }

    pub(crate) const fn sysvar_cache(&self) -> &SysvarCache {
        &self.sysvar_cache
    }

    /// We should only use this when we know we're not touching any executable or sysvar accounts,
    /// or have already handled such cases.
    pub(crate) fn add_account_no_checks(&mut self, pubkey: Address, account: AccountSharedData) {
        self.removed.remove(&pubkey);
        self.inner.insert(pubkey, account);
    }

    pub(crate) fn add_account(
        &mut self,
        pubkey: Address,
        account: AccountSharedData,
    ) -> Result<(), HPSVMError> {
        let had_cached_program = self
            .inner
            .get(&pubkey)
            .is_some_and(|existing| is_cached_program_account(&pubkey, existing));

        // Compute the post-insert cached-program classification up front so the
        // owned `account` can be moved into the store without cloning it.
        let has_cached_program =
            account.lamports() != 0 && is_cached_program_account(&pubkey, &account);

        if is_managed_sysvar_account(&pubkey) && account.lamports() != 0 {
            validate_sysvar_account(pubkey, &account)?;
        }

        if account.lamports() == 0 {
            self.inner.remove(&pubkey);
            self.removed.insert(pubkey);
        } else {
            self.add_account_no_checks(pubkey, account);
        }

        if is_managed_sysvar_account(&pubkey) {
            self.rebuild_sysvar_cache();
        }

        if has_cached_program {
            let loaded_program = self.load_program(
                self.get_account_ref(&pubkey)
                    .expect("program account just inserted - this should never fail"),
            )?;
            self.programs_cache.replenish(pubkey, Arc::new(loaded_program));
        } else if had_cached_program {
            self.rebuild_program_cache()?;
        }

        Ok(())
    }

    pub(crate) fn rebuild_sysvar_cache(&mut self) {
        self.sysvar_cache.reset();
        self.sysvar_cache.fill_missing_entries(|pubkey, set_sysvar| {
            if let Some(acc) = self.inner.get(pubkey) {
                set_sysvar(acc.data());
            }
        });
        let slot = self.sysvar_cache.get_clock().unwrap_or_default().slot;
        self.programs_cache.set_slot_for_tests(slot);
    }

    pub(crate) fn rebuild_program_cache(&mut self) -> Result<(), InstructionError> {
        let slot = self.sysvar_cache.get_clock().unwrap_or_default().slot;
        let mut cache = ProgramCacheForTxBatch::new(slot);

        BUILTINS.iter().filter(|builtin| self.inner.contains_key(&builtin.program_id)).for_each(
            |builtin| {
                let loaded_program =
                    ProgramCacheEntry::new_builtin(0, builtin.name.len(), builtin.register_fn);
                cache.replenish(builtin.program_id, Arc::new(loaded_program));
            },
        );

        let program_keys = self
            .inner
            .iter()
            .filter_map(|(pubkey, account)| {
                is_cached_program_account(pubkey, account).then_some(*pubkey)
            })
            .collect::<Vec<_>>();

        for pubkey in program_keys {
            let loaded_program = self.load_program(
                self.get_account_ref(&pubkey).expect("program account should exist during rebuild - this indicates an internal inconsistency"),
            )?;
            cache.replenish(pubkey, Arc::new(loaded_program));
        }

        self.programs_cache = cache;
        Ok(())
    }

    /// Skip the executable() checks for builtin accounts
    pub(crate) fn add_builtin_account(&mut self, address: Address, data: AccountSharedData) {
        self.removed.remove(&address);
        self.inner.insert(address, data);
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
    ) -> Result<ProgramCacheEntry, InstructionError> {
        let metrics = &mut LoadProgramMetrics::default();

        let owner = program_account.owner();
        let program_runtime = self.environments.get_env_for_execution().clone();
        let slot =
            self.sysvar_cache.get_clock().expect("clock sysvar should always be available").slot;

        if bpf_loader::check_id(owner) || bpf_loader_deprecated::check_id(owner) {
            ProgramCacheEntry::new(
                owner,
                program_runtime,
                slot,
                slot,
                program_account.data(),
                program_account.data().len(),
                metrics,
            )
            .map_err(|e| {
                tracing::error!("Failed to load program: {e:?}");
                InstructionError::InvalidAccountData
            })
        } else if bpf_loader_upgradeable::check_id(owner) {
            let Ok(UpgradeableLoaderState::Program { programdata_address }) =
                program_account.state()
            else {
                tracing::error!(
                    "Program account data does not deserialize to UpgradeableLoaderState::Program"
                );
                return Err(InstructionError::InvalidAccountData);
            };
            let Some(programdata_account) = self.get_account(&programdata_address) else {
                return Ok(ProgramCacheEntry::new_tombstone(
                    slot,
                    ProgramCacheEntryOwner::LoaderV3,
                    ProgramCacheEntryType::Closed,
                ));
            };
            let program_data = programdata_account.data();
            if let Some(programdata) =
                program_data.get(UpgradeableLoaderState::size_of_programdata_metadata()..)
            {
                ProgramCacheEntry::new(
                    owner,
                    program_runtime,
                    slot,
                    slot,
                    programdata,
                    program_account
                        .data()
                        .len()
                        .saturating_add(program_data.len()),
                    metrics).map_err(|e| {
                        tracing::error!("Error encountered when calling ProgramCacheEntry::new() for bpf_loader_upgradeable: {e:?}");
                        InstructionError::InvalidAccountData
                    })
            } else {
                tracing::error!("Index out of bounds using bpf_loader_upgradeable.");
                Err(InstructionError::InvalidAccountData)
            }
        } else if loader_v4::check_id(owner) {
            if let Some(elf_bytes) =
                program_account.data().get(LoaderV4State::program_data_offset()..)
            {
                ProgramCacheEntry::new(
                    &loader_v4::id(),
                    program_runtime,
                    slot,
                    slot,
                    elf_bytes,
                    program_account.data().len(),
                    metrics,
                )
                .map_err(|_| {
                    tracing::error!(
                        "Error encountered when calling LoadedProgram::new() for loader_v4."
                    );
                    InstructionError::InvalidAccountData
                })
            } else {
                tracing::error!("Index out of bounds using loader_v4.");
                Err(InstructionError::InvalidAccountData)
            }
        } else {
            tracing::error!("Owner does not match any expected loader.");
            Err(InstructionError::IncorrectProgramId)
        }
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
            let slot_hashes = self
                .sysvar_cache
                .get_slot_hashes()
                .expect("slot hashes sysvar should always be available");
            let current_slot = self
                .sysvar_cache
                .get_clock()
                .expect("clock sysvar should always be available")
                .slot;
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

impl AddressLoader for &AccountsDb {
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::account_source::{AccountSource, AccountSourceError};

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
        let mut db = AccountsDb::default();
        db.set_account_source(Arc::new(source.clone()));
        let pubkeys: Vec<Address> = (0..5).map(|i| Address::new_from_array([i; 32])).collect();

        let _ = db.get_accounts_batch(&pubkeys);

        assert_eq!(source.batch_calls.load(Ordering::SeqCst), 1);
    }
}
