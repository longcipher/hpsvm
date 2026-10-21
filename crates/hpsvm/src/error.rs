use std::sync::Arc;

use solana_address::Address;
use solana_transaction::InstructionError;
use solana_transaction_error::TransactionError;
use thiserror::Error;

use crate::{AccountSourceError, account_source::AccountSourceErrorKind, tx_registry::TxTag};

/// Errors related to invalid sysvar data
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum InvalidSysvarDataError {
    /// Invalid Clock sysvar data
    #[error("Invalid Clock sysvar data.")]
    Clock,
    /// Invalid EpochRewards sysvar data
    #[error("Invalid EpochRewards sysvar data.")]
    EpochRewards,
    /// Invalid EpochSchedule sysvar data
    #[error("Invalid EpochSchedule sysvar data.")]
    EpochSchedule,
    /// Invalid Fees sysvar data
    #[error("Invalid Fees sysvar data.")]
    Fees,
    /// Invalid LastRestartSlot sysvar data
    #[error("Invalid LastRestartSlot sysvar data.")]
    LastRestartSlot,
    /// Invalid RecentBlockhashes sysvar data
    #[error("Invalid RecentBlockhashes sysvar data.")]
    RecentBlockhashes,
    /// Invalid Rent sysvar data
    #[error("Invalid Rent sysvar data.")]
    Rent,
    /// Invalid SlotHashes sysvar data
    #[error("Invalid SlotHashes sysvar data.")]
    SlotHashes,
    /// Invalid StakeHistory sysvar data
    #[error("Invalid StakeHistory sysvar data.")]
    StakeHistory,
}

