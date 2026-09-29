//! ## 📍 Overview
//!
//! `hpsvm` is a fast and lightweight library for testing Solana programs.
//! It works by creating an in-process Solana VM optimized for program developers.
//! This makes it much faster to run and compile than alternatives like `solana-program-test` and
//! `solana-test-validator`. In a further break from tradition, it has an ergonomic API with sane
//! defaults and extensive configurability for those who want it.
//!
//! ### 🤖 Minimal Example
//!
//! ```rust
//! use hpsvm::HPSVM;
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
//! let mut svm = HPSVM::new();
//! svm.airdrop(&from, 1_000_000_000).unwrap();
//! svm.airdrop(&to, 1_000_000_000).unwrap();
//!
//! let instruction = transfer(&from, &to, 64);
//! let tx = Transaction::new(
//!     &[&from_keypair],
//!     Message::new(&[instruction], Some(&from)),
//!     svm.latest_blockhash(),
//! );
//! let tx_res = svm.send_transaction(tx).unwrap();
//!
//! let from_account = svm.get_account(&from);
//! let to_account = svm.get_account(&to);
//! assert_eq!(from_account.unwrap().lamports, 999994936);
//! assert_eq!(to_account.unwrap().lamports, 1000000064);
//! ```
//!
//! ## Deploying Programs
//!
//! Most of the time we want to do more than just mess around with token transfers -
//! we want to test our own programs.
//!
//! Tip**: if you want to pull a Solana program from mainnet or devnet, use the `solana program
//! dump` command from the Solana CLI.
//!
//! To add a compiled program to our tests we can use
//! [`.add_program_from_file`](HPSVM::add_program_from_file).
//!
//! Here's an example using a [simple program](https://github.com/solana-labs/solana-program-library/tree/bd216c8103cd8eb9f5f32e742973e7afb52f3b81/examples/rust/logging)
//! from the Solana Program Library that just does some logging:
//!
//! ```rust
//! use hpsvm::HPSVM;
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
//!     let mut svm = HPSVM::new();
//!     let payer = Keypair::new();
//!     let bytes = include_bytes!("../test_programs/target/deploy/counter.so");
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
//! ## Time travel
//!
//! Many programs rely on the `Clock` sysvar: for example, a mint that doesn't become available
//! until after a certain time. With `hpsvm` you can dynamically overwrite the `Clock` sysvar
//! using [`svm.set_sysvar::<Clock>()`](HPSVM::set_sysvar).
//! Here's an example using a program that panics if `clock.unix_timestamp` is greater than 100
//! (which is on January 1st 1970):
//!
//! ```rust
//! use hpsvm::HPSVM;
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
//!     let mut svm = HPSVM::new();
//!     let bytes = include_bytes!("../test_programs/target/deploy/hpsvm_clock_example.so");
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
//! See also: [`warp_to_slot`](HPSVM::warp_to_slot), which lets you jump to a future slot.
//!
//! ## Writing arbitrary accounts
//!
//! HPSVM lets you write any account data you want, regardless of
//! whether the account state would even be possible.
//!
//! Here's an example where we give an account a bunch of USDC,
//! even though we don't have the USDC mint keypair. This is
//! convenient for testing because it means we don't have to
//! work with fake USDC in our tests:
//!
//! ```rust
//! use hpsvm::HPSVM;
//! use solana_account::Account;
//! use solana_address::{Address, address};
//! use solana_program_option::COption;
//! use solana_program_pack::Pack;
//! use spl_associated_token_account_interface::address::get_associated_token_address;
//! use spl_token_interface::{
//!     ID as TOKEN_PROGRAM_ID,
//!     state::{Account as TokenAccount, AccountState},
//! };
//!
//! fn test_infinite_usdc_mint() {
//!     let owner = Address::new_unique();
//!     let usdc_mint = address!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
//!     let ata = get_associated_token_address(&owner, &usdc_mint);
//!     let usdc_to_own = 1_000_000_000_000;
//!     let token_acc = TokenAccount {
//!         mint: usdc_mint,
//!         owner,
//!         amount: usdc_to_own,
//!         delegate: COption::None,
//!         state: AccountState::Initialized,
//!         is_native: COption::None,
//!         delegated_amount: 0,
//!         close_authority: COption::None,
//!     };
//!     let mut svm = HPSVM::new();
//!     let mut token_acc_bytes = [0u8; TokenAccount::LEN];
//!     TokenAccount::pack(token_acc, &mut token_acc_bytes).unwrap();
//!     svm.set_account(
//!         ata,
//!         Account {
//!             lamports: 1_000_000_000,
//!             data: token_acc_bytes.to_vec(),
//!             owner: TOKEN_PROGRAM_ID,
//!             executable: false,
//!             rent_epoch: 0,
//!         },
//!     )
//!     .unwrap();
//!     let raw_account = svm.get_account(&ata).unwrap();
//!     assert_eq!(TokenAccount::unpack(&raw_account.data).unwrap().amount, usdc_to_own)
//! }
//! ```
//!
//! ## Copying Accounts from a live environment
//!
//! If you want to copy accounts from mainnet or devnet, you can use the `solana account` command in
//! the Solana CLI to save account data to a file.
//!
//! ## Register tracing
//!
//! `hpsvm` can be instantiated with the capability to provide register tracing
//! data from processed transactions. This functionality is gated behind the
//! `register-tracing` feature flag, which in turn relies on the
//! `invocation-inspect-callback` flag. To enable it, users can either
//! construct `hpsvm` with the `HPSVM::new_debuggable` initializer - allowing
//! register tracing to be configured directly - or simply set the `SBF_TRACE_DIR`
//! environment variable, which `hpsvm` interprets as a signal to turn tracing on
//! upon instantiation. The latter allows users to take advantage of the
//! functionality without actually doing any changes to their code.
//!
//! A default post-instruction callback is provided for storing the
//! register tracing data in files. It persists the register sets,
//! the SBPF instructions, and a SHA-256 hash identifying the executable that
//! was used to generate the tracing data. If the `SBF_TRACE_DISASSEMBLE`
//! environment variable is set, a disassembled register trace will also be
//! produced for each collected register trace. The motivation behind providing the
//! SHA-256 identifier is that files may grow in number, and consumers need a
//! deterministic way to evaluate which shared object should be used when
//! analyzing the tracing data.
//!
//! Once enabled register tracing can't be changed afterwards because in nature
//! it's baked into the program executables at load time. Yet a user may want a
//! more fine-grained control over when register tracing data should be
//! collected - for example, only for a specific instruction. Such control could
//! be achieved by resetting the invocation callback to
//! `EmptyInvocationInspectCallback` and later by restoring it to
//! `DefaultRegisterTracingCallback`.
//!
//! ## Other features
//!
//! Other things you can do with `hpsvm` include:
//!
//! Changing the max compute units and other compute budget behaviour during construction with
//! [`HPSVM::builder`](HPSVM::builder) or later via [`HPSVM::set_compute_budget`]. Disable
//! transaction signature checking during construction with the builder or later via
//! [`HPSVM::set_sigverify`]. Find previous transactions using
//! [`.get_transaction`](`HPSVM::get_transaction`).
//!
//! ## When should I use `solana-test-validator`?
//!
//! While `hpsvm` is faster and more convenient, it is also less like a real RPC node.
//! So `solana-test-validator` is still useful when you need to call RPC methods that HPSVM
//! doesn't support, or when you want to test something that depends on real-life validator
//! behaviour rather than just testing your program and client code.
//!
//! In general though it is recommended to use `hpsvm` wherever possible, as it will make your life
//! much easier.

