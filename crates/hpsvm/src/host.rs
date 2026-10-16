//! Typed host boundary for the VM type family.
//!
//! Design §3.3 calls for a `Host<T>` trait exposing version, block,
//! account, sysvar, program-cache and log access. Only the subset expressible
//! with the types that exist today is implemented here:
//!
//! * `Version` does NOT exist yet (Task 2.1 owns it), so there is no `version()` accessor.
//! * `BlockEnvExt` (the `T::BlockEnvExt` slot) selects the `ext` carried by [`BlockEnv`], so
//!   `block_env()` returns `BlockEnv<T::BlockEnvExt>`.
//! * `DbResult`, `LoadError` and `AccountHandle` do NOT exist yet (Task 7.1 owns the error model),
//!   so account/sysvar methods bind to the real existing types: [`Account`], [`HPSVMError`], and
//!   the `Svm` sysvar bounds (`Default + Serialize + SysvarId` for writes, `Sysvar + SysvarId +
//!   DeserializeOwned` for reads).
//! * `ProgramCache` does NOT exist as a named type yet, so program-cache presence is exposed as
//!   `has_program()`, delegating to the existing `AccountsDb::has_program_cache_entry`.
//! * `log()` has no backing sink yet; it forwards to `tracing::info!` so the call is observable
//!   through the existing tracing setup without inventing new state.

use serde::{Serialize, de::DeserializeOwned};
use solana_account::Account;
use solana_address::Address;
use solana_sysvar::Sysvar;
use solana_sysvar_id::SysvarId;

use crate::{BlockEnv, Svm, SvmTypesHost, error::HPSVMError};

/// Host capabilities a type family runs against.
///
/// The trait is parameterized by the family so a custom family exposes its
/// own slots through this boundary instead of forcing a downcast. The base
/// implementation is [`Svm`] itself (see `SvmTypesHost::Host`).
pub trait Host<T: SvmTypesHost> {
    /// Returns a copy of the current block environment.
    fn block_env(&self) -> BlockEnv<T::BlockEnvExt>;

    /// Returns the account at `addr`, or `None` when it does not exist.
    fn account(&self, addr: &Address) -> Option<Account>;

    /// Writes `account` at `addr`, mirroring [`Svm::set_account`].
    fn set_account(&mut self, addr: Address, account: Account) -> Result<(), HPSVMError>;

    /// Reads a sysvar, mirroring [`Svm::get_sysvar`].
    fn sysvar<S>(&self) -> S
    where
        S: Sysvar + SysvarId + DeserializeOwned;

    /// Overwrites a sysvar, mirroring [`Svm::set_sysvar`].
    fn set_sysvar<S>(&mut self, sysvar: &S) -> Result<(), HPSVMError>
    where
        S: Default + Serialize + SysvarId;

    /// Returns whether `program_id` has a loaded program-cache entry.
    fn has_program(&self, program_id: &Address) -> bool;

    /// Records a host log message.
    ///
    /// Deferred: there is no dedicated log sink yet, so this forwards to
    /// `tracing::info!`. A later task may back it with a capturable buffer.
    fn log(&mut self, message: &str);
}

impl<T: SvmTypesHost> Host<T> for Svm<'_, T> {
    fn block_env(&self) -> BlockEnv<T::BlockEnvExt> {
        Svm::block_env(self)
    }

    fn account(&self, addr: &Address) -> Option<Account> {
        Svm::get_account(self, addr)
    }

    fn set_account(&mut self, addr: Address, account: Account) -> Result<(), HPSVMError> {
        Svm::set_account(self, addr, account)
    }

    fn sysvar<S>(&self) -> S
    where
        S: Sysvar + SysvarId + DeserializeOwned,
    {
        Svm::get_sysvar(self)
    }

    fn set_sysvar<S>(&mut self, sysvar: &S) -> Result<(), HPSVMError>
    where
        S: Default + Serialize + SysvarId,
    {
        Svm::set_sysvar(self, sysvar)
    }

    fn has_program(&self, program_id: &Address) -> bool {
        self.accounts.has_program_cache_entry(program_id)
    }

    fn log(&mut self, message: &str) {
        tracing::info!(target: "hpsvm::host", "{message}");
    }
}