/// High level SVM errors
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum HPSVMError {
    /// Invalid sysvar data error
    #[error("{0}")]
    InvalidSysvarData(#[from] InvalidSysvarDataError),
    /// Sysvar serialization failure
    #[error("failed to serialize sysvar {sysvar}: {reason}")]
    SysvarSerialization { sysvar: &'static str, reason: String },
    /// Instruction error
    #[error("{0}")]
    Instruction(#[from] InstructionError),
    /// Invalid path error
    #[error("{0}")]
    InvalidPath(#[from] std::io::Error),
    /// Runtime environment refresh failure
    #[error("failed to refresh runtime environment {version}: {reason}")]
    RuntimeEnvironment { version: &'static str, reason: String },
    /// Custom syscall registration failure
    #[error("failed to register custom syscall {name} in {runtime}: {reason}")]
    CustomSyscallRegistration { name: String, runtime: &'static str, reason: String },
    /// Invalid loader error
    #[error("unsupported loader {loader_id} for program {program_id}")]
    InvalidLoader { program_id: Address, loader_id: Address },
    /// Required runtime component was not materialized.
    #[error("missing runtime component: {component}")]
    MissingRuntimeComponent { component: &'static str },
    /// External account source failure.
    #[error("account source failed while loading {pubkey}: {source}")]
    AccountSource {
        /// Account address that triggered the source read.
        pubkey: Address,
        /// Underlying account source error.
        source: AccountSourceError,
    },
    /// Program load failure.
    #[error("failed to load program: {0}")]
    ProgramLoad(String),
    /// No transaction handler was registered for the transaction's tag.
    ///
    /// This is the [`HPSVMError`] projection of
    /// [`TxRegistryError::UnknownTag`](crate::TxRegistryError::UnknownTag),
    /// used where a fallible entry point must report a dispatch failure as an
    /// error rather than an execution status.
    #[error("no transaction handler registered for tag {tag:?}")]
    UnknownTransactionTag {
        /// The tag no handler owned.
        tag: TxTag,
    },
    /// Layered database error.
    ///
    /// Lossless [`DbError`] projection: [`DbError::is_fatal`] and
    /// [`DbError::downcast_ref`] keep working through this variant.
    #[error(transparent)]
    Database(#[from] DbError),
    /// Layered load error.
    #[error(transparent)]
    Load(#[from] LoadError),
    /// Layered transaction error.
    #[error(transparent)]
    Transaction(TxError),
    /// Layered execution error.
    #[error(transparent)]
    Execution(ExecutionError),
    /// Layered host error.
    #[error(transparent)]
    Host(#[from] HostError),
    /// Layered handler error.
    #[error(transparent)]
    Handler(#[from] HandlerError),
}

impl From<Box<dyn std::error::Error>> for HPSVMError {
    fn from(err: Box<dyn std::error::Error>) -> Self {
        Self::ProgramLoad(err.to_string())
    }
}

/// Projects an execution-layer failure onto the crate's outer error.
///
/// The account-source failure keeps its own outer variant, so a caller can
/// still match it directly; every other execution failure is projected
/// transparently.
impl From<ExecutionError> for HPSVMError {
    fn from(error: ExecutionError) -> Self {
        match error {
            ExecutionError::AccountSource { pubkey, source } => {
                Self::AccountSource { pubkey, source }
            }
            other => Self::Execution(other),
        }
    }
}

/// Projects a transaction-layer failure onto the crate's outer error.
///
/// An unroutable envelope keeps its own outer variant, so a caller can
/// still see which tag had no handler.
impl From<TxError> for HPSVMError {
    fn from(error: TxError) -> Self {
        match error {
            TxError::UnknownTag(tag) => Self::UnknownTransactionTag { tag },
            other => Self::Transaction(other),
        }
    }
}

// ===========================================================================
// Layered error taxonomy.
//
// Placement: this taxonomy lives in `error.rs` (rather than a new `errors.rs`
// module) because `error.rs` is already the crate's single error home — it
// owns `HPSVMError` and `InvalidSysvarDataError` — and splitting the new
// layer across a second module would force every reader to chase conversions
// between two homes. One module, one import path (`crate::error::`).
//
// Shape of the migration (binding scope decision): the FULL taxonomy is
// introduced here, but only the DATABASE layer and the TxHandler boundary
// are migrated to it in this task. [`HPSVMError`] stays the crate's
// outer/public error, with a lossless transparent projection (`Database`,
// `Load`, `Transaction`, `Execution`, `Host`, `Handler`) and hence a
// lossless `From` conversion from every new layer. The remaining call sites
// (execution internals, registry `validate`/`execute` signatures, host entry
// points) keep surfacing `HPSVMError` directly; their migration owners are
// documented on the relevant types below.
//
// Classification rule: fatal-ness and layer identity travel in TYPES
// (`DbError::is_fatal`, exact enum variants, `downcast_ref`), never in
// message strings. `Display` impls therefore delegate to the inner error
// wherever a layer adds no context, and add a fixed prefix otherwise.
// ===========================================================================

/// Type-erased error that stays recoverable by downcast.
///
/// The inner error is held behind an `Arc`, so cloning is a pointer bump
/// and the same value can cross the registry's erased handler boundary and
/// still be recovered with [`downcast_ref`](AnyError::downcast_ref).
#[derive(Clone)]
pub struct AnyError(Arc<dyn std::error::Error + Send + Sync>);

impl AnyError {
    /// Erases a concrete error, preserving it for later downcast recovery.
    pub fn new<E>(error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self(Arc::new(error))
    }

    /// Returns `true` when the erased error is of type `T`.
    pub fn is<T>(&self) -> bool
    where
        T: std::error::Error + 'static,
    {
        self.0.is::<T>()
    }

    /// Recovers a reference to the concrete error of type `T`, if present.
    pub fn downcast_ref<T>(&self) -> Option<&T>
    where
        T: std::error::Error + 'static,
    {
        self.0.downcast_ref::<T>()
    }

    /// Consumes the wrapper and returns the erased error.
    pub fn into_inner(self) -> Arc<dyn std::error::Error + Send + Sync> {
        self.0
    }
}

impl std::fmt::Debug for AnyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("AnyError").field(&self.0.to_string()).finish()
    }
}

impl std::fmt::Display for AnyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for AnyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

impl From<HPSVMError> for AnyError {
    fn from(error: HPSVMError) -> Self {
        Self::new(error)
    }
}

impl From<AccountSourceError> for AnyError {
    fn from(error: AccountSourceError) -> Self {
        Self::new(error)
    }
}

impl From<DbError> for AnyError {
    fn from(error: DbError) -> Self {
        Self::new(error)
    }
}

/// Database-layer error: a concrete failure plus its fatal classification.
///
/// `fatal == true` means the failure is infrastructural (the source is
/// down): retrying within the same transaction is pointless. `fatal == false`
/// means the failure is deterministic for this input (bad data, missing
/// file) and must be handled, not retried, at this layer.
#[derive(Clone, Debug)]
pub struct DbError {
    error: AnyError,
    fatal: bool,
}

impl DbError {
    /// Wraps a concrete error with an explicit fatal classification.
    pub fn new<E>(error: E, fatal: bool) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self { error: AnyError::new(error), fatal }
    }

    /// Wraps a concrete infrastructural failure (retrying is pointless).
    pub fn fatal<E>(error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::new(error, true)
    }

    /// Wraps a concrete deterministic failure (handle, do not retry).
    pub fn non_fatal<E>(error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::new(error, false)
    }

    /// Returns the fatal classification. Survives every erased boundary
    /// because it is a plain flag on the recovered value, not a string.
    pub const fn is_fatal(&self) -> bool {
        self.fatal
    }

    /// Returns the erased concrete error.
    pub const fn error(&self) -> &AnyError {
        &self.error
    }

    /// Consumes the wrapper and returns the erased concrete error.
    pub fn into_inner(self) -> AnyError {
        self.error
    }

    /// Recovers a reference to the concrete error of type `T`, if present.
    pub fn downcast_ref<T>(&self) -> Option<&T>
    where
        T: std::error::Error + 'static,
    {
        self.error.downcast_ref::<T>()
    }

    /// Returns `true` when the wrapped error is of type `T`.
    pub fn is<T>(&self) -> bool
    where
        T: std::error::Error + 'static,
    {
        self.error.is::<T>()
    }
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.error)
    }
}

impl std::error::Error for DbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// Fatal mapping for account-source failures: `AccountSourceErrorKind::Unavailable`
/// means the infrastructure is down (retrying is pointless within the
/// transaction), while `InvalidResponse`/`Other` are deterministic outcomes
/// of this request.
impl From<AccountSourceError> for DbError {
    fn from(error: AccountSourceError) -> Self {
        let fatal = matches!(error.kind(), AccountSourceErrorKind::Unavailable);
        Self::new(error, fatal)
    }
}

/// I/O failures arise loading programs and files. A missing file is
/// deterministic for this input, not infrastructure collapse, so the mapping
/// is non-fatal.
impl From<std::io::Error> for DbError {
    fn from(error: std::io::Error) -> Self {
        Self::non_fatal(error)
    }
}

/// Invalid sysvar bytes are deterministic for this input, not fatal.
impl From<InvalidSysvarDataError> for DbError {
    fn from(error: InvalidSysvarDataError) -> Self {
        Self::non_fatal(error)
    }
}

/// Program-cache load failures (`InvalidAccountData`, `IncorrectProgramId`,
/// `MissingAccount`) are deterministic for this input, not fatal.
impl From<InstructionError> for DbError {
    fn from(error: InstructionError) -> Self {
        Self::non_fatal(error)
    }
}

/// Fallible database result. Returned by the migrated database layer
/// (`Accounts::prewarm` in `crate::accounts_db`); other
/// database entry points keep their current signatures until their owning
/// tasks migrate them (see the module docs above).
pub type DbResult<T> = Result<T, DbError>;

/// Load-layer error: which load site produced the [`DbError`].
///
/// The load sites (account reads, program loads and sysvar validation) all
/// report through this type, so the fatal classification of the underlying
/// database failure survives the projection.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum LoadError {
    /// An account read failed at the source.
    Account(DbError),
    /// A program load failed validation.
    Program(DbError),
    /// A managed sysvar failed validation.
    Sysvar(DbError),
}

impl LoadError {
    /// Wraps an account-read failure.
    pub const fn account(error: DbError) -> Self {
        Self::Account(error)
    }

    /// Wraps a program-load failure.
    pub const fn program(error: DbError) -> Self {
        Self::Program(error)
    }

    /// Wraps a sysvar-validation failure.
    pub const fn sysvar(error: DbError) -> Self {
        Self::Sysvar(error)
    }

    /// Returns the underlying database error, whatever the load site.
    pub const fn db_error(&self) -> &DbError {
        match self {
            Self::Account(error) | Self::Program(error) | Self::Sysvar(error) => error,
        }
    }

    /// Delegates to the underlying database error's fatal classification.
    pub const fn is_fatal(&self) -> bool {
        self.db_error().is_fatal()
    }
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Account(error) => write!(f, "failed to load account: {error}"),
            Self::Program(error) => write!(f, "failed to load program: {error}"),
            Self::Sysvar(error) => write!(f, "failed to load sysvar: {error}"),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.db_error())
    }
}

/// Transaction-validation failure.
///
/// A handler rejects a transaction with this type; the rejection is
/// deterministic and never fatal.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum TxValidationError {
    /// The transaction envelope failed validation.
    Invalid(TransactionError),
}

impl TxValidationError {
    /// Returns the underlying transaction error.
    pub fn transaction_error(&self) -> TransactionError {
        match self {
            Self::Invalid(error) => error.clone(),
        }
    }
}

impl From<TransactionError> for TxValidationError {
    fn from(error: TransactionError) -> Self {
        Self::Invalid(error)
    }
}

impl std::fmt::Display for TxValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(error) => write!(f, "transaction validation failed: {error}"),
        }
    }
}

