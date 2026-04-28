use solana_account::AccountSharedData;
use solana_address::Address;

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct AccountSourceError {
    message: String,
}

impl AccountSourceError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
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
