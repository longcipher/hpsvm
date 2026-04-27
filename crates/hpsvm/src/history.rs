use indexmap::IndexMap;
use solana_signature::Signature;

use crate::types::TransactionResult;

#[derive(Clone)]
pub struct TransactionHistory {
    entries: IndexMap<Signature, TransactionResult>,
    max_entries: usize,
}

impl TransactionHistory {
    pub fn new() -> Self {
        Self { entries: IndexMap::with_capacity(32), max_entries: 32 }
    }

    pub fn set_capacity(&mut self, new_cap: usize) {
        self.max_entries = new_cap;
        while self.entries.len() > self.max_entries {
            self.entries.shift_remove_index(0);
        }
    }

    pub fn get_transaction(&self, signature: &Signature) -> Option<&TransactionResult> {
        self.entries.get(signature)
    }

    pub fn add_new_transaction(&mut self, signature: Signature, result: TransactionResult) {
        if self.max_entries != 0 {
            if self.entries.len() == self.max_entries {
                self.entries.shift_remove_index(0);
            }
            self.entries.insert(signature, result);
        }
    }

    pub fn check_transaction(&self, signature: &Signature) -> bool {
        self.entries.contains_key(signature)
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
