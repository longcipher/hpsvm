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

    /// Returns whether history recording is active (capacity > 0).
    ///
    /// Used by the commit path to skip building a history entry (and the
    /// associated `TransactionResult` clone) entirely when history is disabled.
    #[inline]
    pub(crate) const fn is_enabled(&self) -> bool {
        self.max_entries != 0
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use solana_transaction_error::TransactionError;

    use super::*;
    use crate::types::{FailedTransactionMetadata, TransactionMetadata};

    /// Builds a deterministic, unique-per-index signature.
    fn signature(seed: u8) -> Signature {
        Signature::from([seed; 64])
    }

    fn ok_result() -> TransactionResult {
        Ok(TransactionMetadata::default())
    }

    fn err_result(err: TransactionError) -> TransactionResult {
        Err(FailedTransactionMetadata { err, meta: TransactionMetadata::default() })
    }

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

    #[test]
    fn new_history_is_enabled_with_the_default_capacity() {
        let history = TransactionHistory::new();
        assert!(history.is_enabled());
        assert_eq!(history.max_entries, 32);
    }

    #[test]
    fn get_transaction_returns_the_stored_failure_variant() {
        let mut history = TransactionHistory::new();
        let sig = signature(9);
        history.add_new_transaction(sig, err_result(TransactionError::AlreadyProcessed));

        match history.get_transaction(&sig) {
            Some(Err(meta)) => assert!(matches!(meta.err, TransactionError::AlreadyProcessed)),
            other => panic!("expected a stored Err, got {other:?}"),
        }
        assert!(history.check_transaction(&sig));
    }

    #[test]
    fn get_transaction_of_an_unknown_signature_is_none() {
        let history = TransactionHistory::new();
        assert!(history.get_transaction(&signature(1)).is_none());
        assert!(!history.check_transaction(&signature(1)));
    }

    #[test]
    fn raising_the_capacity_preserves_existing_entries() {
        let mut history = TransactionHistory::new();
        history.set_capacity(1);
        history.add_new_transaction(signature(1), ok_result());

        history.set_capacity(8);

        assert!(history.is_enabled());
        assert_eq!(history.max_entries, 8);
        assert!(history.check_transaction(&signature(1)));
        assert!(history.get_transaction(&signature(1)).is_some());
    }

    #[test]
    fn shrinking_the_capacity_evicts_least_recently_used_entries() {
        let mut history = TransactionHistory::new();
        history.set_capacity(4);
        for seed in 0..4 {
            history.add_new_transaction(signature(seed), ok_result());
        }

        history.set_capacity(2);

        // Shrinking keeps the most-recently-used half: entries 0 and 1 were
        // inserted first, so they are the ones evicted.
        assert!(!history.check_transaction(&signature(0)));
        assert!(!history.check_transaction(&signature(1)));
        assert!(history.check_transaction(&signature(2)));
        assert!(history.check_transaction(&signature(3)));
    }

    #[test]
    fn re_enabling_after_zero_capacity_stores_again() {
        let mut history = TransactionHistory::new();
        history.set_capacity(0);
        assert!(!history.is_enabled());

        history.set_capacity(4);

        assert!(history.is_enabled());
        assert_eq!(history.max_entries, 4);
        history.add_new_transaction(signature(5), ok_result());
        assert!(history.check_transaction(&signature(5)));
    }

    #[test]
    fn disabling_history_twice_is_idempotent() {
        let mut history = TransactionHistory::new();
        history.set_capacity(0);
        history.set_capacity(0);
        assert!(!history.is_enabled());
        history.add_new_transaction(signature(1), ok_result());
        assert!(!history.check_transaction(&signature(1)));
    }

    #[test]
    fn capacity_one_keeps_only_the_most_recent_entry() {
        let mut history = TransactionHistory::with_capacity(1);
        history.add_new_transaction(signature(1), ok_result());
        history.add_new_transaction(signature(2), ok_result());
        history.add_new_transaction(signature(3), ok_result());

        assert!(!history.check_transaction(&signature(1)));
        assert!(!history.check_transaction(&signature(2)));
        assert!(history.check_transaction(&signature(3)));
    }

    #[test]
    fn peeking_does_not_change_lru_ordering() {
        let mut history = TransactionHistory::with_capacity(2);
        history.add_new_transaction(signature(1), ok_result());
        history.add_new_transaction(signature(2), ok_result());

        // Touch the oldest entry via `peek`; `peek` must not promote it, so
        // inserting a third entry still evicts entry 1.
        assert!(history.get_transaction(&signature(1)).is_some());
        history.add_new_transaction(signature(3), ok_result());

        assert!(!history.check_transaction(&signature(1)));
        assert!(history.check_transaction(&signature(2)));
        assert!(history.check_transaction(&signature(3)));
    }

    // Property: with a fixed capacity the cache retains exactly the last
    // `capacity` inserted signatures, in the order they were added.
    proptest! {
        #[test]
        fn history_retains_exactly_the_last_capacity_entries(
            capacity in 1usize..8,
            total in 0usize..24,
        ) {
            let mut history = TransactionHistory::with_capacity(capacity);
            for index in 0..total {
                history.add_new_transaction(signature(index as u8), ok_result());
            }

            let first_kept = total.saturating_sub(capacity);
            for index in 0..total {
                if index < first_kept {
                    prop_assert!(
                        !history.check_transaction(&signature(index as u8)),
                        "seed {index}"
                    );
                } else {
                    prop_assert!(
                        history.check_transaction(&signature(index as u8)),
                        "seed {index}"
                    );
                }
            }
        }
    }

    // Property: while disabled, nothing is ever recorded — for any capacity
    // transition sequence that lands on zero.
    proptest! {
        #[test]
        fn disabled_history_never_records(
            capacity in 1usize..6,
            writes in 0usize..10,
        ) {
            let mut history = TransactionHistory::with_capacity(capacity);
            history.set_capacity(0);

            for index in 0..writes {
                history.add_new_transaction(signature(index as u8), ok_result());
            }

            prop_assert!(!history.is_enabled());
            prop_assert_eq!(history.max_entries, 0);
            for index in 0..writes {
                prop_assert!(!history.check_transaction(&signature(index as u8)));
            }
        }
    }

    // Property: capacity is idempotent under repeated assignment, and disabling
    // is idempotent once it is off.
    proptest! {
        #[test]
        fn capacity_assignment_is_idempotent(
            capacity in 1usize..8,
            repeats in 1usize..5,
        ) {
            let mut history = TransactionHistory::new();
            for _ in 0..repeats {
                history.set_capacity(capacity);
            }
            prop_assert!(history.is_enabled());
            prop_assert_eq!(history.max_entries, capacity);

            history.set_capacity(0);
            for _ in 0..repeats {
                history.set_capacity(0);
            }
            prop_assert!(!history.is_enabled());
            prop_assert_eq!(history.max_entries, 0);
        }
    }
}
