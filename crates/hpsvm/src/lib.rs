//! Fast, lightweight, in-process Solana VM for testing programs.
//!
//! `hpsvm` executes Solana transactions in-process against a single mutable
//! environment: fund accounts, stage transactions, then keep or roll back
//! their post-state. [`Hpsvm`] is the ready-made instantiation (the base type
//! family); [`Svm`] is the generic host behind it, parameterized by a
//! [`SvmTypesHost`] family.
//!
//! # Minimal example
//!
//! ```rust
//! use hpsvm::Hpsvm;
//! use solana_address::Address;
//! use solana_keypair::Keypair;
//! use solana_message::Message;
//! use solana_signer::Signer;
//! use solana_system_interface::instruction::transfer;
//! use solana_transaction::Transaction;
//!
//! let from_keypair = Keypair::new();
//! let from = from_keypair.pubkey();
//! let to = Address::new_unique();
//!
//! let mut svm = Hpsvm::new();
//! svm.airdrop(&from, 1_000_000_000).unwrap();
//! svm.airdrop(&to, 1_000_000_000).unwrap();
//!
//! let instruction = transfer(&from, &to, 64);
//! let tx = Transaction::new(
//!     &[&from_keypair],
//!     Message::new(&[instruction], Some(&from)),
//!     svm.latest_blockhash(),
//! );
//! svm.transact(tx).expect("transact should stage").commit().unwrap();
//!
//! assert_eq!(svm.get_balance(&from).unwrap(), 999_994_936);
//! assert_eq!(svm.get_balance(&to).unwrap(), 1_000_000_064);
//!
//! // A second transfer can be staged and then rolled back.
//! let retry = transfer(&from, &to, 100);
//! let tx2 = Transaction::new(
//!     &[&from_keypair],
//!     Message::new(&[retry], Some(&from)),
//!     svm.latest_blockhash(),
//! );
//! svm.transact(tx2).expect("transact should stage").discard().unwrap();
//! assert_eq!(svm.get_balance(&from).unwrap(), 999_994_936);
//! assert_eq!(svm.get_balance(&to).unwrap(), 1_000_000_064);
//! ```
//!
//! # Architecture
//!
//! ## Type family
//!
//! [`Svm`] is generic over a [`SvmTypesHost`] family selecting the spec id,
//! transaction envelope, host, and extension slots. [`BaseSvmTypes`] is the
//! stock family and [`Hpsvm`] names `Svm<'static, BaseSvmTypes>`. A custom
//! family implements [`SvmTypesHost`] (plus [`SvmTypes`] for the defaulted
//! slots) and runs anywhere the base family does.
//!
//! The VM's own state is grouped the same way: [`Svm`] holds its mutable
//! state (the account overlay and the transaction history), its protocol
//! configuration ([`Version`]), its block environment, the facilities a fork
//! can replace (the inspector, the transaction registry, the precompile set,
//! the compiled-tier promoter, the runtime-environment registrations and the
//! reserved account keys) and the record of which runtime components are
//! materialized.
//!
//! ## Execution protocol
//!
//! [`Version`] is the execution-shaping protocol configuration: feature set,
//! signature verification, blockhash checks, compute budget, rent, limits, and
//! the [`ClusterSpecId`] profile. Inspect it with [`Svm::version`]; change it
//! with the `set_*` reconfigurers ([`Svm::set_feature_set`],
//! [`Svm::set_sigverify`], [`Svm::set_compute_budget`]). [`Host`] is the
//! execution-facing view of the VM that every handler receives.
//!
//! ## Staged execution
//!
//! [`Svm::transact`] executes and stages the post-state in the live overlay,
//! returning an [`ExecutedTx`] that borrows the VM. [`ExecutedTx::commit`]
//! keeps the staged state and records history, [`ExecutedTx::discard`] rolls
//! it back, and [`ExecutedTx::commit_with`] streams each account change into
//! a [`StateChangeSink`] first. [`Svm::try_transact`] and
//! [`Svm::transact_detached`] return the owned readonly [`DetachedTx`]
//! without staging; [`Svm::send_transaction`] is `transact` plus `commit` in
//! one call.
//!
//! ## Transaction registry
//!
//! Every entry point dispatches through a [`TxRegistry`] keyed by [`TxTag`].
//! [`Svm::register_tx_handler`] replaces the [`TxHandler`] for one tag, which
//! is how tests and forks intercept execution; the [`handler`] helper builds
//! a handler from a validate/execute pair.
//!
//! ## Precompiles
//!
//! A [`PrecompileProvider`] answers which program ids run as precompiles.
//! [`PrecompileSet`] is the map-backed provider, [`NoPrecompiles`] disables
//! dispatch, and [`Svm::set_precompile_provider`] swaps the active provider
//! (plus [`Precompile`] for a single entry).
//!
//! ## Inspectors
//!
//! An [`Inspector`] observes execution through `&mut self` hooks. Install one
//! with [`Svm::set_inspector`] (or [`Svm::with_inspector`]) and recover it
//! with [`Svm::clear_inspector`]. [`OriginFiltered`] scopes hooks by
//! [`TransactionOrigin`] via [`OriginFilter`]; [`InspectMode`] is the
//! const-generic switch that compiles hook dispatch away when no inspector is
//! live.
//!
//! ## Declaring instructions
//!
//! `hpsvm_macros::svm_instruction` declares a builtin instruction from a
//! function, threaded through [`SvmInstructionCtx`] with declared
//! [`SvmInstructionAccount`]s (see `examples/custom_svm.rs` below).
//!
//! ## Errors
//!
//! Setup and fatal staging failures surface as
//! [`HPSVMError`]; per-transaction outcomes are
//! `TransactionResult`s carrying
//! [`TransactionMetadata`](crate::types::TransactionMetadata) or
//! [`FailedTransactionMetadata`](crate::types::FailedTransactionMetadata),
//! with [`ExecutionDiagnostics`](crate::types::ExecutionDiagnostics) for
//! pre/post diffs.
//!
//! The error taxonomy is layered, and every layer is live: a handler
//! rejects a transaction with [`TxValidationError`] and reports an
//! execution failure with [`ExecutionError`]; the registry reports a
//! dispatch failure as [`TxError`]; the host boundary reports through
//! [`HostError`]. [`HPSVMError`] is the crate's outer projection of those
//! layers, and it stays lossless: the fatal classification travels in the
//! type, never in a message string.
//!
//! ## State model
//!
//! Staging journals overlay mutations behind an internal checkpoint (rolled
//! back by `discard`); caches and history derive from the overlay. The
//! checkpoint type and the account database are crate-internal; the public
//! state views are [`AccountsView`] and [`StateChangeSet`].
//!
//! # Deploying programs
//!
//! Beyond transfers, add compiled programs with [`Hpsvm::add_program`] (or
//! [`Hpsvm::add_program_from_file`]) and exercise them through staged or
//! direct transactions. Pull released programs with `solana program dump`.
//!
//! ```rust
//! use hpsvm::Hpsvm;
//! use solana_address::{Address, address};
//! use solana_instruction::{Instruction, account_meta::AccountMeta};
//! use solana_keypair::Keypair;
//! use solana_message::{Message, VersionedMessage};
//! use solana_signer::Signer;
//! use solana_transaction::versioned::VersionedTransaction;
//!
//! fn test_logging() {
//!     let program_id = address!("Logging111111111111111111111111111111111111");
//!     let account_meta =
//!         AccountMeta { pubkey: Address::new_unique(), is_signer: false, is_writable: true };
//!     let ix = Instruction {
//!         program_id,
//!         accounts: vec![account_meta],
//!         data: vec![5, 10, 11, 12, 13, 14],
//!     };
//!     let mut svm = Hpsvm::new();
//!     let payer = Keypair::new();
//!     // The `counter` test program is a separate SBF build; skip when absent.
//!     let Some(bytes) = hpsvm_test_support::read_program(hpsvm_test_support::COUNTER) else {
//!         return;
//!     };
//!     svm.add_program(program_id, &bytes[..]);
//!     svm.airdrop(&payer.pubkey(), 1_000_000_000).unwrap();
//!     let blockhash = svm.latest_blockhash();
//!     let msg = Message::new_with_blockhash(&[ix], Some(&payer.pubkey()), &blockhash);
//!     let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[&payer]).unwrap();
//!     // Let's simulate it first
//!     let sim_res = svm.simulate_transaction(tx.clone()).unwrap();
//!     let meta = svm.send_transaction(tx).unwrap();
//!     assert_eq!(sim_res.meta, meta);
//!     // The program should log something
//!     assert!(meta.logs.len() > 1);
//!     assert!(meta.compute_units_consumed < 10_000); // not being precise here in case it changes
//! }
//! ```
//!
//! # Compiled-program tiering
//!
//! Program compilation is tiered, cached and observable — the harness around
//! agave's `solana-sbpf` JIT, not a fork of it:
//!
//! * [`JitPolicy`] on [`Version`] selects the tier (`Eager` compiles every program at load time on
//!   x86_64; `Lazy` interprets until a program crosses its hot threshold, then promotes it on a
//!   background pool; `Off` forces the interpreter even on x86_64). The default is `Eager`,
//!   byte-identical to the pre-tiering implementation.
//! * The resident entry store (content-keyed by environment identity, ELF hash, loader and slot
//!   bucket) re-links already-compiled entries across VMs and across `rebuild_program_cache`
//!   instead of recompiling them.
//! * [`Svm::prewarm_programs`] front-loads a batch stage's program set before its first transaction
//!   (and schedules promotion under `Lazy`).
//! * [`Svm::program_tier_stats`] reports policy, resident/cache entries, agave compilations and
//!   invocation splits, plus promotion outcomes — all from atomics, with `Off` as the effective
//!   policy off x86_64.
//!
//! ```rust
//! use hpsvm::{Hpsvm, JitPolicy};
//!
//! let mut svm = Hpsvm::new().with_jit_policy(JitPolicy::Off);
//! assert_eq!(svm.version().jit_policy, JitPolicy::Off);
//! svm.prewarm_programs(&[]).expect("empty prewarm is a no-op");
//! let stats = svm.program_tier_stats();
//! // `Off` is effective on every target, so this doctest holds on x86_64
//! // and elsewhere alike.
//! assert_eq!(stats.effective_policy, JitPolicy::Off);
//! assert_eq!(stats.promotions_attempted, 0);
//! ```
//!
//! # Time travel
//!
//! Programs reading the `Clock` sysvar can be tested across time with
//! [`Hpsvm::set_sysvar`] / [`Hpsvm::get_sysvar`], or jumped across slots with
//! [`Hpsvm::warp_to_slot`]. Here's an example using a program that panics if
//! `clock.unix_timestamp` is greater than 100 (January 1st 1970):
//!
//! ```rust
//! use hpsvm::Hpsvm;
//! use solana_address::Address;
//! use solana_clock::Clock;
//! use solana_instruction::Instruction;
//! use solana_keypair::Keypair;
//! use solana_message::{Message, VersionedMessage};
//! use solana_signer::Signer;
//! use solana_transaction::versioned::VersionedTransaction;
//!
//! fn test_set_clock() {
//!     let program_id = Address::new_unique();
//!     let mut svm = Hpsvm::new();
//!     // The clock example is a separate SBF build; skip when absent.
//!     let Some(bytes) = hpsvm_test_support::read_program(hpsvm_test_support::CLOCK_EXAMPLE)
//!     else {
//!         return;
//!     };
//!     svm.add_program(program_id, &bytes[..]);
//!     let payer = Keypair::new();
//!     let payer_address = payer.pubkey();
//!     svm.airdrop(&payer.pubkey(), 1_000_000_000).unwrap();
//!     let blockhash = svm.latest_blockhash();
//!     let ixs = [Instruction { program_id, data: vec![], accounts: vec![] }];
//!     let msg = Message::new_with_blockhash(&ixs, Some(&payer_address), &blockhash);
//!     let versioned_msg = VersionedMessage::Legacy(msg);
//!     let tx = VersionedTransaction::try_new(versioned_msg, &[&payer]).unwrap();
//!     // Set the time to January 1st 2000
//!     let mut initial_clock = svm.get_sysvar::<Clock>();
//!     initial_clock.unix_timestamp = 1735689600;
//!     svm.set_sysvar::<Clock>(&initial_clock).expect("clock sysvar override should succeed");
//!     // This will fail because the program expects early 1970 timestamp
//!     let _err = svm.send_transaction(tx.clone()).unwrap_err();
//!     // So let's turn back time
//!     let mut clock = svm.get_sysvar::<Clock>();
//!     clock.unix_timestamp = 50;
//!     svm.set_sysvar::<Clock>(&clock).expect("clock sysvar override should succeed");
//!     let ixs2 = [Instruction {
//!         program_id,
//!         data: vec![1], // unused, this is just to dedup the transaction
//!         accounts: vec![],
//!     }];
//!     let msg2 = Message::new_with_blockhash(&ixs2, Some(&payer_address), &blockhash);
//!     let versioned_msg2 = VersionedMessage::Legacy(msg2);
//!     let tx2 = VersionedTransaction::try_new(versioned_msg2, &[&payer]).unwrap();
//!     // Now the transaction goes through
//!     svm.send_transaction(tx2).unwrap();
//! }
//! ```
//!
//! # Custom environments
//!
//! `examples/custom_svm.rs` stands up a custom [`SvmTypesHost`] family end to
//! end: custom spec id, block-environment extension, transaction handler,
//! macro-declared instruction, and custom precompile, each asserted observable
//! through `commit` / `commit_with` flows. Start there when extending the VM.
//!
//! # Migrating from the pre-refactor API
//!
//! The refactor replaces the previous surface outright:
//!
//! - `HPSVM` is now [`Hpsvm`], the `Svm<'static, BaseSvmTypes>` alias; new code uses generic
//!   [`Svm`] with a [`SvmTypesHost`] family.
//! - `SvmCfg` / `RuntimeEnv` are now [`Version`] (see [`Svm::version`]).
//! - `ExecutionOutcome` / `commit_transaction` are now [`ExecutedTx`]: [`Svm::transact`] stages,
//!   [`ExecutedTx::commit`] keeps, [`ExecutedTx::discard`] rolls back, and
//!   [`ExecutedTx::commit_with`] streams changes into a [`StateChangeSink`].
//! - `Arc<dyn Inspector>` is now `Option<Box<..>>` plus [`Svm::set_inspector`] (recover with
//!   [`Svm::clear_inspector`]); scope origins with [`OriginFiltered`].
//! - The boolean precompile switch is now a [`PrecompileProvider`] (see
//!   [`Svm::set_precompile_provider`]).