#[cfg(test)]
mod tests {
    use solana_address::Address;
    use solana_clock::Clock;
    use solana_system_interface::program as system_program;

    use super::*;
    use crate::BaseSvmTypes;

    fn test_account(lamports: u64) -> Account {
        Account {
            lamports,
            data: Vec::new(),
            owner: system_program::id(),
            executable: false,
            rent_epoch: 0,
        }
    }

    fn account_via_host<H: Host<BaseSvmTypes>>(host: &H, addr: &Address) -> Option<Account> {
        host.account(addr)
    }

    fn block_via_host<H: Host<BaseSvmTypes>>(host: &H) -> BlockEnv<()> {
        host.block_env()
    }

    fn sysvar_via_host<H: Host<BaseSvmTypes>>(host: &H) -> Clock {
        host.sysvar::<Clock>()
    }

    fn has_program_via_host<H: Host<BaseSvmTypes>>(host: &H, program_id: &Address) -> bool {
        host.has_program(program_id)
    }

    #[test]
    fn host_is_implemented_for_the_base_svm() {
        fn uses_host<H: Host<BaseSvmTypes>>() {}
        uses_host::<Svm<'_, BaseSvmTypes>>();
    }

    #[test]
    fn block_env_matches_the_vm_block_env_through_a_generic_host() {
        let svm = Svm::<BaseSvmTypes>::new();
        let via_host = block_via_host(&svm);
        assert_eq!(via_host, svm.block_env());
        assert_eq!(via_host.latest_blockhash, svm.latest_blockhash());
    }

    #[test]
    fn account_read_returns_none_for_a_missing_address_through_a_generic_host() {
        let svm = Svm::<BaseSvmTypes>::new();
        let missing = Address::new_unique();
        assert!(account_via_host(&svm, &missing).is_none());
    }

    #[test]
    fn account_write_then_read_round_trips_exact_lamports_through_a_generic_host() {
        let mut svm = Svm::<BaseSvmTypes>::new();
        let addr = Address::new_unique();
        Host::<BaseSvmTypes>::set_account(&mut svm, addr, test_account(777)).unwrap();
        let stored = account_via_host(&svm, &addr).unwrap();
        assert_eq!(stored.lamports, 777);
        assert_eq!(stored.owner, system_program::id());
    }

    #[test]
    fn sysvar_read_returns_the_default_clock_through_a_generic_host() {
        let svm = Svm::<BaseSvmTypes>::new();
        let clock = sysvar_via_host(&svm);
        assert_eq!(clock, Clock::default());
        assert_eq!(clock.slot, 0);
    }

    #[test]
    fn sysvar_write_then_read_round_trips_the_timestamp_through_a_generic_host() {
        let mut svm = Svm::<BaseSvmTypes>::new();
        let mut clock: Clock = Host::<BaseSvmTypes>::sysvar(&svm);
        assert_eq!(clock.unix_timestamp, Clock::default().unix_timestamp);
        clock.unix_timestamp = 50;
        Host::<BaseSvmTypes>::set_sysvar(&mut svm, &clock).unwrap();
        let reread: Clock = sysvar_via_host(&svm);
        assert_eq!(reread.unix_timestamp, 50);
        assert_eq!(reread.slot, clock.slot);
    }

    #[test]
    fn has_program_is_false_for_an_unknown_program_through_a_generic_host() {
        let svm = Svm::<BaseSvmTypes>::new();
        assert!(!has_program_via_host(&svm, &Address::new_unique()));
    }

    #[test]
    fn has_program_is_true_after_adding_a_builtin_through_a_generic_host() {
        let svm = Svm::<BaseSvmTypes>::new();
        let system_id = solana_system_interface::program::id();
        assert!(
            has_program_via_host(&svm, &system_id),
            "a fresh VM loads the system builtin into the program cache"
        );
    }

    #[test]
    fn log_accepts_a_message_without_changing_vm_state() {
        let mut svm = Svm::<BaseSvmTypes>::new();
        let before = svm.block_env();
        Host::<BaseSvmTypes>::log(&mut svm, "hello from host");
        assert_eq!(svm.block_env(), before);
    }
}
