use std::num::NonZeroUsize;

use lru::LruCache;
use solana_signature::Signature;

use crate::types::TransactionResult;

/// Transaction history with bounded LRU eviction.
///
/// Wraps [`lru::LruCache`] so inserts beyond `max_entries` evict the
/// least-recently-used entry in O(1) instead of the previous O(n) shift.
/// When `max_entries == 0` the history is disabled and all inserts are
/// silently dropped.
#[derive(Clone, Debug)]
pub(crate) struct TransactionHistory {
    /// Bounded LRU cache. When `max_entries == 0` this stays empty.
    entries: LruCache<Signature, TransactionResult>,
    /// Configured capacity. `0` disables history.
    max_entries: usize,
}

impl TransactionHistory {
    /// Creates a new transaction history with the default capacity of 32.
    pub(crate) fn new() -> Self {
        Self::with_capacity(32)
    }

    /// Creates a new transaction history with the given capacity.
    fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: LruCache::new(NonZeroUsize::new(capacity.max(1)).expect("max(1) is non-zero")),
            max_entries: capacity,
        }
    }

    /// Updates the capacity. When `new_cap == 0`, history is disabled
    /// and existing entries are cleared. Otherwise, the cache is resized
    /// to the new capacity, evicting LRU entries as needed.
    pub(crate) fn set_capacity(&mut self, new_cap: usize) {
        if new_cap == 0 {
            self.entries.clear();
            self.max_entries = 0;
            return;
        }
        let new_size = NonZeroUsize::new(new_cap).expect("new_cap is non-zero here");
        self.entries.resize(new_size);
        self.max_entries = new_cap;
    }

    /// Returns the result of a previously processed transaction, if any.
    /// Uses `peek` so lookups do not affect LRU ordering.
    pub(crate) fn get_transaction(&self, signature: &Signature) -> Option<&TransactionResult> {
        if self.max_entries == 0 {
            return None;
        }
        self.entries.peek(signature)
    }

    /// Records a processed transaction. No-op when history is disabled.
    pub(crate) fn add_new_transaction(&mut self, signature: Signature, result: TransactionResult) {
        if self.max_entries == 0 {
            return;
        }
        // `put` evicts the LRU entry if at capacity, keeping the cache bounded.
        self.entries.put(signature, result);
    }

    /// Returns whether the signature is present in history.
    pub(crate) fn check_transaction(&self, signature: &Signature) -> bool {
        if self.max_entries == 0 {
            return false;
        }
        self.entries.peek(signature).is_some()
    }
}

#[cfg(test)]
mod tests {
    use solana_transaction_error::TransactionError;

    use super::*;
    use crate::types::{FailedTransactionMetadata, TransactionMetadata};

    #[test]
    fn set_capacity_limits_history_by_entry_count() {
        let mut history = TransactionHistory::new();
        history.set_capacity(1);

        let first = Signature::from([1; 64]);
        let second = Signature::from([2; 64]);
        let result = Ok(TransactionMetadata::default());

        history.add_new_transaction(first, result.clone());
        history.add_new_transaction(second, result);

        assert!(!history.check_transaction(&first));
        assert!(history.check_transaction(&second));
    }

    #[test]
    fn zero_capacity_disables_history_storage() {
        let mut history = TransactionHistory::new();
        history.set_capacity(0);

        let signature = Signature::from([3; 64]);
        history.add_new_transaction(
            signature,
            Err(FailedTransactionMetadata {
                err: TransactionError::AlreadyProcessed,
                meta: TransactionMetadata::default(),
            }),
        );

        assert!(!history.check_transaction(&signature));
        assert!(history.get_transaction(&signature).is_none());
    }
}