#![cfg_attr(docsrs, feature(doc_cfg))]
// `InspectMode` is threaded through execution as a const-generic
// parameter. A fieldless enum as a const-param type needs minimal ADT const
// params; the workspace already pins a nightly toolchain (rust-toolchain.toml).
#![feature(min_adt_const_params)]

use std::{marker::PhantomData, sync::Arc};

use agave_reserved_account_keys::ReservedAccountKeys;
use parking_lot::Mutex;
use solana_program_runtime::invoke_context::BuiltinFunctionRegisterer;

pub(crate) use crate::helpers::*;
use crate::{
    accounts_db::Accounts, history::TransactionHistory, runtime_registry::RuntimeExtensionRegistry,
};
macro_rules! hotpath_block {
    ($label:literal, $expr:expr) => {{
        #[cfg(feature = "hotpath")]
        {
            hotpath::measure_block!($label, $expr)
        }
        #[cfg(not(feature = "hotpath"))]
        {
            $expr
        }
    }};
}

pub(crate) use hotpath_block;

/// A registered custom syscall paired with its loader function.
#[derive(Clone)]
pub(crate) struct CustomSyscallRegistration {
    name: String,
    function: BuiltinFunctionRegisterer,
}

/// Transaction batch planning and parallel execution.
pub mod batch;
/// Error types for the SVM.
pub mod error;
/// Staged (`ExecutedTx`) and detached (`DetachedTx`) transaction execution.
pub mod executed;
/// RPC-backed account source for forking live cluster state.
#[cfg(feature = "fork")]
pub mod fork;
/// Solana instruction types and helpers.
pub mod instruction;
/// Loader v3 helpers (set_upgrade_authority, deploy_upgradeable_program).
#[cfg(feature = "loader")]
pub mod loader;
pub mod state_change_sink;
/// SPL token operation helpers (transfer, mint, ATA, freeze, etc.).
#[cfg(feature = "token")]
pub mod token;
/// Public return types and execution metadata.
pub mod types;