impl std::error::Error for TxValidationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Invalid(error) => Some(error),
        }
    }
}

/// Machine halt reason, without stringly-typed payloads.
///
/// Only real halts observed in this codebase are represented:
/// [`AlreadyProcessed`](HaltReason::AlreadyProcessed) (duplicate transaction,
/// see history and execution paths) and a [`Transaction`](HaltReason::Transaction) wrapper for
/// every other `TransactionError` halt.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum HaltReason {
    /// The transaction was already processed (duplicate).
    AlreadyProcessed,
    /// Any other halt, carrying the exact transaction error.
    Transaction(TransactionError),
}

impl HaltReason {
    /// Returns the transaction error this halt reports on chain.
    pub fn transaction_error(&self) -> TransactionError {
        match self {
            Self::AlreadyProcessed => TransactionError::AlreadyProcessed,
            Self::Transaction(error) => error.clone(),
        }
    }
}

impl From<TransactionError> for HaltReason {
    fn from(error: TransactionError) -> Self {
        match error {
            TransactionError::AlreadyProcessed => Self::AlreadyProcessed,
            other => Self::Transaction(other),
        }
    }
}

impl std::fmt::Display for HaltReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyProcessed => write!(f, "transaction already processed"),
            Self::Transaction(error) => write!(f, "halted: {error}"),
        }
    }
}

impl std::error::Error for HaltReason {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::AlreadyProcessed => None,
            Self::Transaction(error) => Some(error),
        }
    }
}

/// Execution-layer error: database failure, erased fatal, or machine halt.
///
/// The execution pipeline reports through this type, so the fatal
/// classification and the concrete cause survive every boundary they cross.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ExecutionError {
    /// The database layer failed; fatal-ness comes from the inner error.
    Database(DbError),
    /// An external account source failed while loading an account.
    AccountSource {
        /// The account address whose load failed.
        pubkey: Address,
        /// The underlying account source error.
        source: AccountSourceError,
    },
    /// An erased fatal failure; recover the concrete error by downcast.
    Fatal(Box<AnyError>),
    /// The machine halted; the reason carries no strings.
    Halt(HaltReason),
}

impl ExecutionError {
    /// Returns `true` for halts and erased fatals, and delegates to the
    /// inner database error otherwise.
    pub fn is_fatal(&self) -> bool {
        match self {
            Self::Database(error) => error.is_fatal(),
            Self::AccountSource { source, .. } => {
                matches!(source.kind(), AccountSourceErrorKind::Unavailable)
            }
            Self::Fatal(_) | Self::Halt(_) => true,
        }
    }

    /// Returns the database error for the `Database` variant, if present.
    pub const fn db_error(&self) -> Option<&DbError> {
        match self {
            Self::Database(error) => Some(error),
            _ => None,
        }
    }
}

impl From<DbError> for ExecutionError {
    fn from(error: DbError) -> Self {
        Self::Database(error)
    }
}

impl From<HaltReason> for ExecutionError {
    fn from(reason: HaltReason) -> Self {
        Self::Halt(reason)
    }
}

impl From<AnyError> for ExecutionError {
    fn from(error: AnyError) -> Self {
        Self::Fatal(Box::new(error))
    }
}

impl From<HPSVMError> for ExecutionError {
    fn from(error: HPSVMError) -> Self {
        match error {
            HPSVMError::Execution(error) => error,
            HPSVMError::Database(error) => Self::Database(error),
            HPSVMError::AccountSource { pubkey, source } => Self::AccountSource { pubkey, source },
            other => Self::Fatal(Box::new(AnyError::new(other))),
        }
    }
}

