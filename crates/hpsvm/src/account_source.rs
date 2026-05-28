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