// Bridges the workspace's newer solana interface types to the older copies the
// agave 4.5 runtime crates are compiled against (see `agave_compat`).
mod account_source;
mod accounts_db;
mod agave_compat;
mod builder;
mod callback;
mod commit;
mod env;
mod execution;
mod format_logs;
mod helpers;
mod history;
mod host;
mod inspector;
mod message_processor;
mod precompile_registry;
#[cfg(feature = "precompiles")]
mod precompiles;
mod programs;
#[cfg(feature = "register-tracing")]
pub mod register_tracing;
mod runtime_env;
mod runtime_registry;
mod state_change_set;
mod svm_instruction;
mod svm_types;
mod tx_registry;
mod utils;
/// Execution-shaping protocol configuration.
mod version;
/// Core `Svm` construction, account/sysvar state, and transaction API.
mod vm;

/// Compiles and runs every ```rust``` block in the repository README as a
/// doctest, so the published quick-start examples cannot drift from the API.
#[cfg(doctest)]
#[doc = include_str!("../../../README.md")]
struct ReadmeExamples;

pub use account_source::{AccountSource, AccountSourceError};
pub use accounts_db::AccountsView;
pub use builder::{FeatureConfigOpen, FeatureConfigSealed, HpsvmBuilder};
pub use env::{BlockEnv, TxEnv};
pub use error::{
    AnyError, DbError, DbResult, ExecutionError, HPSVMError, HaltReason, HandlerError, HostError,
    LoadError, TxError, TxValidationError,
};
pub use executed::{DetachedTx, ExecutedTx};
pub use host::Host;
pub use inspector::{InspectMode, Inspector, OriginFilter, OriginFiltered, TransactionOrigin};
#[cfg(feature = "precompiles")]
pub use precompile_registry::NativePrecompiles;
pub use precompile_registry::{NoPrecompiles, Precompile, PrecompileProvider, PrecompileSet};
pub use programs::{JitPolicy, ProgramTierStats};
pub use state_change_set::StateChangeSet;
pub use state_change_sink::{AccountChange, AccountChangeRecord, StateChangeSink};
pub use svm_instruction::{SvmInstructionAccount, SvmInstructionCtx};
pub use svm_types::{BaseSvmTypes, SvmTypes, SvmTypesHost};
pub use tx_registry::{DefaultExecutionHandler, TxHandler, TxRegistry, TxTag, handler, tag_of};
pub use version::{
    ClusterInfo, ClusterSpecId, FeatureGate, RuntimeLimits, TxFeatureFlags, Version,
};