impl std::fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => write!(f, "{error}"),
            Self::AccountSource { pubkey, source } => {
                write!(f, "account source failed while loading {pubkey}: {source}")
            }
            Self::Fatal(error) => write!(f, "{error}"),
            Self::Halt(reason) => write!(f, "execution halted: {reason}"),
        }
    }
}

impl std::error::Error for ExecutionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::AccountSource { source, .. } => Some(source),
            Self::Fatal(error) => Some(error.as_ref()),
            Self::Halt(reason) => Some(reason),
        }
    }
}

/// Transaction-layer error: validation, execution, or database.
///
/// This is the error the transaction registry reports: an unroutable
/// envelope, a validation rejection, or an execution failure.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum TxError {
    /// The transaction failed validation (deterministic, never fatal).
    Validation(TxValidationError),
    /// The transaction failed during execution.
    Execution(ExecutionError),
    /// The database layer failed; fatal-ness comes from the inner error.
    Database(DbError),
    /// No handler owns the transaction's tag.
    UnknownTag(TxTag),
}

impl TxError {
    /// Validation rejections are deterministic, never fatal. Execution and
    /// database failures delegate to their inner classification.
    pub fn is_fatal(&self) -> bool {
        match self {
            Self::Validation(_) => false,
            Self::Execution(error) => error.is_fatal(),
            Self::Database(error) => error.is_fatal(),
            Self::UnknownTag(_) => false,
        }
    }

    /// Maps a dispatch failure to the execution status a detached outcome
    /// carries when the failure cannot propagate as an error.
    ///
    /// An unroutable envelope looks, from the chain's perspective, like an
    /// unsupported version; a handler that declines a transaction rejects it
    /// before execution, like a failed sanity check.
    pub(crate) fn transaction_error(&self) -> TransactionError {
        match self {
            Self::UnknownTag(_) => TransactionError::UnsupportedVersion,
            Self::Validation(_) | Self::Execution(_) | Self::Database(_) => {
                TransactionError::SanitizeFailure
            }
        }
    }
}

impl From<TxValidationError> for TxError {
    fn from(error: TxValidationError) -> Self {
        Self::Validation(error)
    }
}

impl From<ExecutionError> for TxError {
    fn from(error: ExecutionError) -> Self {
        Self::Execution(error)
    }
}

impl From<DbError> for TxError {
    fn from(error: DbError) -> Self {
        Self::Database(error)
    }
}

impl std::fmt::Display for TxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(error) => write!(f, "{error}"),
            Self::Execution(error) => write!(f, "{error}"),
            Self::Database(error) => write!(f, "{error}"),
            Self::UnknownTag(tag) => {
                write!(f, "no transaction handler registered for tag {tag:?}")
            }
        }
    }
}

impl std::error::Error for TxError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Validation(error) => Some(error),
            Self::Execution(error) => Some(error),
            Self::Database(error) => Some(error),
            Self::UnknownTag(_) => None,
        }
    }
}

/// Host-layer error: a machine halt or an execution failure.
///
/// The host boundary — the trait every handler and every custom family
/// implements — reports through this type.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum HostError {
    /// The machine halted.
    Halt(HaltReason),
    /// Execution failed.
    Execution(ExecutionError),
}

impl HostError {
    /// Halts are always fatal; execution failures delegate inward.
    pub fn is_fatal(&self) -> bool {
        match self {
            Self::Halt(_) => true,
            Self::Execution(error) => error.is_fatal(),
        }
    }
}

impl From<HaltReason> for HostError {
    fn from(reason: HaltReason) -> Self {
        Self::Halt(reason)
    }
}

impl From<ExecutionError> for HostError {
    fn from(error: ExecutionError) -> Self {
        Self::Execution(error)
    }
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Halt(reason) => write!(f, "{reason}"),
            Self::Execution(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for HostError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Halt(reason) => Some(reason),
            Self::Execution(error) => Some(error),
        }
    }
}

/// Handler-layer error: the erased failure of one `TxHandler`.
///
/// This is the boundary type the registry's erased dispatch uses: a
/// handler's concrete failure is erased here and recovered later by
/// [`downcast_ref`](HandlerError::downcast_ref).
#[derive(Clone)]
pub struct HandlerError(AnyError);

impl HandlerError {
    /// Erases a handler failure, preserving it for later downcast recovery.
    pub fn new<E>(error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self(AnyError::new(error))
    }

    /// Consumes the wrapper and returns the erased error.
    pub fn into_inner(self) -> AnyError {
        self.0
    }

    /// Recovers a reference to the concrete error of type `T`, if present.
    pub fn downcast_ref<T>(&self) -> Option<&T>
    where
        T: std::error::Error + 'static,
    {
        self.0.downcast_ref::<T>()
    }

    /// Returns `true` when the wrapped error is of type `T`.
    pub fn is<T>(&self) -> bool
    where
        T: std::error::Error + 'static,
    {
        self.0.is::<T>()
    }
}

impl std::fmt::Debug for HandlerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("HandlerError").field(&self.0.to_string()).finish()
    }
}

impl std::fmt::Display for HandlerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for HandlerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

impl From<HPSVMError> for HandlerError {
    fn from(error: HPSVMError) -> Self {
        Self::new(error)
    }
}

impl From<AnyError> for HandlerError {
    fn from(error: AnyError) -> Self {
        Self(error)
    }
}

#[cfg(test)]
mod tests {
    use solana_transaction::InstructionError;

    use super::*;
    use crate::account_source::AccountSourceErrorKind;

    /// Every `InvalidSysvarDataError` variant must render a message that names
    /// the offending sysvar, and the `#[from]` conversion must preserve it.
    /// The database error of an execution failure is reachable, and it keeps
    /// its fatal classification.
    #[test]
    fn an_execution_error_exposes_its_database_error() {
        let db = DbError::fatal(AccountSourceError::new("source is down"));
        let error = ExecutionError::from(db);

        let inner = error.db_error().expect("the database error must be reachable");
        assert!(inner.is_fatal());
        assert!(
            matches!(error, ExecutionError::Database(_)),
            "expected a database failure, got {error:?}"
        );
    }

