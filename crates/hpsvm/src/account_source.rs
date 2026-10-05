use solana_account::AccountSharedData;
use solana_address::Address;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AccountSourceErrorKind {
    Unavailable,
    InvalidResponse,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{kind:?}: {message}")]
pub struct AccountSourceError {
    kind: AccountSourceErrorKind,
    message: String,
}

impl AccountSourceError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { kind: AccountSourceErrorKind::Unavailable, message: message.into() }
    }

    pub fn with_kind(kind: AccountSourceErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }

    pub const fn kind(&self) -> AccountSourceErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

pub trait AccountSource: Send + Sync {
    fn get_account(
        &self,
        pubkey: &Address,
    ) -> Result<Option<AccountSharedData>, AccountSourceError>;

    fn get_accounts(
        &self,
        pubkeys: &[Address],
    ) -> Result<Vec<Option<AccountSharedData>>, AccountSourceError> {
        pubkeys.iter().map(|pk| self.get_account(pk)).collect()
    }
}

#[derive(Clone, Default)]
pub(crate) struct EmptyAccountSource;

impl AccountSource for EmptyAccountSource {
    fn get_account(
        &self,
        _pubkey: &Address,
    ) -> Result<Option<AccountSharedData>, AccountSourceError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use solana_account::{Account, ReadableAccount};

    use super::*;

    /// Source that serves a fixed key -> account map so both the trait's default
    /// `get_accounts` loop and custom overrides can be exercised.
    struct MapSource {
        accounts: Vec<(Address, AccountSharedData)>,
        /// When set, `get_account` fails with this error for the key at this index.
        fail_at: Option<(usize, AccountSourceError)>,
    }

    impl AccountSource for MapSource {
        fn get_account(
            &self,
            pubkey: &Address,
        ) -> Result<Option<AccountSharedData>, AccountSourceError> {
            if let Some((index, error)) = &self.fail_at &&
                let Some(slot) = self.accounts.iter().position(|(key, _)| key == pubkey) &&
                slot == *index
            {
                return Err(error.clone());
            }
            Ok(self.accounts.iter().find(|(key, _)| key == pubkey).map(|(_, data)| data.clone()))
        }
    }

    /// A distinctive account so assertions can tell accounts apart.
    fn account(lamports: u64) -> AccountSharedData {
        let mut account = Account::new(lamports, 0, &Address::new_unique());
        account.executable = true;
        AccountSharedData::from(account)
    }

    #[test]
    fn new_defaults_to_the_unavailable_kind() {
        let error = AccountSourceError::new("timed out");
        assert_eq!(error.kind(), AccountSourceErrorKind::Unavailable);
        assert_eq!(error.message(), "timed out");
    }

    #[test]
    fn with_kind_preserves_an_explicit_kind() {
        for kind in [
            AccountSourceErrorKind::Unavailable,
            AccountSourceErrorKind::InvalidResponse,
            AccountSourceErrorKind::Other,
        ] {
            let error = AccountSourceError::with_kind(kind, "boom");
            assert_eq!(error.kind(), kind);
            assert_eq!(error.to_string(), format!("{kind:?}: boom"));
        }
    }

    #[test]
    fn display_combines_the_debug_kind_and_the_message() {
        assert_eq!(
            AccountSourceError::with_kind(AccountSourceErrorKind::InvalidResponse, "bad body")
                .to_string(),
            "InvalidResponse: bad body"
        );
        assert_eq!(
            AccountSourceError::with_kind(AccountSourceErrorKind::Other, "custom").to_string(),
            "Other: custom"
        );
    }

    #[test]
    fn empty_account_source_always_reports_a_missing_account() {
        let source = EmptyAccountSource;
        assert!(source.get_account(&Address::new_unique()).unwrap().is_none());

        // `get_accounts` returns one slot per requested key, so the batch is
        // the same length as the request rather than empty.
        let batch = source
            .get_accounts(&[Address::new_unique(), Address::new_unique()])
            .expect("an empty source never fails");
        assert_eq!(batch.len(), 2);
        assert!(batch.iter().all(Option::is_none));
    }

    #[test]
    fn default_get_accounts_preserves_order_and_length() {
        let keys: Vec<Address> = (0..4).map(|_| Address::new_unique()).collect();
        let source = MapSource {
            accounts: vec![(keys[1], account(11)), (keys[3], account(33))],
            fail_at: None,
        };

        let resolved = source.get_accounts(&keys).unwrap();

        assert_eq!(resolved.len(), keys.len());
        assert!(resolved[0].is_none());
        assert_eq!(resolved[1].as_ref().map(|a| a.lamports()), Some(11));
        assert!(resolved[2].is_none());
        assert_eq!(resolved[3].as_ref().map(|a| a.lamports()), Some(33));
    }

    #[test]
    fn default_get_accounts_propagates_the_first_error() {
        let keys: Vec<Address> = (0..3).map(|_| Address::new_unique()).collect();
        let source = MapSource {
            accounts: vec![(keys[0], account(1)), (keys[1], account(2)), (keys[2], account(3))],
            fail_at: Some((1, AccountSourceError::new("read timed out"))),
        };

        let error = source.get_accounts(&keys).unwrap_err();
        assert_eq!(error.kind(), AccountSourceErrorKind::Unavailable);
        assert_eq!(error.message(), "read timed out");
    }

    #[test]
    fn default_get_accounts_on_an_empty_key_list_is_empty() {
        let source = MapSource { accounts: Vec::new(), fail_at: None };
        assert!(source.get_accounts(&[]).unwrap().is_empty());
    }

    // The default `get_accounts` must behave exactly like a per-key loop:
    // same length and same per-index result, for any key list.
    proptest! {
        #[test]
        fn default_get_accounts_matches_an_explicit_per_key_loop(present in prop::collection::vec(any::<bool>(), 0..12)) {
            let keys: Vec<Address> = (0..present.len()).map(|_| Address::new_unique()).collect();
            let source = MapSource {
                accounts: keys
                    .iter()
                    .zip(&present)
                    .filter(|(_, present)| **present)
                    .map(|(key, _)| (*key, account(7)))
                    .collect(),
                fail_at: None,
            };

            let batched = source.get_accounts(&keys).unwrap();
            let per_key: Vec<Option<AccountSharedData>> =
                keys.iter().map(|key| source.get_account(key).unwrap()).collect();

            prop_assert_eq!(batched.len(), keys.len());
            prop_assert_eq!(batched, per_key);
        }
    }

    // A failing key must abort the default batch at that index: every earlier
    // key resolves successfully and the error is surfaced verbatim.
    proptest! {
        #[test]
        fn default_get_accounts_short_circuits_at_the_failing_key(fail_at in 0usize..8, keys_len in 1usize..9) {
            let keys: Vec<Address> = (0..keys_len).map(|_| Address::new_unique()).collect();
            let failing_key = fail_at % keys_len;
            let source = MapSource {
                accounts: keys.iter().map(|key| (*key, account(5))).collect(),
                fail_at: Some((failing_key, AccountSourceError::with_kind(AccountSourceErrorKind::Other, "nope"))),
            };

            let error = source.get_accounts(&keys).unwrap_err();
            prop_assert_eq!(error.kind(), AccountSourceErrorKind::Other);
            prop_assert_eq!(error.message(), "nope");

            // Keys before the failing one still resolve on their own.
            for key in &keys[..failing_key] {
                prop_assert!(source.get_account(key).is_ok());
            }
        }
    }
}