// Re-export crate-internal items at the root so child modules can reference
// them as `crate::X` (descendant modules can read root-private items, but the
// names must resolve via the crate root).
pub(crate) use crate::commit::apply_commit_delta;

/// The core Solana Virtual Machine simulator, parameterized by its type family.
///
/// The single `T` parameter selects every swappable axis of the VM through
/// [`SvmTypesHost`]: the spec identifier, the transaction envelope, the six
/// extension slots and the host itself. Instantiate it with [`BaseSvmTypes`] to
/// get the shape the rest of this workspace uses, or with a custom family to
/// extend a slot without touching the core.
///
/// `'a` is the host lifetime. It is carried by the phantom marker below and
/// bounds no real borrow, so a VM value has no lifetime obligations of its own.
///
/// Every inherent method is written once against [`SvmTypesHost`] and
/// instantiated per family, so a fork that leaves a slot at `()` pays nothing
/// for it.
///
/// # Compatibility alias
///
/// The all-caps spelling is an alias for the instantiation every pre-refactor caller used:
/// `Svm<'static, BaseSvmTypes>`. It exists so the existing public-behaviour
/// surface keeps compiling unchanged while call sites migrate.
pub struct Svm<'a, T: SvmTypesHost> {
    /// The VM's mutable state.
    state: SvmState,
    /// The execution-shaping protocol configuration.
    version: Version,
    /// The current block environment.
    block_env: BlockEnv<T::BlockEnvExt>,
    /// The facilities a fork can replace.
    extensions: SvmExtensions<'a, T>,
    /// Which runtime components are materialized.
    materialized: MaterializedState,
    /// The keypair the internal airdrop transfer is funded from.
    airdrop_kp: [u8; 64],
    /// Binds the host lifetime and the type family to the value without letting
    /// either affect the VM's auto traits.
    ///
    /// A `fn() -> &'a T` pointer is `Send + Sync` whatever `T` is, so the VM's
    /// existing `Send + Sync` behaviour does not depend on the auto traits of
    /// the family it is instantiated with. Storing `T` or `&'a T` directly
    /// would make the VM `Send`/`Sync` exactly when the family is, which would
    /// silently change what a `Svm<'_, T>` can do inside a `rayon` batch stage.
    /// It is covariant in both parameters, which keeps the compatibility alias
    /// below interchangeable with the generic host.
    _type_family: PhantomData<fn() -> &'a T>,
}