    /// An execution failure that is not a database failure has no database
    /// error behind it.
    #[test]
    fn an_execution_failure_without_a_database_error_has_none() {
        let error = ExecutionError::Halt(HaltReason::AlreadyProcessed);
        assert!(error.db_error().is_none());
    }

    /// The error source of an execution failure is the failure it wraps.
    #[test]
    fn an_execution_error_exposes_its_source() {
        let inner = AccountSourceError::new("boom");
        let error = ExecutionError::Database(DbError::non_fatal(inner.clone()));
        let source = std::error::Error::source(&error).expect("the source must be reachable");
        assert_eq!(source.to_string(), inner.to_string());
    }

    /// The error source of a transaction failure is the failure it wraps.
    #[test]
    fn a_transaction_error_exposes_its_source() {
        let inner = TxValidationError::Invalid(TransactionError::AccountNotFound);
        let error = TxError::Validation(inner.clone());
        let source = std::error::Error::source(&error).expect("the source must be reachable");
        assert_eq!(source.to_string(), inner.to_string());
    }

    /// An account-source failure keeps its own fatal classification.
    #[test]
    fn an_account_source_failure_is_fatal_only_when_the_source_is_unavailable() {
        let unavailable = ExecutionError::AccountSource {
            pubkey: Address::new_unique(),
            source: AccountSourceError::new("down"),
        };
        assert!(unavailable.is_fatal());

        let invalid = ExecutionError::AccountSource {
            pubkey: Address::new_unique(),
            source: AccountSourceError::with_kind(
                AccountSourceErrorKind::InvalidResponse,
                "bad payload",
            ),
        };
        assert!(!invalid.is_fatal());
    }

    #[test]
    fn invalid_sysvar_data_error_variants_render_and_convert() {
        let cases: [(InvalidSysvarDataError, &str); 9] = [
            (InvalidSysvarDataError::Clock, "Invalid Clock sysvar data."),
            (InvalidSysvarDataError::EpochRewards, "Invalid EpochRewards sysvar data."),
            (InvalidSysvarDataError::EpochSchedule, "Invalid EpochSchedule sysvar data."),
            (InvalidSysvarDataError::Fees, "Invalid Fees sysvar data."),
            (InvalidSysvarDataError::LastRestartSlot, "Invalid LastRestartSlot sysvar data."),
            (InvalidSysvarDataError::RecentBlockhashes, "Invalid RecentBlockhashes sysvar data."),
            (InvalidSysvarDataError::Rent, "Invalid Rent sysvar data."),
            (InvalidSysvarDataError::SlotHashes, "Invalid SlotHashes sysvar data."),
            (InvalidSysvarDataError::StakeHistory, "Invalid StakeHistory sysvar data."),
        ];

        for (error, expected) in cases {
            assert_eq!(error.to_string(), expected);
            // `HPSVMError::InvalidSysvarData` delegates rendering to the inner error.
            let wrapped = HPSVMError::from(error);
            assert!(matches!(wrapped, HPSVMError::InvalidSysvarData(_)));
            assert_eq!(wrapped.to_string(), expected);
        }
    }

    #[test]
    fn sysvar_serialization_error_renders_sysvar_and_reason() {
        let error = HPSVMError::SysvarSerialization {
            sysvar: "EpochSchedule",
            reason: "slot index out of range".to_string(),
        };
        assert_eq!(
            error.to_string(),
            "failed to serialize sysvar EpochSchedule: slot index out of range"
        );
    }

    #[test]
    fn instruction_error_conversion_preserves_payload() {
        let error = HPSVMError::from(InstructionError::InvalidArgument);
        match &error {
            HPSVMError::Instruction(InstructionError::InvalidArgument) => {}
            other => panic!("expected HPSVMError::Instruction, got {other:?}"),
        }
        assert_eq!(error.to_string(), InstructionError::InvalidArgument.to_string());
    }