#![cfg_attr(docsrs, feature(doc_cfg))]

use std::sync::{Arc, atomic::AtomicU64};

use agave_reserved_account_keys::ReservedAccountKeys;
use solana_program_runtime::invoke_context::BuiltinFunctionRegisterer;

pub(crate) use crate::helpers::*;
use crate::{
    accounts_db::AccountsDb, history::TransactionHistory,
    runtime_registry::RuntimeExtensionRegistry,
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
/// Error types for the HPSVM.
pub mod error;
/// RPC-backed account source for forking live cluster state.
#[cfg(feature = "fork")]
pub mod fork;
/// Solana instruction types and helpers.
pub mod instruction;
/// Loader v3 helpers (set_upgrade_authority, deploy_upgradeable_program).
#[cfg(feature = "loader")]
pub mod loader;
/// SPL token operation helpers (transfer, mint, ATA, freeze, etc.).
#[cfg(feature = "token")]
pub mod token;
/// Public return types and execution metadata.
pub mod types;

mod account_source;
mod accounts_db;
mod builder;
mod callback;
mod commit;
mod env;
mod execution;
mod format_logs;
mod helpers;
mod history;
mod inspector;
mod message_processor;
#[cfg(feature = "precompiles")]
mod precompiles;
mod programs;
#[cfg(feature = "register-tracing")]
pub mod register_tracing;
mod runtime_env;
mod runtime_registry;
mod utils;
/// Core `HPSVM` construction, account/sysvar state, and transaction API.
mod vm;

/// Compiles and runs every ```rust``` block in the repository README as a
/// doctest, so the published quick-start examples cannot drift from the API.
#[cfg(doctest)]
#[doc = include_str!("../../../README.md")]
struct ReadmeExamples;

pub use account_source::{AccountSource, AccountSourceError};
pub use accounts_db::AccountsView;
pub use builder::{FeatureConfigOpen, FeatureConfigSealed, HpsvmBuilder};
pub use env::{BlockEnv, RuntimeEnv, SvmCfg};
pub use inspector::{Inspector, TransactionOrigin};

// Re-export crate-internal items at the root so child modules can reference
// them as `crate::X` (descendant modules can read root-private items, but the
// names must resolve via the crate root).
pub(crate) use crate::commit::{CommitDelta, apply_commit_delta, outcome_into_result_and_delta};
pub(crate) use crate::error::HPSVMError;

static NEXT_VM_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_vm_instance_id() -> u64 {
    NEXT_VM_INSTANCE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// The core Solana Virtual Machine simulator.
///
/// HPSVM manages account state, program execution, and transaction processing
/// in a deterministic, single-threaded environment.
pub struct HPSVM {
    accounts: AccountsDb,
    airdrop_kp: [u8; 64],
    builtins_loaded: bool,
    default_programs_loaded: bool,
    spl_programs_loaded: bool,
    cfg: SvmCfg,
    feature_accounts_loaded: bool,
    inspector: Arc<dyn Inspector>,
    inspection_origin: TransactionOrigin,
    reserved_account_keys: ReservedAccountKeys,
    runtime_registry: RuntimeExtensionRegistry,
    instance_id: u64,
    state_version: u64,
    block_env: BlockEnv,
    history: TransactionHistory,
    runtime_env: RuntimeEnv,
    sysvars_loaded: bool,
    /// The callback which can be used to inspect invoke_context
    /// and extract low-level information such as bpf traces, transaction
    /// context, detailed timings, etc.
    #[cfg(feature = "invocation-inspect-callback")]
    invocation_inspect_callback: Arc<dyn InvocationInspectCallback>,
    /// Dictates whether or not register tracing was enabled.
    /// Provided as input to the invocation inspect callback for potential
    /// register trace consumption.
    #[cfg(feature = "invocation-inspect-callback")]
    enable_register_tracing: bool,
}

impl std::fmt::Debug for HPSVM {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("HPSVM");
        debug
            .field("accounts", &self.accounts)
            .field("builtins_loaded", &self.builtins_loaded)
            .field("default_programs_loaded", &self.default_programs_loaded)
            .field("spl_programs_loaded", &self.spl_programs_loaded)
            .field("cfg", &self.cfg)
            .field("feature_accounts_loaded", &self.feature_accounts_loaded)
            .field("inspection_origin", &self.inspection_origin)
            .field("runtime_registry", &self.runtime_registry)
            .field("state_version", &self.state_version)
            .field("block_env", &self.block_env)
            .field("runtime_env", &self.runtime_env)
            .field("sysvars_loaded", &self.sysvars_loaded);
        #[cfg(feature = "invocation-inspect-callback")]
        debug.field("enable_register_tracing", &self.enable_register_tracing);
        debug.finish_non_exhaustive()
    }
}

impl Clone for HPSVM {
    fn clone(&self) -> Self {
        Self {
            accounts: self.accounts.clone(),
            airdrop_kp: self.airdrop_kp,
            builtins_loaded: self.builtins_loaded,
            default_programs_loaded: self.default_programs_loaded,
            spl_programs_loaded: self.spl_programs_loaded,
            cfg: self.cfg.clone(),
            feature_accounts_loaded: self.feature_accounts_loaded,
            inspector: self.inspector.clone(),
            inspection_origin: self.inspection_origin,
            reserved_account_keys: self.reserved_account_keys.clone(),
            runtime_registry: self.runtime_registry.clone(),
            instance_id: next_vm_instance_id(),
            state_version: self.state_version,
            block_env: self.block_env,
            history: self.history.clone(),
            runtime_env: self.runtime_env,
            sysvars_loaded: self.sysvars_loaded,
            #[cfg(feature = "invocation-inspect-callback")]
            invocation_inspect_callback: self.invocation_inspect_callback.clone(),
            #[cfg(feature = "invocation-inspect-callback")]
            enable_register_tracing: self.enable_register_tracing,
        }
    }
}

impl Default for HPSVM {
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

#[cfg(feature = "invocation-inspect-callback")]
pub trait InvocationInspectCallback: Send + Sync {
    fn before_invocation(
        &self,
        svm: &HPSVM,
        tx: &SanitizedTransaction,
        program_indices: &[IndexOfAccount],
        invoke_context: &InvokeContext<'_, '_>,
    );

    fn after_invocation(
        &self,
        svm: &HPSVM,
        invoke_context: &InvokeContext<'_, '_>,
        enable_register_tracing: bool,
    );
}

#[cfg(feature = "invocation-inspect-callback")]
#[derive(Debug)]
pub struct EmptyInvocationInspectCallback;

#[cfg(feature = "invocation-inspect-callback")]
impl InvocationInspectCallback for EmptyInvocationInspectCallback {
    fn before_invocation(
        &self,
        _: &HPSVM,
        _: &SanitizedTransaction,
        _: &[IndexOfAccount],
        _: &InvokeContext<'_, '_>,
    ) {
    }

    fn after_invocation(
        &self,
        _: &HPSVM,
        _: &InvokeContext<'_, '_>,
        _enable_register_tracing: bool,
    ) {
    }
}