/// The VM's mutable state: the account overlay and the transaction history.
///
/// The two belong together: the history records what the overlay committed,
/// and both are rolled back by a staged transaction's `discard`.
struct SvmState {
    /// The account overlay and the caches derived from it.
    accounts: Accounts,
    /// The bounded transaction history.
    history: TransactionHistory,
}

/// Which runtime components are materialized into the VM.
///
/// The components are materialized once and then reused, so the VM tracks
/// them explicitly: a feature-set reconfiguration has to know what to tear
/// down and rebuild.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct MaterializedState {
    /// The built-in programs are loaded.
    builtins: bool,
    /// The default programs are loaded.
    default_programs: bool,
    /// The SPL programs are loaded.
    spl_programs: bool,
    /// The feature-gate accounts are materialized.
    feature_accounts: bool,
    /// The sysvars are materialized.
    sysvars: bool,
}

/// The facilities a fork can replace.
///
/// Grouping them names the type family's extension points in one place: the
/// inspector, the transaction registry, the precompile set, the compiled-tier
/// promoter, the runtime-environment registrations and the reserved account
/// keys.
struct SvmExtensions<'a, T: SvmTypesHost> {
    /// The installed inspector, if any.
    ///
    /// Stored behind a lock as `Option<Box<..>>` (not `Arc`) so hooks can
    /// take `&mut self`: each dispatcher takes the box out, runs the hook
    /// with the lock released, and puts it back. A hook that re-enters
    /// execution therefore observes no inspector instead of deadlocking.
    inspector: Mutex<Option<Box<dyn Inspector<T> + 'a>>>,
    /// The origin the inspector is consulted with.
    inspection_origin: TransactionOrigin,
    /// Tag-to-handler map consulted by every transaction entry point.
    ///
    /// Starts from [`TxRegistry::with_defaults`](crate::TxRegistry::with_defaults),
    /// so existing transaction kinds behave exactly as before the registry
    /// existed. [`register_tx_handler`](Svm::register_tx_handler) replaces the
    /// entry for a tag, which is how tests and forks intercept execution.
    registry: TxRegistry<T, DetachedTx>,
    /// Per-instance precompile dispatch: which program ids run as precompiles
    /// and what runs for each of them.
    ///
    /// Defaults to the agave-backed native set when the `precompiles`
    /// feature is on and to [`NoPrecompiles`] otherwise. Replaced wholesale
    /// by [`set_precompile_provider`](Svm::set_precompile_provider); clones
    /// share the allocation. Providers are `'static` so batch worker VMs can
    /// share them across Rayon threads.
    precompiles: Arc<dyn PrecompileProvider<T>>,
    /// Compiled-program tier promoter (interpreter-first tiering).
    ///
    /// Shared across clones via `Arc` so batch workers and simulates observe
    /// the same promotion counters. Rebuilt whenever the JIT policy changes;
    /// disabled (no pool) unless the effective policy is `Lazy` on x86_64.
    tier_promoter: Arc<crate::programs::TierPromoter>,
    /// The custom syscall and standard-precompile registrations.
    runtime_registry: RuntimeExtensionRegistry,
    /// The account keys the active feature set reserves.
    reserved_account_keys: ReservedAccountKeys,
    /// The callback which can be used to inspect invoke_context
    /// and extract low-level information such as bpf traces, transaction
    /// context, detailed timings, etc.
    #[cfg(feature = "invocation-inspect-callback")]
    invocation_inspect_callback: Arc<dyn InvocationInspectCallback<T>>,
    /// Dictates whether or not register tracing was enabled.
    /// Provided as input to the invocation inspect callback for potential
    /// register trace consumption.
    #[cfg(feature = "invocation-inspect-callback")]
    enable_register_tracing: bool,
}