    #[test]
    fn invalid_path_error_converts_from_io_error() {
        let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "missing program.so");
        let error = HPSVMError::from(io_error);
        match &error {
            HPSVMError::InvalidPath(io) => assert_eq!(io.kind(), std::io::ErrorKind::NotFound),
            other => panic!("expected HPSVMError::InvalidPath, got {other:?}"),
        }
        assert_eq!(error.to_string(), "missing program.so");
    }

    #[test]
    fn runtime_environment_error_renders_version_and_reason() {
        let error = HPSVMError::RuntimeEnvironment {
            version: "v1",
            reason: "custom syscall replay failed".to_string(),
        };
        assert_eq!(
            error.to_string(),
            "failed to refresh runtime environment v1: custom syscall replay failed"
        );
    }

    #[test]
    fn custom_syscall_registration_error_renders_name_runtime_and_reason() {
        let error = HPSVMError::CustomSyscallRegistration {
            name: "sol_alloc_free_".to_string(),
            runtime: "runtime",
            reason: "duplicate registration".to_string(),
        };
        assert_eq!(
            error.to_string(),
            "failed to register custom syscall sol_alloc_free_ in runtime: duplicate registration"
        );
    }

    #[test]
    fn invalid_loader_error_renders_program_and_loader_ids() {
        let program_id = Address::new_unique();
        let loader_id = Address::new_unique();
        let error = HPSVMError::InvalidLoader { program_id, loader_id };
        assert_eq!(
            error.to_string(),
            format!("unsupported loader {loader_id} for program {program_id}")
        );
    }

    #[test]
    fn missing_runtime_component_error_renders_component() {
        let error = HPSVMError::MissingRuntimeComponent { component: "loader_cache" };
        assert_eq!(error.to_string(), "missing runtime component: loader_cache");
    }

    #[test]
    fn account_source_error_renders_pubkey_and_underlying_source() {
        let pubkey = Address::new_unique();
        let source = AccountSourceError::new("rpc endpoint unreachable");
        let error = HPSVMError::AccountSource { pubkey, source };

        assert_eq!(
            error.to_string(),
            format!(
                "account source failed while loading {pubkey}: Unavailable: rpc endpoint unreachable"
            )
        );
    }

    #[test]
    fn program_load_error_renders_message() {
        let error = HPSVMError::ProgramLoad("ELF header is truncated".to_string());
        assert_eq!(error.to_string(), "failed to load program: ELF header is truncated");
    }

    #[test]
    fn boxed_error_converts_to_program_load() {
        let boxed: Box<dyn std::error::Error> = Box::new(std::io::Error::other("bad magic"));
        let error = HPSVMError::from(boxed);
        match &error {
            HPSVMError::ProgramLoad(message) => assert_eq!(message, "bad magic"),
            other => panic!("expected HPSVMError::ProgramLoad, got {other:?}"),
        }
    }

    #[test]
    fn account_source_error_is_classifiable_through_hpsvm_error() {
        // `HPSVMError::AccountSource` must not erase the source error's kind.
        let source = AccountSourceError::with_kind(
            AccountSourceErrorKind::InvalidResponse,
            "malformed json",
        );
        let error = HPSVMError::AccountSource { pubkey: Address::new_unique(), source };

        match error {
            HPSVMError::AccountSource { source, .. } => {
                assert_eq!(source.kind(), AccountSourceErrorKind::InvalidResponse);
            }
            other => panic!("expected HPSVMError::AccountSource, got {other:?}"),
        }
    }

    #[test]
    fn unknown_transaction_tag_error_renders_tag() {
        use crate::tx_registry::TxTag;
        let error = HPSVMError::UnknownTransactionTag { tag: TxTag::Custom(9) };
        assert_eq!(error.to_string(), "no transaction handler registered for tag Custom(9)");
    }

    #[test]
    fn registry_unknown_tag_converts_into_hpsvm_error() {
        use crate::tx_registry::TxTag;
        let error = HPSVMError::from(TxError::UnknownTag(TxTag::V0));
        match &error {
            HPSVMError::UnknownTransactionTag { tag } => assert_eq!(*tag, TxTag::V0),
            other => panic!("expected HPSVMError::UnknownTransactionTag, got {other:?}"),
        }
    }

    #[test]
    fn an_execution_failure_keeps_its_concrete_cause_across_the_outer_error() {
        let error = HPSVMError::from(ExecutionError::Fatal(Box::new(AnyError::new(
            HPSVMError::ProgramLoad("boom".to_string()),
        ))));
        match &error {
            HPSVMError::Execution(ExecutionError::Fatal(erased)) => {
                let recovered = erased
                    .downcast_ref::<HPSVMError>()
                    .expect("the concrete cause must stay recoverable by downcast");
                match recovered {
                    HPSVMError::ProgramLoad(message) => assert_eq!(message, "boom"),
                    other => panic!("expected HPSVMError::ProgramLoad, got {other:?}"),
                }
            }
            other => panic!("expected HPSVMError::Execution(Fatal), got {other:?}"),
        }
        assert!(matches!(&error, HPSVMError::Execution(error) if error.is_fatal()));
    }

    #[test]
    fn an_account_source_failure_keeps_its_outer_variant() {
        let source = AccountSourceError::new("rpc down");
        let error = HPSVMError::from(ExecutionError::AccountSource {
            pubkey: Address::new_unique(),
            source,
        });
        assert!(
            matches!(error, HPSVMError::AccountSource { .. }),
            "expected HPSVMError::AccountSource, got {error:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The layered error taxonomy.
//
// These tests name the taxonomy types before they exist, so this module fails
// to compile until the taxonomy is added below the `HPSVMError` section.
// Every assertion matches an exact variant (never `is_err`) and every
// classification travels in types, never in strings.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod task71_taxonomy_tests {
    use solana_transaction_error::TransactionError;

    use super::*;
    use crate::account_source::{AccountSourceError, AccountSourceErrorKind};

    #[test]
    fn any_error_downcast_round_trip_recovers_the_concrete_type() {
        let original =
            AccountSourceError::with_kind(AccountSourceErrorKind::InvalidResponse, "bad body");
        let erased = AnyError::new(original.clone());

        assert!(erased.is::<AccountSourceError>());
        assert!(!erased.is::<std::io::Error>());
        let recovered = erased
            .downcast_ref::<AccountSourceError>()
            .expect("AnyError must recover the concrete source error");
        assert_eq!(recovered, &original);
        assert_eq!(recovered.kind(), AccountSourceErrorKind::InvalidResponse);
        assert!(erased.downcast_ref::<std::io::Error>().is_none());
    }

    #[test]
    fn any_error_survives_a_boxed_erased_boundary() {
        let erased = AnyError::new(AccountSourceError::new("rpc down"));
        let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(erased);
        let outer = boxed
            .downcast_ref::<AnyError>()
            .expect("AnyError must survive a boxed erased boundary");
        let recovered = outer
            .downcast_ref::<AccountSourceError>()
            .expect("concrete error must be recoverable after double erasure");
        assert_eq!(recovered.kind(), AccountSourceErrorKind::Unavailable);
        assert_eq!(recovered.message(), "rpc down");
    }

    #[test]
    fn db_error_from_io_error_is_non_fatal_and_recoverable() {
        let db_error =
            DbError::from(std::io::Error::new(std::io::ErrorKind::NotFound, "missing program.so"));
        assert!(!db_error.is_fatal());
        let io =
            db_error.downcast_ref::<std::io::Error>().expect("DbError must recover the io error");
        assert_eq!(io.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn db_error_fatal_matrix_over_account_source_kinds() {
        for (kind, fatal) in [
            (AccountSourceErrorKind::Unavailable, true),
            (AccountSourceErrorKind::InvalidResponse, false),
            (AccountSourceErrorKind::Other, false),
        ] {
            let db_error = DbError::from(AccountSourceError::with_kind(kind, "boom"));
            assert_eq!(db_error.is_fatal(), fatal, "kind {kind:?} must map to fatal={fatal}");
            // The mapping must survive an erased boundary: erase into
            // `AnyError`, recover the `DbError`, and the flag is intact.
            let erased = AnyError::new(db_error);
            let recovered = erased
                .downcast_ref::<DbError>()
                .expect("DbError must be recoverable from AnyError");
            assert_eq!(recovered.is_fatal(), fatal, "kind {kind:?} must stay fatal={fatal}");
            let source = recovered
                .downcast_ref::<AccountSourceError>()
                .expect("AccountSourceError must be recoverable through DbError");
            assert_eq!(source.kind(), kind);
        }
    }

    #[test]
    fn db_result_alias_carries_db_error() {
        let ok: DbResult<u32> = Ok(7);
        match ok {
            Ok(value) => assert_eq!(value, 7),
            Err(error) => panic!("expected DbResult::Ok, got {error:?}"),
        }
        let err: DbResult<u32> = Err(DbError::non_fatal(AccountSourceError::with_kind(
            AccountSourceErrorKind::Other,
            "x",
        )));
        match err {
            Err(error) => assert!(!error.is_fatal()),
            Ok(_) => panic!("expected DbResult::Err, got Ok"),
        }
    }

    #[test]
    fn load_error_variants_carry_their_db_error_and_fatal_flag() {
        let fatal_db = DbError::from(AccountSourceError::new("rpc down"));
        let non_fatal_db =
            DbError::from(AccountSourceError::with_kind(AccountSourceErrorKind::Other, "custom"));

        let account = LoadError::account(fatal_db);
        let program = LoadError::program(non_fatal_db.clone());
        let sysvar = LoadError::sysvar(non_fatal_db);

        match &account {
            LoadError::Account(inner) => assert!(inner.is_fatal()),
            other => panic!("expected LoadError::Account, got {other:?}"),
        }
        match &program {
            LoadError::Program(inner) => assert!(!inner.is_fatal()),
            other => panic!("expected LoadError::Program, got {other:?}"),
        }
        match &sysvar {
            LoadError::Sysvar(inner) => assert!(!inner.is_fatal()),
            other => panic!("expected LoadError::Sysvar, got {other:?}"),
        }
        assert!(account.db_error().is_fatal());
        assert!(!program.db_error().is_fatal());
        assert_eq!(account.to_string(), "failed to load account: Unavailable: rpc down");
        assert_eq!(program.to_string(), "failed to load program: Other: custom");
        assert_eq!(sysvar.to_string(), "failed to load sysvar: Other: custom");
    }

    #[test]
    fn tx_validation_error_wraps_the_transaction_error() {
        let validation = TxValidationError::from(TransactionError::BlockhashNotFound);
        match &validation {
            TxValidationError::Invalid(TransactionError::BlockhashNotFound) => {}
            other => panic!("expected TxValidationError::Invalid, got {other:?}"),
        }
        assert_eq!(validation.to_string(), "transaction validation failed: Blockhash not found");
    }

    #[test]
    fn tx_error_variants_and_from_conversions_cover_every_layer() {
        let validation =
            TxError::from(TxValidationError::Invalid(TransactionError::BlockhashNotFound));
        match &validation {
            TxError::Validation(TxValidationError::Invalid(
                TransactionError::BlockhashNotFound,
            )) => {}
            other => panic!("expected TxError::Validation, got {other:?}"),
        }
        assert!(!validation.is_fatal());

        let execution = TxError::from(ExecutionError::from(HaltReason::AlreadyProcessed));
        match &execution {
            TxError::Execution(ExecutionError::Halt(HaltReason::AlreadyProcessed)) => {}
            other => panic!("expected TxError::Execution(Halt), got {other:?}"),
        }
        assert!(execution.is_fatal());

        let fatal_db = DbError::from(AccountSourceError::new("rpc down"));
        let database = TxError::from(fatal_db);
        match &database {
            TxError::Database(inner) => assert!(inner.is_fatal()),
            other => panic!("expected TxError::Database, got {other:?}"),
        }
        assert!(database.is_fatal());
    }

    #[test]
    fn execution_error_variants_distinguish_database_fatal_and_halt() {
        let non_fatal_db =
            DbError::from(AccountSourceError::with_kind(AccountSourceErrorKind::Other, "custom"));
        let database = ExecutionError::from(non_fatal_db);
        match &database {
            ExecutionError::Database(inner) => assert!(!inner.is_fatal()),
            other => panic!("expected ExecutionError::Database, got {other:?}"),
        }
        assert!(!database.is_fatal());

        let fatal =
            ExecutionError::Fatal(Box::new(AnyError::new(AccountSourceError::new("rpc down"))));
        match &fatal {
            ExecutionError::Fatal(_) => {}
            other => panic!("expected ExecutionError::Fatal, got {other:?}"),
        }
        assert!(fatal.is_fatal());
        match fatal.db_error() {
            // A `Fatal` erasure carries no `DbError` classification.
            None => {}
            Some(_) => panic!("expected no DbError inside ExecutionError::Fatal"),
        }

        let halt = ExecutionError::from(HaltReason::Transaction(TransactionError::AccountInUse));
        match &halt {
            ExecutionError::Halt(HaltReason::Transaction(TransactionError::AccountInUse)) => {}
            other => panic!("expected ExecutionError::Halt, got {other:?}"),
        }
        assert!(halt.is_fatal());
    }

    #[test]
    fn halt_reason_covers_real_halts_without_stringly_typed_reasons() {
        let already = HaltReason::AlreadyProcessed;
        assert_eq!(already.transaction_error(), TransactionError::AlreadyProcessed);
        assert_eq!(already.to_string(), "transaction already processed");

        let wrapped = HaltReason::Transaction(TransactionError::InsufficientFundsForFee);
        assert_eq!(wrapped.transaction_error(), TransactionError::InsufficientFundsForFee);
        match &wrapped {
            HaltReason::Transaction(TransactionError::InsufficientFundsForFee) => {}
            other => panic!("expected HaltReason::Transaction, got {other:?}"),
        }
    }

    #[test]
    fn host_error_variants_and_from_conversions() {
        let halt = HostError::from(HaltReason::AlreadyProcessed);
        match &halt {
            HostError::Halt(HaltReason::AlreadyProcessed) => {}
            other => panic!("expected HostError::Halt, got {other:?}"),
        }
        assert!(halt.is_fatal());

        let execution = HostError::from(ExecutionError::from(DbError::from(
            AccountSourceError::with_kind(AccountSourceErrorKind::Other, "custom"),
        )));
        match &execution {
            HostError::Execution(ExecutionError::Database(_)) => {}
            other => panic!("expected HostError::Execution(Database), got {other:?}"),
        }
        assert!(!execution.is_fatal());
    }

    #[test]
    fn handler_error_round_trip_recovers_the_hpsvm_error() {
        let handler = HandlerError::from(HPSVMError::ProgramLoad("boom".to_string()));
        let recovered =
            handler.downcast_ref::<HPSVMError>().expect("HandlerError must recover the HPSVMError");
        match recovered {
            HPSVMError::ProgramLoad(message) => assert_eq!(message, "boom"),
            other => panic!("expected HPSVMError::ProgramLoad, got {other:?}"),
        }
        assert_eq!(handler.to_string(), "failed to load program: boom");
    }

    #[test]
    fn hpsvm_error_outer_variants_preserve_every_layer_losslessly() {
        let db_error = DbError::from(AccountSourceError::new("rpc down"));
        let error = HPSVMError::from(db_error);
        match &error {
            HPSVMError::Database(inner) => {
                assert!(inner.is_fatal());
                let source = inner
                    .downcast_ref::<AccountSourceError>()
                    .expect("AccountSourceError must survive the HPSVMError boundary");
                assert_eq!(source.kind(), AccountSourceErrorKind::Unavailable);
            }
            other => panic!("expected HPSVMError::Database, got {other:?}"),
        }

        let error = HPSVMError::from(LoadError::account(DbError::from(AccountSourceError::new(
            "rpc down",
        ))));
        assert!(
            matches!(error, HPSVMError::Load(LoadError::Account(_))),
            "expected HPSVMError::Load(Account), got {error:?}"
        );

        let error = HPSVMError::from(TxError::from(TxValidationError::Invalid(
            TransactionError::BlockhashNotFound,
        )));
        assert!(
            matches!(error, HPSVMError::Transaction(TxError::Validation(_))),
            "expected HPSVMError::Transaction(Validation), got {error:?}"
        );

        let error = HPSVMError::from(ExecutionError::from(HaltReason::AlreadyProcessed));
        assert!(
            matches!(error, HPSVMError::Execution(ExecutionError::Halt(_))),
            "expected HPSVMError::Execution(Halt), got {error:?}"
        );

        let error = HPSVMError::from(HostError::from(HaltReason::AlreadyProcessed));
        assert!(
            matches!(error, HPSVMError::Host(HostError::Halt(_))),
            "expected HPSVMError::Host(Halt), got {error:?}"
        );

        let error =
            HPSVMError::from(HandlerError::from(HPSVMError::ProgramLoad("boom".to_string())));
        assert!(
            matches!(error, HPSVMError::Handler(_)),
            "expected HPSVMError::Handler, got {error:?}"
        );
    }

    #[test]
    fn fatal_at_db_is_still_fatal_after_host_crossing() {
        // A fatal database error (infrastructure down) must remain
        // distinguishable as fatal after crossing the erased `AnyError`
        // boundary and the public `HPSVMError` boundary.
        let db_error = DbError::from(AccountSourceError::new("rpc down"));
        assert!(db_error.is_fatal());

        let erased = AnyError::new(db_error);
        let recovered =
            erased.downcast_ref::<DbError>().expect("DbError must be recoverable from AnyError");
        assert!(recovered.is_fatal());

        let host = HPSVMError::from(HostError::from(ExecutionError::Database(
            erased
                .downcast_ref::<DbError>()
                .expect("DbError must be recoverable a second time")
                .clone(),
        )));
        match &host {
            HPSVMError::Host(HostError::Execution(ExecutionError::Database(inner))) => {
                assert!(inner.is_fatal(), "fatal-at-DB must still be fatal after host");
            }
            other => panic!("expected HPSVMError::Host(Execution(Database)), got {other:?}"),
        }
    }

    #[test]
    fn taxonomy_types_are_send_sync_and_static_for_handler_erasure() {
        fn assert_send_sync_static<T: Send + Sync + 'static>() {}
        assert_send_sync_static::<AnyError>();
        assert_send_sync_static::<DbError>();
        assert_send_sync_static::<LoadError>();
        assert_send_sync_static::<TxValidationError>();
        assert_send_sync_static::<HaltReason>();
        assert_send_sync_static::<ExecutionError>();
        assert_send_sync_static::<TxError>();
        assert_send_sync_static::<HostError>();
        assert_send_sync_static::<HandlerError>();
        assert_send_sync_static::<HPSVMError>();
    }
}
