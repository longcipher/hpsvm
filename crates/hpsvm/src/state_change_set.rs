//! [`StateChangeSet`]: a serializable slice of the account journal.
//!
//! The single-transaction path stages state eagerly into the journaled
//! overlay (`ExecutedTx`), but the parallel batch path collects its
//! per-worker mutations as owned `StateChangeSet` values and merges them
//! through the same journal via [`crate::commit::apply_commit_delta`] (which
//! becomes `journal::extend_from_set`).
//!
//! A `StateChangeSet` is therefore an ordered journal slice rather than a
//! `HashMap`: replay order matters if a later merged stage is ever reverted.

use solana_account::AccountSharedData;
use solana_address::Address;
use solana_signature::Signature;

use crate::types::TransactionResult;

/// An ordered slice of journal mutations produced by one transaction
/// execution (typically a parallel batch worker).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StateChangeSet {
    /// Writable post-execution account snapshot, in commit order.
    pub post_accounts: Vec<(Address, AccountSharedData)>,
    /// Transaction-history entry to record when history is enabled.
    pub history_entry: Option<(Signature, TransactionResult)>,
}

impl StateChangeSet {
    /// Build the result + change set that corresponds to a detached execution
    /// outcome, mirroring the commit pipeline's detached-to-result conversion.
    ///
    /// The transaction result is returned alongside the set so callers can
    /// surface it to users without recomputing it.
    pub fn from_detached(
        detached: crate::executed::DetachedTx,
        history_enabled: bool,
    ) -> (crate::types::TransactionResult, Self) {
        crate::commit::detached_into_result_and_state_change_set(detached, history_enabled)
    }

    /// Builds an empty change set.
    #[must_use]
    pub const fn new() -> Self {
        Self { post_accounts: Vec::new(), history_entry: None }
    }

    /// Builds a change set from its two parts.
    #[must_use]
    pub const fn from_parts(
        post_accounts: Vec<(Address, AccountSharedData)>,
        history_entry: Option<(Signature, TransactionResult)>,
    ) -> Self {
        Self { post_accounts, history_entry }
    }

    /// Returns `true` when applying this set would mutate account state or
    /// transaction history.
    #[must_use]
    pub const fn mutates_state(&self) -> bool {
        !self.post_accounts.is_empty() || self.history_entry.is_some()
    }
}

impl Default for StateChangeSet {
    fn default() -> Self {
        Self::new()
    }
}