/// The pre-refactor name for [`Svm<'static, BaseSvmTypes>`].
///
/// Every call site that predates the type family still names this alias, so the
/// public-behaviour surface is unchanged by the rename. It is removed once the
/// call sites have migrated to [`Svm`].
pub type HPSVM = Svm<'static, BaseSvmTypes>;

/// Ergonomic name for the base VM instantiation.
///
/// This names exactly the same type as the all-caps spelling: `Svm<'static,
/// BaseSvmTypes>`. New code should prefer `Hpsvm`, while the all-caps spelling remains as the
/// pre-refactor compatibility spelling.
pub type Hpsvm = Svm<'static, BaseSvmTypes>;

impl<T: SvmTypesHost> std::fmt::Debug for Svm<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("Svm");
        debug
            .field("accounts", &self.state.accounts)
            .field("builtins_loaded", &self.materialized.builtins)
            .field("default_programs_loaded", &self.materialized.default_programs)
            .field("spl_programs_loaded", &self.materialized.spl_programs)
            .field("version", &self.version)
            .field("feature_accounts_loaded", &self.materialized.feature_accounts)
            .field("inspection_origin", &self.extensions.inspection_origin)
            .field("runtime_registry", &self.extensions.runtime_registry)
            .field("registry", &self.extensions.registry)
            .field("block_env", &self.block_env)
            .field("precompile_ids", &self.extensions.precompiles.program_ids())
            .field("sysvars_loaded", &self.materialized.sysvars);
        #[cfg(feature = "invocation-inspect-callback")]
        debug.field("enable_register_tracing", &self.extensions.enable_register_tracing);
        debug.finish_non_exhaustive()
    }
}

