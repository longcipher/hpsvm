//! State change sink: lets tooling observe account changes as they commit,
//! without a second pass over live state.
//!
//! `ExecutedTx` records the per-address before/after deltas at staging time
//! (see [`AccountChangeRecord`]) and streams them through a [`StateChangeSink`]
//! during [`ExecutedTx::commit_with`](crate::ExecutedTx::commit_with). The
//! changes are owned by the staged transaction, so streaming them never
//! re-reads the overlay.

use solana_account::AccountSharedData;
use solana_address::Address;

/// A single account's before/after state across one transaction.
///
/// `before` is `None` when the account did not exist in the overlay before
/// staging; `after` is `None` when the account was removed by the transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountChange<'a> {
    /// The account address.
    pub address: Address,
    /// Account state before staging, if any.
    pub before: Option<&'a AccountSharedData>,
    /// Account state after staging, if any.
    pub after: Option<&'a AccountSharedData>,
}

/// Owned record of an account change, retained by a staged transaction.
#[derive(Clone, Debug)]
pub struct AccountChangeRecord {
    /// The account address.
    pub address: Address,
    /// Account state before staging, if any.
    pub before: Option<AccountSharedData>,
    /// Account state after staging, if any.
    pub after: Option<AccountSharedData>,
}

/// Observer for state changes committed by a transaction.
///
/// Implementations receive each journaled account change exactly once, in
/// staging order, before the commit is finalized. Returning `Err` from any
/// hook aborts the commit and rolls the staged state back.
pub trait StateChangeSink {
    /// Error type returned when an observed change cannot be ingested.
    type Error;

    /// Called once per changed account, with the before/after states.
    fn on_account_change(&mut self, change: AccountChange<'_>) -> Result<(), Self::Error>;
}