impl<T: SvmTypesHost> Svm<'_, T> {
    /// Builds a copy of this VM that owns its state privately.
    ///
    /// This is the **single place the VM's field list is written down**.
    /// [`Clone`] delegates here, and the parallel batch workers use it to
    /// swap in their own account overlay, transaction history and execution
    /// origin. Keeping one field list means a new field cannot be silently
    /// dropped by a second, hand-maintained copy of the struct.
    ///
    /// The inspector is never inherited: a copy is a scratch VM (a
    /// simulation, a batch worker) and inspector state belongs to the VM it
    /// was installed on.
    pub(crate) fn clone_with_state(&self, state: SvmState, origin: TransactionOrigin) -> Self {
        Self {
            state,
            airdrop_kp: self.airdrop_kp,
            materialized: self.materialized,
            version: self.version.clone(),
            extensions: SvmExtensions {
                inspector: Mutex::new(None),
                inspection_origin: origin,
                reserved_account_keys: self.extensions.reserved_account_keys.clone(),
                runtime_registry: self.extensions.runtime_registry.clone(),
                registry: self.extensions.registry.clone(),
                precompiles: Arc::clone(&self.extensions.precompiles),
                tier_promoter: Arc::clone(&self.extensions.tier_promoter),
                #[cfg(feature = "invocation-inspect-callback")]
                invocation_inspect_callback: self.extensions.invocation_inspect_callback.clone(),
                #[cfg(feature = "invocation-inspect-callback")]
                enable_register_tracing: self.extensions.enable_register_tracing,
            },
            block_env: self.block_env,
            _type_family: PhantomData,
        }
    }

    /// Returns whether the installed inspector would fire its hooks for an
    /// execution with `origin`.
    pub(crate) fn inspector_accepts_origin(&self, origin: TransactionOrigin) -> bool {
        self.extensions
            .inspector
            .lock()
            .as_ref()
            .is_some_and(|inspector| inspector.accepts_origin(origin))
    }

    /// Returns whether an inspector is installed, *without consulting it*.
    ///
    /// The VM consults an inspector's `accepts_origin` exactly once per
    /// execution, so a check that needs to know "is there an inspector at
    /// all" must not call it: an extra consultation is observable to an
    /// origin-recording inspector.
    pub(crate) fn has_inspector(&self) -> bool {
        self.extensions.inspector.lock().is_some()
    }
}

impl<T: SvmTypesHost> Clone for Svm<'_, T> {
    fn clone(&self) -> Self {
        self.clone_with_state(
            SvmState { accounts: self.state.accounts.clone(), history: self.state.history.clone() },
            self.extensions.inspection_origin,
        )
    }
}

impl Default for Hpsvm {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "invocation-inspect-callback")]
use solana_program_runtime::invoke_context::InvokeContext;
#[cfg(feature = "invocation-inspect-callback")]
use solana_transaction::sanitized::SanitizedTransaction;
#[cfg(feature = "invocation-inspect-callback")]
use solana_transaction_context::IndexOfAccount;

/// Low-level execution hook, parameterized by the VM's type family.
///
/// The type parameter follows [`Inspector`]: the callback receives the very host
/// the VM is instantiated with, so a family with extension slots exposes them
/// here instead of forcing a downcast.
#[cfg(feature = "invocation-inspect-callback")]
pub trait InvocationInspectCallback<T: SvmTypesHost>: Send + Sync {
    fn before_invocation(
        &self,
        svm: &Svm<'_, T>,
        tx: &SanitizedTransaction,
        program_indices: &[IndexOfAccount],
        invoke_context: &InvokeContext<'_, '_>,
    );

    fn after_invocation(
        &self,
        svm: &Svm<'_, T>,
        invoke_context: &InvokeContext<'_, '_>,
        enable_register_tracing: bool,
    );
}

#[cfg(feature = "invocation-inspect-callback")]
#[derive(Debug)]
pub struct EmptyInvocationInspectCallback;

#[cfg(feature = "invocation-inspect-callback")]
impl<T: SvmTypesHost> InvocationInspectCallback<T> for EmptyInvocationInspectCallback {
    fn before_invocation(
        &self,
        _: &Svm<'_, T>,
        _: &SanitizedTransaction,
        _: &[IndexOfAccount],
        _: &InvokeContext<'_, '_>,
    ) {
    }

    fn after_invocation(
        &self,
        _: &Svm<'_, T>,
        _: &InvokeContext<'_, '_>,
        _enable_register_tracing: bool,
    ) {
    }
}

#[cfg(test)]
mod alias_red_probe {
    use super::*;

    #[test]
    fn hpsvm_alias_is_the_base_instantiation() {
        const _: fn() = || {
            let _: fn(Hpsvm) -> Svm<'static, BaseSvmTypes> = std::convert::identity;
        };
        fn takes_both<A, B>()
        where
            A: Into<B>,
        {
        }
        takes_both::<Hpsvm, Svm<'static, BaseSvmTypes>>();
    }

    #[test]
    fn base_svm_implements_the_host_boundary() {
        fn uses_host<H: crate::host::Host<BaseSvmTypes>>(_: H) {}
        fn assert_impl<T: SvmTypesHost>()
        where
            Svm<'static, T>: crate::host::Host<T>,
        {
        }
        assert_impl::<BaseSvmTypes>();
        let _ = uses_host::<Svm<'static, BaseSvmTypes>>;
    }
}
