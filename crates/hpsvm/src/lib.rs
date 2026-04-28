//! <div align="center">
//! <img src="https://raw.githubusercontent.com/hpsvm/hpsvm/master/logo.jpeg" width="50%" height="50%">
//! </div>
//!
//! ---
//!
//! # HPSVM
//!
//! [<img alt="github" src="https://img.shields.io/badge/github-HPSVM/hpsvm-8da0cb?style=for-the-badge&labelColor=555555&logo=github" height="20">](https://github.com/HPSVM/hpsvm)
//! [<img alt="crates.io" src="https://img.shields.io/crates/v/hpsvm.svg?style=for-the-badge&color=fc8d62&logo=rust" height="20">](https://crates.io/crates/hpsvm)
//! [<img alt="docs.rs" src="https://img.shields.io/badge/docs.rs-hpsvm-66c2a5?style=for-the-badge&labelColor=555555&logo=docs.rs" height="20">](https://docs.rs/hpsvm/latest/hpsvm/)
//! [<img alt="build status" src="https://img.shields.io/github/actions/workflow/status/HPSVM/hpsvm/CI.yml?branch=master&style=for-the-badge" height="20">](https://github.com/HPSVM/hpsvm/actions?query=branch%3Amaster)
//!
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
//! Changing the max compute units and other compute budget behaviour using
//! [`.with_compute_budget`](HPSVM::with_compute_budget). Disable transaction signature checking
//! using [`.with_sigverify(false)`](HPSVM::with_sigverify). Find previous transactions using
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

use std::{
    cell::RefCell,
    path::Path,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use agave_feature_set::{
    FeatureSet, increase_cpi_account_info_limit, raise_cpi_nesting_limit_to_8,
};
use agave_reserved_account_keys::ReservedAccountKeys;
use agave_syscalls::{
    create_program_runtime_environment_v1, create_program_runtime_environment_v2,
};
use log::error;
#[cfg(feature = "precompiles")]
use precompiles::load_precompiles;
#[cfg(feature = "nodejs-internal")]
use qualifier_attr::qualifiers;
use serde::de::DeserializeOwned;
use solana_account::{
    Account, AccountSharedData, ReadableAccount, WritableAccount, state_traits::StateMut,
};
use solana_address::Address;
use solana_builtins::BUILTINS;
use solana_clock::Clock;
use solana_compute_budget::{
    compute_budget::ComputeBudget, compute_budget_limits::ComputeBudgetLimits,
};
use solana_compute_budget_instruction::instructions_processor::process_compute_budget_instructions;
use solana_epoch_rewards::EpochRewards;
use solana_epoch_schedule::EpochSchedule;
use solana_feature_gate_interface::{self as feature_gate, Feature};
use solana_fee::FeeFeatures;
use solana_fee_structure::FeeStructure;
use solana_hash::Hash;
use solana_keypair::Keypair;
use solana_last_restart_slot::LastRestartSlot;
use solana_loader_v3_interface::{get_program_data_address, state::UpgradeableLoaderState};
use solana_message::{
    Message, SanitizedMessage, VersionedMessage, inner_instruction::InnerInstructionsList,
};
use solana_native_token::LAMPORTS_PER_SOL;
use solana_nonce::{NONCED_TX_MARKER_IX_INDEX, state::DurableNonce};
use solana_program_runtime::{
    invoke_context::{BuiltinFunctionWithContext, EnvironmentConfig, InvokeContext},
    loaded_programs::{LoadProgramMetrics, ProgramCacheEntry},
    solana_sbpf::program::BuiltinFunction,
};
use solana_rent::Rent;
use solana_sdk_ids::{
    bpf_loader, bpf_loader_deprecated, bpf_loader_upgradeable, native_loader, system_program,
};
use solana_signature::Signature;
use solana_signer::Signer;
use solana_slot_hashes::SlotHashes;
use solana_slot_history::SlotHistory;
use solana_stake_interface::stake_history::StakeHistory;
use solana_svm_log_collector::LogCollector;
use solana_svm_timings::ExecuteTimings;
use solana_svm_transaction::svm_message::SVMMessage;
use solana_system_program::{SystemAccountKind, get_system_account_kind};
#[expect(deprecated)]
use solana_sysvar::recent_blockhashes::IterItem;
use solana_sysvar::{Sysvar, SysvarSerialize};
#[expect(deprecated)]
use solana_sysvar::{fees::Fees, recent_blockhashes::RecentBlockhashes};
use solana_sysvar_id::SysvarId;
use solana_transaction::{
    sanitized::{MAX_TX_ACCOUNT_LOCKS, MessageHash, SanitizedTransaction},
    versioned::VersionedTransaction,
};
use solana_transaction_context::{ExecutionRecord, IndexOfAccount, TransactionContext};
use solana_transaction_error::TransactionError;
use types::SimulatedTransactionInfo;
use utils::{
    construct_instructions_account,
    inner_instructions::inner_instructions_list_from_instruction_trace,
};

#[cfg(feature = "register-tracing")]
use crate::register_tracing::DefaultRegisterTracingCallback;
use crate::{
    accounts_db::AccountsDb,
    batch::{TransactionBatchError, TransactionBatchExecutionResult, TransactionBatchPlan},
    error::HPSVMError,
    history::TransactionHistory,
    message_processor::process_message,
    programs::{DEFAULT_PROGRAM_IDS, load_default_programs},
    types::{
        ExecutionOutcome, ExecutionResult, FailedTransactionMetadata, TransactionMetadata,
        TransactionResult,
    },
    utils::{
        create_blockhash,
        rent::{RentState, check_rent_state_with_account, get_account_rent_state},
    },
};

#[derive(Clone)]
pub(crate) struct CustomSyscallRegistration {
    name: String,
    function: BuiltinFunction<InvokeContext<'static, 'static>>,
}

#[expect(missing_docs)]
pub mod batch;
#[expect(missing_docs)]
pub mod error;
#[expect(missing_docs)]
pub mod types;

mod account_source;
mod accounts_db;
mod callback;
mod env;
mod format_logs;
mod history;
mod inspector;
mod message_processor;
#[cfg(feature = "precompiles")]
mod precompiles;
mod programs;
#[cfg(feature = "register-tracing")]
pub mod register_tracing;
mod runtime_registry;
mod utils;

pub use account_source::{AccountSource, AccountSourceError};
pub use accounts_db::AccountsView;
pub use env::{BlockEnv, RuntimeEnv, SvmCfg};
pub use inspector::Inspector;
use runtime_registry::RuntimeExtensionRegistry;

static NEXT_VM_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_vm_instance_id() -> u64 {
    NEXT_VM_INSTANCE_ID.fetch_add(1, Ordering::Relaxed)
}

#[expect(missing_docs)]
pub struct HPSVM {
    accounts: AccountsDb,
    airdrop_kp: [u8; 64],
    builtins_loaded: bool,
    default_programs_loaded: bool,
    cfg: SvmCfg,
    feature_accounts_loaded: bool,
    inspector: Arc<dyn Inspector>,
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
            .field("cfg", &self.cfg)
            .field("feature_accounts_loaded", &self.feature_accounts_loaded)
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
            cfg: self.cfg.clone(),
            feature_accounts_loaded: self.feature_accounts_loaded,
            inspector: self.inspector.clone(),
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
        // Allow users to virtually get register tracing data without
        // doing any changes to their code provided `SBF_TRACE_DIR` is set.
        #[cfg(feature = "register-tracing")]
        let enable_register_tracing = std::env::var("SBF_TRACE_DIR").is_ok();
        #[cfg(not(feature = "register-tracing"))]
        let enable_register_tracing = false;

        Self::new_inner(enable_register_tracing)
    }
}

impl HPSVM {
    const fn invalidate_execution_outcomes(&mut self) {
        self.state_version = self.state_version.wrapping_add(1);
    }

    fn sync_block_env_slot(&mut self) {
        self.block_env.slot = self.accounts.current_slot();
    }

    fn new_inner(_enable_register_tracing: bool) -> Self {
        let feature_set = FeatureSet::default();
        let latest_blockhash = create_blockhash(b"genesis");

        Self {
            accounts: Default::default(),
            airdrop_kp: Keypair::new().to_bytes(),
            builtins_loaded: false,
            default_programs_loaded: false,
            reserved_account_keys: Self::reserved_account_keys_for_feature_set(&feature_set),
            cfg: SvmCfg {
                feature_set,
                sigverify: false,
                blockhash_check: false,
                fee_structure: FeeStructure::default(),
            },
            feature_accounts_loaded: false,
            inspector: Arc::new(inspector::NoopInspector),
            runtime_registry: RuntimeExtensionRegistry::default(),
            instance_id: next_vm_instance_id(),
            state_version: 0,
            block_env: BlockEnv { latest_blockhash, slot: 0 },
            history: TransactionHistory::new(),
            runtime_env: RuntimeEnv { compute_budget: None, log_bytes_limit: Some(10_000) },
            sysvars_loaded: false,
            #[cfg(feature = "invocation-inspect-callback")]
            enable_register_tracing: _enable_register_tracing,
            #[cfg(feature = "invocation-inspect-callback")]
            invocation_inspect_callback: {
                #[cfg(feature = "register-tracing")]
                if _enable_register_tracing {
                    Arc::new(DefaultRegisterTracingCallback::default())
                } else {
                    Arc::new(EmptyInvocationInspectCallback {})
                }
                #[cfg(not(feature = "register-tracing"))]
                Arc::new(EmptyInvocationInspectCallback {})
            },
        }
    }

    fn into_basic(self) -> Self {
        let svm = self
            .with_feature_set(FeatureSet::all_enabled())
            .with_builtins()
            .with_lamports(1_000_000u64.wrapping_mul(LAMPORTS_PER_SOL))
            .with_sysvars()
            .with_feature_accounts()
            .with_default_programs()
            .with_sigverify(true)
            .with_blockhash_check(true);

        #[cfg(feature = "precompiles")]
        let svm = svm.with_precompiles();

        svm
    }

    /// Creates the basic test environment.
    pub fn new() -> Self {
        Self::default().into_basic()
    }

    /// Installs an execution inspector that observes top-level transaction activity.
    pub fn with_inspector<I: Inspector + 'static>(mut self, inspector: I) -> Self {
        self.inspector = Arc::new(inspector);
        self.invalidate_execution_outcomes();
        self
    }

    fn on_transaction_start(&self, tx: &SanitizedTransaction) {
        self.inspector.on_transaction_start(self, tx);
    }

    pub(crate) fn on_instruction(&self, index: usize, program_id: &Address) {
        self.inspector.on_instruction(self, index, program_id);
    }

    fn on_transaction_end(&self, result: &solana_transaction_error::TransactionResult<()>) {
        self.inspector.on_transaction_end(self, result);
    }

    #[cfg(feature = "register-tracing")]
    /// Create a test environment with debugging features.
    ///
    /// This constructor allows enabling low-level VM debugging capabilities,
    /// such as register tracing, which are baked into program executables at
    /// load time and cannot be changed afterwards.
    ///
    /// When `enable_register_tracing` is `true`:
    /// - Programs are loaded with register tracing support
    /// - A default [`DefaultRegisterTracingCallback`] is installed
    /// - Trace data is written to `SBF_TRACE_DIR` (or `target/sbf/trace` by default)
    pub fn new_debuggable(enable_register_tracing: bool) -> Self {
        Self::new_inner(enable_register_tracing).into_basic()
    }

    fn clear_feature_accounts(&mut self, previous_feature_set: &FeatureSet) {
        previous_feature_set.active().iter().for_each(|(feature_id, _)| {
            self.accounts.remove_account(feature_id);
        });
    }

    fn clear_builtin_accounts(&mut self) {
        for builtin in BUILTINS {
            self.accounts.remove_account(&builtin.program_id);
        }
    }

    fn clear_default_programs(&mut self) {
        for program_id in &DEFAULT_PROGRAM_IDS {
            if self
                .accounts
                .get_account_ref(program_id)
                .is_some_and(|account| account.owner() == &bpf_loader_upgradeable::id())
            {
                self.accounts.remove_account(&get_program_data_address(program_id));
            }
            self.accounts.remove_account(program_id);
        }
    }

    #[cfg(feature = "precompiles")]
    fn clear_precompile_accounts(&mut self) {
        agave_precompiles::get_precompiles().iter().for_each(|precompile| {
            self.accounts.remove_account(&precompile.program_id);
        });
    }

    fn try_refresh_runtime_environments(&mut self) -> Result<(), HPSVMError> {
        #[cfg(feature = "register-tracing")]
        let enable_register_tracing = self.enable_register_tracing;
        #[cfg(not(feature = "register-tracing"))]
        let enable_register_tracing = false;

        let compute_budget = self.runtime_env.compute_budget.unwrap_or_else(|| {
            ComputeBudget::new_with_defaults(
                self.cfg.feature_set.is_active(&raise_cpi_nesting_limit_to_8::ID),
                self.cfg.feature_set.is_active(&increase_cpi_account_info_limit::ID),
            )
        });
        let mut program_runtime_v1 = create_program_runtime_environment_v1(
            &self.cfg.feature_set.runtime_features(),
            &compute_budget.to_budget(),
            false,
            enable_register_tracing,
        )
        .map_err(|error| HPSVMError::RuntimeEnvironment {
            version: "v1",
            reason: error.to_string(),
        })?;

        let mut program_runtime_v2 = create_program_runtime_environment_v2(
            &compute_budget.to_budget(),
            enable_register_tracing,
        );

        for syscall in self.runtime_registry.custom_syscalls() {
            program_runtime_v1.register_function(&syscall.name, syscall.function).map_err(
                |error| HPSVMError::CustomSyscallRegistration {
                    name: syscall.name.clone(),
                    runtime: "runtime_v1",
                    reason: error.to_string(),
                },
            )?;
            program_runtime_v2.register_function(&syscall.name, syscall.function).map_err(
                |error| HPSVMError::CustomSyscallRegistration {
                    name: syscall.name.clone(),
                    runtime: "runtime_v2",
                    reason: error.to_string(),
                },
            )?;
        }

        let environments = self.accounts.runtime_environments_mut();
        environments.program_runtime_v1 = Arc::new(program_runtime_v1);
        environments.program_runtime_v2 = Arc::new(program_runtime_v2);

        Ok(())
    }

    fn refresh_runtime_environments(&mut self) {
        self.try_refresh_runtime_environments()
            .expect("runtime environment refresh should never fail for internal configuration");
    }

    fn reconfigure_materialized_feature_state(&mut self, previous_feature_set: &FeatureSet) {
        if self.feature_accounts_loaded {
            self.clear_feature_accounts(previous_feature_set);
            self.materialize_feature_accounts();
        }

        if self.default_programs_loaded {
            self.clear_default_programs();
        }

        if self.builtins_loaded {
            self.clear_builtin_accounts();
            self.load_builtins();
        } else if !self.runtime_registry.custom_syscalls().is_empty() {
            self.refresh_runtime_environments();
        }

        #[cfg(feature = "precompiles")]
        if self.runtime_registry.loads_standard_precompiles() {
            self.clear_precompile_accounts();
            self.load_precompiles();
        }

        if self.default_programs_loaded {
            self.load_default_programs();
        }

        assert!(
            self.accounts.rebuild_program_cache().is_ok(),
            "feature-set reconfiguration produced invalid program cache state"
        );
    }

    #[cfg_attr(feature = "nodejs-internal", qualifiers(pub))]
    const fn set_compute_budget(&mut self, compute_budget: ComputeBudget) {
        self.runtime_env.compute_budget = Some(compute_budget);
        self.invalidate_execution_outcomes();
    }

    /// Sets the compute budget.
    pub const fn with_compute_budget(mut self, compute_budget: ComputeBudget) -> Self {
        self.set_compute_budget(compute_budget);
        self
    }

    #[cfg_attr(feature = "nodejs-internal", qualifiers(pub))]
    const fn set_sigverify(&mut self, sigverify: bool) {
        self.cfg.sigverify = sigverify;
        self.invalidate_execution_outcomes();
    }

    /// Enables or disables sigverify.
    pub const fn with_sigverify(mut self, sigverify: bool) -> Self {
        self.set_sigverify(sigverify);
        self
    }

    #[cfg_attr(feature = "nodejs-internal", qualifiers(pub))]
    const fn set_blockhash_check(&mut self, check: bool) {
        self.cfg.blockhash_check = check;
        self.invalidate_execution_outcomes();
    }

    /// Enables or disables the blockhash check.
    pub const fn with_blockhash_check(mut self, check: bool) -> Self {
        self.set_blockhash_check(check);
        self
    }

    #[cfg_attr(feature = "nodejs-internal", qualifiers(pub))]
    fn set_sysvars(&mut self) {
        self.sysvars_loaded = true;
        self.set_sysvar_internal(&Clock::default());
        self.set_sysvar_internal(&EpochRewards::default());
        self.set_sysvar_internal(&EpochSchedule::default());
        #[expect(deprecated)]
        let fees = Fees::default();
        self.set_sysvar_internal(&fees);
        self.set_sysvar_internal(&LastRestartSlot::default());
        let latest_blockhash = self.block_env.latest_blockhash;
        #[expect(deprecated)]
        self.set_sysvar_internal(&RecentBlockhashes::from_iter([IterItem(
            0,
            &latest_blockhash,
            fees.fee_calculator.lamports_per_signature,
        )]));

        // Rent account differs based off feature gating
        #[expect(deprecated)]
        {
            let mut rent_account = Rent::default();
            if self
                .cfg
                .feature_set
                .is_active(&agave_feature_set::deprecate_rent_exemption_threshold::id())
            {
                rent_account.exemption_threshold = 1.0;
                rent_account.lamports_per_byte_year = solana_rent::DEFAULT_LAMPORTS_PER_BYTE;
            }
            self.set_sysvar_internal(&rent_account);
        }
        self.set_sysvar_internal(&SlotHashes::new(&[(
            self.accounts.current_slot(),
            latest_blockhash,
        )]));
        self.set_sysvar_internal(&SlotHistory::default());
        self.set_sysvar_internal(&StakeHistory::default());
        self.invalidate_execution_outcomes();
    }

    /// Includes the default sysvars.
    pub fn with_sysvars(mut self) -> Self {
        self.set_sysvars();
        self
    }

    /// Set the FeatureSet used by the VM instance.
    pub fn with_feature_set(mut self, feature_set: FeatureSet) -> Self {
        self.set_feature_set(feature_set);
        self
    }

    #[cfg_attr(feature = "nodejs-internal", qualifiers(pub))]
    fn set_feature_set(&mut self, feature_set: FeatureSet) {
        let previous_feature_set = self.cfg.feature_set.clone();
        self.cfg.feature_set = feature_set;
        self.reserved_account_keys =
            Self::reserved_account_keys_for_feature_set(&self.cfg.feature_set);
        self.reconfigure_materialized_feature_state(&previous_feature_set);
        self.invalidate_execution_outcomes();
    }

    fn materialize_feature_accounts(&mut self) {
        self.feature_accounts_loaded = true;
        for (feature_id, activation_slot) in self.cfg.feature_set.active() {
            let feature_account = Feature { activated_at: Some(*activation_slot) };
            let lamports = self.minimum_balance_for_rent_exemption(Feature::size_of());
            let account = feature_gate::create_account(&feature_account, lamports);
            self.accounts.add_account_no_checks(*feature_id, account);
        }
    }

    #[cfg_attr(feature = "nodejs-internal", qualifiers(pub))]
    fn set_feature_accounts(&mut self) {
        self.materialize_feature_accounts();
        self.invalidate_execution_outcomes();
    }

    #[expect(missing_docs)]
    pub fn with_feature_accounts(mut self) -> Self {
        self.set_feature_accounts();
        self
    }

    fn reserved_account_keys_for_feature_set(feature_set: &FeatureSet) -> ReservedAccountKeys {
        let mut reserved_account_keys = ReservedAccountKeys::default();
        reserved_account_keys.update_active_set(feature_set);
        reserved_account_keys
    }

    fn load_builtins(&mut self) {
        self.builtins_loaded = true;
        self.refresh_runtime_environments();
        for builtint in BUILTINS {
            if builtint.enable_feature_id.is_none_or(|x| self.cfg.feature_set.is_active(&x)) {
                let loaded_program =
                    ProgramCacheEntry::new_builtin(0, builtint.name.len(), builtint.entrypoint);
                self.accounts
                    .replenish_program_cache(builtint.program_id, Arc::new(loaded_program));
                self.accounts.add_builtin_account(
                    builtint.program_id,
                    crate::utils::create_loadable_account_for_test(builtint.name),
                );
            }
        }
    }

    #[cfg_attr(feature = "nodejs-internal", qualifiers(pub))]
    fn set_builtins(&mut self) {
        self.load_builtins();
        self.invalidate_execution_outcomes();
    }

    /// Changes the default builtins.
    // Use `with_feature_set` beforehand to change change what builtins are added.
    pub fn with_builtins(mut self) -> Self {
        self.set_builtins();
        self
    }

    #[cfg_attr(feature = "nodejs-internal", qualifiers(pub))]
    fn set_lamports(&mut self, lamports: u64) {
        self.accounts.add_account_no_checks(
            Keypair::try_from(self.airdrop_kp.as_slice())
                .expect("airdrop keypair should be valid")
                .pubkey(),
            AccountSharedData::new(lamports, 0, &system_program::id()),
        );
        self.invalidate_execution_outcomes();
    }

    /// Changes the initial lamports in HPSVM's airdrop account.
    pub fn with_lamports(mut self, lamports: u64) -> Self {
        self.set_lamports(lamports);
        self
    }

    fn load_default_programs(&mut self) {
        self.default_programs_loaded = true;
        load_default_programs(self);
    }

    #[cfg_attr(feature = "nodejs-internal", qualifiers(pub))]
    fn set_default_programs(&mut self) {
        self.load_default_programs();
        self.invalidate_execution_outcomes();
    }

    /// Includes the standard SPL programs.
    pub fn with_default_programs(mut self) -> Self {
        self.set_default_programs();
        self
    }

    #[cfg_attr(feature = "nodejs-internal", qualifiers(pub))]
    fn set_transaction_history(&mut self, capacity: usize) {
        self.history.set_capacity(capacity);
        self.invalidate_execution_outcomes();
    }

    /// Changes the capacity of the transaction history.
    /// Set this to 0 to disable transaction history and allow duplicate transactions.
    pub fn with_transaction_history(mut self, capacity: usize) -> Self {
        self.set_transaction_history(capacity);
        self
    }

    fn set_account_source(&mut self, source: impl AccountSource + 'static) {
        self.accounts.set_account_source(Arc::new(source));
        self.invalidate_execution_outcomes();
    }

    /// Configures a read-through account source used when an account is missing from local state.
    pub fn with_account_source(mut self, source: impl AccountSource + 'static) -> Self {
        self.set_account_source(source);
        self
    }

    #[cfg_attr(feature = "nodejs-internal", qualifiers(pub))]
    const fn set_log_bytes_limit(&mut self, limit: Option<usize>) {
        self.runtime_env.log_bytes_limit = limit;
        self.invalidate_execution_outcomes();
    }

    #[expect(missing_docs)]
    pub const fn with_log_bytes_limit(mut self, limit: Option<usize>) -> Self {
        self.set_log_bytes_limit(limit);
        self
    }

    #[cfg(feature = "precompiles")]
    fn load_precompiles(&mut self) {
        load_precompiles(self);
    }

    #[cfg_attr(feature = "nodejs-internal", qualifiers(pub))]
    #[cfg(feature = "precompiles")]
    fn set_precompiles(&mut self) {
        self.runtime_registry.enable_standard_precompiles();
        self.load_precompiles();
        self.invalidate_execution_outcomes();
    }

    /// Adds the standard precompiles to the VM.
    // Use `with_feature_set` beforehand to change change what precompiles are added.
    #[cfg(feature = "precompiles")]
    pub fn with_precompiles(mut self) -> Self {
        self.set_precompiles();
        self
    }

    /// Returns minimum balance required to make an account with specified data length rent exempt.
    pub fn minimum_balance_for_rent_exemption(&self, data_len: usize) -> u64 {
        self.accounts.minimum_balance_for_rent_exemption(data_len)
    }

    /// Returns all information associated with the account of the provided pubkey.
    pub fn get_account(&self, address: &Address) -> Option<Account> {
        self.accounts.get_account(address).map(Into::into)
    }

    /// **⚠️ ADVANCED USE ONLY ⚠️**
    ///
    /// Sets all information associated with the account of the provided pubkey.
    ///
    /// This writes directly into the in-memory test state. It does not execute
    /// the owning program or replay the full transaction pipeline, so it is best
    /// used for fixtures, snapshots, and explicit state surgery between
    /// transactions. Prefer [`airdrop`](HPSVM::airdrop) or
    /// [`send_transaction`](HPSVM::send_transaction) when you want a
    /// protocol-consistent state transition.
    pub fn set_account(&mut self, address: Address, data: Account) -> Result<(), HPSVMError> {
        self.accounts.add_account(address, data.into())?;
        self.sync_block_env_slot();
        self.invalidate_execution_outcomes();
        Ok(())
    }

    /// **⚠️ ADVANCED USE ONLY ⚠️**
    ///
    /// Returns a read-only view of the internal accounts database.
    ///
    /// This provides read-only access to the accounts database for advanced inspection.
    /// Use [`get_account`](HPSVM::get_account) for normal account retrieval.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hpsvm::HPSVM;
    ///
    /// let svm = HPSVM::new();
    ///
    /// // Read-only access to accounts data
    /// let accounts = svm.accounts();
    /// // ... inspect internal state if needed
    /// ```
    pub const fn accounts(&self) -> AccountsView<'_> {
        AccountsView::new(&self.accounts)
    }

    /// Gets the balance of the provided account pubkey.
    pub fn get_balance(&self, address: &Address) -> Option<u64> {
        self.accounts.get_account(address).map(|account| account.lamports())
    }

    /// Gets the latest blockhash.
    pub const fn latest_blockhash(&self) -> Hash {
        self.block_env.latest_blockhash
    }

    /// Gets the current block environment.
    pub const fn block_env(&self) -> BlockEnv {
        self.block_env
    }

    /// **⚠️ ADVANCED USE ONLY ⚠️**
    ///
    /// Sets the sysvar in the test environment.
    ///
    /// This is a direct override intended for tests that need to manipulate
    /// runtime context. It bypasses transaction execution.
    ///
    /// Returns an error if serialization fails or if the sysvar account update
    /// is rejected by the internal accounts database.
    pub fn set_sysvar<T>(&mut self, sysvar: &T) -> Result<(), HPSVMError>
    where
        T: Sysvar + SysvarId + SysvarSerialize,
    {
        self.try_set_sysvar(sysvar)?;
        self.sync_block_env_slot();
        self.invalidate_execution_outcomes();
        Ok(())
    }

    fn try_set_sysvar<T>(&mut self, sysvar: &T) -> Result<(), HPSVMError>
    where
        T: Sysvar + SysvarId + SysvarSerialize,
    {
        let mut account = AccountSharedData::new(1, T::size_of(), &solana_sdk_ids::sysvar::id());
        account.serialize_data(sysvar).map_err(|error| HPSVMError::SysvarSerialization {
            sysvar: std::any::type_name::<T>(),
            reason: error.to_string(),
        })?;
        self.accounts.add_account(T::id(), account)
    }

    fn set_sysvar_internal<T>(&mut self, sysvar: &T)
    where
        T: Sysvar + SysvarId + SysvarSerialize,
    {
        self.try_set_sysvar(sysvar)
            .expect("internal sysvar setup should never fail for supported sysvars");
        self.sync_block_env_slot();
    }

    /// Gets a sysvar from the test environment.
    pub fn get_sysvar<T>(&self) -> T
    where
        T: Sysvar + SysvarId + DeserializeOwned,
    {
        self.accounts
            .get_account_ref(&T::id())
            .expect("sysvar account should exist")
            .deserialize_data()
            .expect("sysvar deserialization should never fail")
    }

    /// Gets a transaction from the transaction history.
    pub fn get_transaction(&self, signature: &Signature) -> Option<&TransactionResult> {
        self.history.get_transaction(signature)
    }

    /// Returns the pubkey of the internal airdrop account.
    pub fn airdrop_pubkey(&self) -> Address {
        Keypair::try_from(self.airdrop_kp.as_slice())
            .expect("airdrop keypair should be valid")
            .pubkey()
    }

    /// Airdrops lamports by submitting an internal system transfer transaction.
    ///
    /// Unlike [`set_account`](HPSVM::set_account), this goes through the normal
    /// execution pipeline instead of mutating balances directly.
    pub fn airdrop(&mut self, address: &Address, lamports: u64) -> TransactionResult {
        let payer =
            Keypair::try_from(self.airdrop_kp.as_slice()).expect("airdrop keypair should be valid");
        let tx = VersionedTransaction::try_new(
            VersionedMessage::Legacy(Message::new_with_blockhash(
                &[solana_system_interface::instruction::transfer(
                    &payer.pubkey(),
                    address,
                    lamports,
                )],
                Some(&payer.pubkey()),
                &self.block_env.latest_blockhash,
            )),
            &[payer],
        )
        .expect("failed to create airdrop transaction");

        let inspector = std::mem::replace(&mut self.inspector, Arc::new(inspector::NoopInspector));
        let result = self.send_transaction(tx);
        self.inspector = inspector;
        result
    }

    /// Adds a builtin program to the test environment.
    pub fn add_builtin(&mut self, program_id: Address, entrypoint: BuiltinFunctionWithContext) {
        let builtin = ProgramCacheEntry::new_builtin(self.accounts.current_slot(), 1, entrypoint);

        self.accounts.replenish_program_cache(program_id, Arc::new(builtin));

        let mut account = AccountSharedData::new(1, 1, &bpf_loader::id());
        account.set_executable(true);
        self.accounts.add_account_no_checks(program_id, account);
        self.invalidate_execution_outcomes();
    }

    /// Adds an SBF program to the test environment from the file specified.
    pub fn add_program_from_file(
        &mut self,
        program_id: impl Into<Address>,
        path: impl AsRef<Path>,
    ) -> Result<(), HPSVMError> {
        let bytes = std::fs::read(path)?;
        self.add_program(program_id, &bytes)?;
        Ok(())
    }

    fn add_program_internal<const PREVERIFIED: bool>(
        &mut self,
        program_id: impl Into<Address>,
        program_bytes: &[u8],
        loader_id: &Address,
    ) -> Result<(), HPSVMError> {
        let program_id = program_id.into();
        let current_slot = self.accounts.current_slot();

        let program_size = if bpf_loader_upgradeable::check_id(loader_id) {
            let (programdata_address, _bump) =
                Address::find_program_address(&[program_id.as_ref()], loader_id);

            let programdata_metadata_len = UpgradeableLoaderState::size_of_programdata_metadata();
            let programdata_len = programdata_metadata_len + program_bytes.len();
            let programdata_lamports = self.minimum_balance_for_rent_exemption(programdata_len);
            let mut programdata_account =
                AccountSharedData::new(programdata_lamports, programdata_len, loader_id);
            programdata_account
                .set_state(&UpgradeableLoaderState::ProgramData {
                    slot: current_slot,
                    upgrade_authority_address: None,
                })
                .expect("UpgradeableLoaderState::ProgramData serialization should never fail");
            programdata_account.data_as_mut_slice()[programdata_metadata_len..]
                .copy_from_slice(program_bytes);

            let program_len = UpgradeableLoaderState::size_of_program();
            let program_lamports = self.minimum_balance_for_rent_exemption(program_len);
            let mut program_account =
                AccountSharedData::new(program_lamports, program_len, loader_id);
            program_account.set_executable(true);
            program_account
                .set_state(&UpgradeableLoaderState::Program { programdata_address })
                .expect("UpgradeableLoaderState::Program serialization should never fail");

            self.accounts.add_account_no_checks(programdata_address, programdata_account);
            self.accounts.add_account_no_checks(program_id, program_account);

            programdata_len
        } else if bpf_loader::check_id(loader_id) || bpf_loader_deprecated::check_id(loader_id) {
            let program_len = program_bytes.len();
            let lamports = self.minimum_balance_for_rent_exemption(program_len);
            let mut account = AccountSharedData::new(lamports, program_len, loader_id);
            account.set_executable(true);
            account.set_data_from_slice(program_bytes);

            self.accounts.add_account_no_checks(program_id, account);

            program_len
        } else {
            return Err(HPSVMError::InvalidLoader { program_id, loader_id: *loader_id });
        };

        let mut loaded_program = solana_bpf_loader_program::load_program_from_bytes(
            None,
            &mut LoadProgramMetrics::default(),
            program_bytes,
            loader_id,
            program_size,
            current_slot,
            self.accounts.runtime_environments().program_runtime_v1.clone(),
            PREVERIFIED,
        )
        .map_err(HPSVMError::from)?;
        loaded_program.effective_slot = current_slot;

        self.accounts.replenish_program_cache(program_id, Arc::new(loaded_program));

        Ok(())
    }

    /// Adds an SBF program to the test environment.
    ///
    /// Uses `BPFLoaderUpgradeable` by default for the loader.
    pub fn add_program(
        &mut self,
        program_id: impl Into<Address>,
        program_bytes: &[u8],
    ) -> Result<(), HPSVMError> {
        self.add_program_internal::<false>(
            program_id,
            program_bytes,
            &bpf_loader_upgradeable::id(),
        )?;
        self.invalidate_execution_outcomes();
        Ok(())
    }

    /// Adds an SBF program with a specific loader to match mainnet CU behavior.
    ///
    /// Use `bpf_loader::id()` for BPFLoader2, `bpf_loader_deprecated::id()` for BPFLoader1,
    /// or `bpf_loader_upgradeable::id()` for the upgradeable loader.
    pub fn add_program_with_loader(
        &mut self,
        program_id: impl Into<Address>,
        program_bytes: &[u8],
        loader_id: Address,
    ) -> Result<(), HPSVMError> {
        self.add_program_internal::<false>(program_id, program_bytes, &loader_id)?;
        self.invalidate_execution_outcomes();
        Ok(())
    }

    /// Adds an SBF program that is known-good and already verified.
    pub(crate) fn add_program_preverified(
        &mut self,
        program_id: impl Into<Address>,
        program_bytes: &[u8],
        loader_id: &Address,
    ) -> Result<(), HPSVMError> {
        self.add_program_internal::<true>(program_id, program_bytes, loader_id)
    }

    fn create_transaction_context(
        &self,
        compute_budget: ComputeBudget,
        accounts: Vec<(Address, AccountSharedData)>,
    ) -> TransactionContext<'_> {
        TransactionContext::new(
            accounts,
            self.get_sysvar(),
            compute_budget.max_instruction_stack_depth,
            compute_budget.max_instruction_trace_length,
        )
    }

    fn sanitize_transaction_no_verify_inner(
        &self,
        tx: VersionedTransaction,
    ) -> Result<SanitizedTransaction, TransactionError> {
        let res = SanitizedTransaction::try_create(
            tx,
            MessageHash::Compute,
            Some(false),
            &self.accounts,
            &self.reserved_account_keys.active,
        );
        res.inspect_err(|_| {
            log::error!("Transaction sanitization failed");
        })
    }

    fn sanitize_transaction_no_verify(
        &self,
        tx: VersionedTransaction,
    ) -> Result<SanitizedTransaction, ExecutionResult> {
        self.sanitize_transaction_no_verify_inner(tx)
            .map_err(|err| ExecutionResult { tx_result: Err(err), ..Default::default() })
    }

    fn sanitize_transaction(
        &self,
        tx: VersionedTransaction,
    ) -> Result<SanitizedTransaction, ExecutionResult> {
        self.sanitize_transaction_inner(tx)
            .map_err(|err| ExecutionResult { tx_result: Err(err), ..Default::default() })
    }

    fn sanitize_transaction_inner(
        &self,
        tx: VersionedTransaction,
    ) -> Result<SanitizedTransaction, TransactionError> {
        let tx = self.sanitize_transaction_no_verify_inner(tx)?;

        tx.verify()?;
        SanitizedTransaction::validate_account_locks(
            tx.message(),
            get_transaction_account_lock_limit(self),
        )?;

        Ok(tx)
    }

    fn process_transaction<'a, 'b>(
        &'a self,
        tx: &'b SanitizedTransaction,
        compute_budget_limits: ComputeBudgetLimits,
        log_collector: Rc<RefCell<LogCollector>>,
    ) -> (Result<(), TransactionError>, u64, Option<TransactionContext<'b>>, u64, Option<Address>)
    where
        'a: 'b,
    {
        let compute_budget = self.runtime_env.compute_budget.unwrap_or_else(|| ComputeBudget {
            compute_unit_limit: u64::from(compute_budget_limits.compute_unit_limit),
            heap_size: compute_budget_limits.updated_heap_bytes,
            ..ComputeBudget::new_with_defaults(
                self.cfg.feature_set.is_active(&raise_cpi_nesting_limit_to_8::ID),
                self.cfg.feature_set.is_active(&increase_cpi_account_info_limit::ID),
            )
        });
        let rent = self
            .accounts
            .sysvar_cache()
            .get_rent()
            .expect("rent sysvar should always be available");
        let message = tx.message();
        let blockhash = message.recent_blockhash();
        // reload program cache
        let mut program_cache_for_tx_batch = self.accounts.cloned_programs_cache();
        let mut accumulated_consume_units = 0;
        let account_keys = message.account_keys();
        let prioritization_fee = compute_budget_limits.get_prioritization_fee();
        let fee = solana_fee::calculate_fee(
            message,
            false,
            self.cfg.fee_structure.lamports_per_signature,
            prioritization_fee,
            FeeFeatures::from(&self.cfg.feature_set),
        );
        let mut validated_fee_payer = false;
        let mut payer_key = None;
        let maybe_accounts = account_keys
            .iter()
            .enumerate()
            .map(|(i, key)| {
                let account = if solana_sdk_ids::sysvar::instructions::check_id(key) {
                    construct_instructions_account(message)
                } else {
                    let is_instruction_account = message.is_instruction_account(i);
                    let mut account = if !is_instruction_account &&
                        !message.is_writable(i) &&
                        self.accounts.has_program_cache_entry(key)
                    {
                        // Optimization to skip loading of accounts which are only used as
                        // programs in top-level instructions and not passed as instruction
                        // accounts.
                        self.accounts
                            .get_account(key)
                            .expect("account should exist during processing")
                    } else {
                        self.accounts.get_account(key).unwrap_or_else(|| {
                            let mut default_account = AccountSharedData::default();
                            default_account.set_rent_epoch(0);
                            default_account
                        })
                    };

                    if !validated_fee_payer && (!message.is_invoked(i) || is_instruction_account) {
                        validate_fee_payer(key, &mut account, i as IndexOfAccount, &rent, fee)?;
                        validated_fee_payer = true;
                        payer_key = Some(*key);
                    }
                    account
                };
                Ok((*key, account))
            })
            .collect::<solana_transaction_error::TransactionResult<Vec<_>>>();
        let mut accounts = match maybe_accounts {
            Ok(accs) => accs,
            Err(e) => {
                return (Err(e), accumulated_consume_units, None, fee, payer_key);
            }
        };
        if !validated_fee_payer {
            error!("Failed to validate fee payer");
            return (
                Err(TransactionError::AccountNotFound),
                accumulated_consume_units,
                None,
                fee,
                payer_key,
            );
        }
        let builtins_start_index = accounts.len();
        let maybe_program_indices = tx
            .message()
            .instructions()
            .iter()
            .map(|c| {
                let program_index = c.program_id_index as usize;
                // This may never error, because the transaction is sanitized
                let (program_id, program_account) =
                    accounts.get(program_index).expect("program account should exist");
                if native_loader::check_id(program_id) {
                    return Ok(program_index as IndexOfAccount);
                }
                if !program_account.executable() {
                    error!("Program account {program_id} is not executable.");
                    return Err(TransactionError::InvalidProgramForExecution);
                }

                let owner_id = program_account.owner();
                if native_loader::check_id(owner_id) {
                    return Ok(program_index as IndexOfAccount);
                }

                if !accounts
                    .get(builtins_start_index..)
                    .ok_or(TransactionError::ProgramAccountNotFound)?
                    .iter()
                    .any(|(key, _)| key == owner_id)
                {
                    let owner_account =
                        self.accounts.get_account(owner_id).expect("owner account should exist");
                    if !native_loader::check_id(owner_account.owner()) {
                        error!(
                            "Owner account {owner_id} is not owned by the native loader program."
                        );
                        return Err(TransactionError::InvalidProgramForExecution);
                    }
                    if !owner_account.executable() {
                        error!("Owner account {owner_id} is not executable");
                        return Err(TransactionError::InvalidProgramForExecution);
                    }
                    // Add program_id to the stuff
                    accounts.push((*owner_id, owner_account));
                }
                Ok(program_index as IndexOfAccount)
            })
            .collect::<Result<Vec<u16>, TransactionError>>();

        match maybe_program_indices {
            Ok(program_indices) => {
                let mut context = self.create_transaction_context(compute_budget, accounts);

                // Check rent before creating invoke context
                if let Err(err) = self.check_accounts_rent(tx, &context, &rent) {
                    return (Err(err), accumulated_consume_units, None, fee, None);
                }

                let feature_set = self.cfg.feature_set.runtime_features();
                let mut invoke_context = InvokeContext::new(
                    &mut context,
                    &mut program_cache_for_tx_batch,
                    EnvironmentConfig::new(
                        *blockhash,
                        self.cfg.fee_structure.lamports_per_signature,
                        self,
                        &feature_set,
                        self.accounts.runtime_environments(),
                        self.accounts.runtime_environments(),
                        self.accounts.sysvar_cache(),
                    ),
                    Some(log_collector),
                    compute_budget.to_budget(),
                    compute_budget.to_cost(),
                );

                #[cfg(feature = "invocation-inspect-callback")]
                self.invocation_inspect_callback.before_invocation(
                    self,
                    tx,
                    &program_indices,
                    &invoke_context,
                );

                self.on_transaction_start(tx);

                let tx_result = process_message(
                    self,
                    message,
                    &program_indices,
                    &mut invoke_context,
                    &mut ExecuteTimings::default(),
                    &mut accumulated_consume_units,
                );

                self.on_transaction_end(&tx_result);

                #[cfg(feature = "invocation-inspect-callback")]
                self.invocation_inspect_callback.after_invocation(
                    self,
                    &invoke_context,
                    self.enable_register_tracing,
                );

                (tx_result, accumulated_consume_units, Some(context), fee, payer_key)
            }
            Err(e) => (Err(e), accumulated_consume_units, None, fee, payer_key),
        }
    }

    fn check_accounts_rent(
        &self,
        tx: &SanitizedTransaction,
        context: &TransactionContext<'_>,
        rent: &Rent,
    ) -> Result<(), TransactionError> {
        let message = tx.message();
        for index in 0..message.account_keys().len() {
            if message.is_writable(index) {
                let account = context
                    .accounts()
                    .try_borrow(index as IndexOfAccount)
                    .map_err(|err| TransactionError::InstructionError(index as u8, err))?;

                let pubkey = context
                    .get_key_of_account_at_index(index as IndexOfAccount)
                    .map_err(|err| TransactionError::InstructionError(index as u8, err))?;

                let post_rent_state =
                    get_account_rent_state(rent, account.lamports(), account.data().len());
                let pre_rent_state =
                    self.accounts.get_account(pubkey).map_or(RentState::Uninitialized, |acc| {
                        get_account_rent_state(rent, acc.lamports(), acc.data().len())
                    });

                check_rent_state_with_account(
                    &pre_rent_state,
                    &post_rent_state,
                    pubkey,
                    index as IndexOfAccount,
                )?;
            }
        }
        Ok(())
    }

    fn execute_transaction_no_verify(
        &mut self,
        tx: VersionedTransaction,
        log_collector: Rc<RefCell<LogCollector>>,
    ) -> ExecutionResult {
        map_sanitize_result(self.sanitize_transaction_no_verify(tx), |s_tx| {
            self.execute_sanitized_transaction(&s_tx, log_collector)
        })
    }

    fn execute_transaction(
        &mut self,
        tx: VersionedTransaction,
        log_collector: Rc<RefCell<LogCollector>>,
    ) -> ExecutionResult {
        map_sanitize_result(self.sanitize_transaction(tx), |s_tx| {
            self.execute_sanitized_transaction(&s_tx, log_collector)
        })
    }

    fn execute_sanitized_transaction(
        &mut self,
        sanitized_tx: &SanitizedTransaction,
        log_collector: Rc<RefCell<LogCollector>>,
    ) -> ExecutionResult {
        let CheckAndProcessTransactionSuccess {
            core: CheckAndProcessTransactionSuccessCore { result, compute_units_consumed, context },
            fee,
            payer_key,
        } = match self.check_and_process_transaction(sanitized_tx, log_collector) {
            Ok(value) => value,
            Err(value) => return value,
        };
        if let Some(ctx) = context {
            execution_result_if_context(
                sanitized_tx,
                ctx,
                result,
                compute_units_consumed,
                fee,
                payer_key,
            )
        } else {
            ExecutionResult { tx_result: result, compute_units_consumed, fee, ..Default::default() }
        }
    }

    fn execute_sanitized_transaction_readonly(
        &self,
        sanitized_tx: &SanitizedTransaction,
        log_collector: Rc<RefCell<LogCollector>>,
    ) -> ExecutionResult {
        let CheckAndProcessTransactionSuccess {
            core: CheckAndProcessTransactionSuccessCore { result, compute_units_consumed, context },
            fee,
            payer_key,
        } = match self.check_and_process_transaction(sanitized_tx, log_collector) {
            Ok(value) => value,
            Err(value) => return value,
        };
        if let Some(ctx) = context {
            execution_result_if_context(
                sanitized_tx,
                ctx,
                result,
                compute_units_consumed,
                fee,
                payer_key,
            )
        } else {
            ExecutionResult { tx_result: result, compute_units_consumed, fee, ..Default::default() }
        }
    }

    fn check_and_process_transaction<'a, 'b>(
        &'a self,
        sanitized_tx: &'b SanitizedTransaction,
        log_collector: Rc<RefCell<LogCollector>>,
    ) -> Result<CheckAndProcessTransactionSuccess<'b>, ExecutionResult>
    where
        'a: 'b,
    {
        self.maybe_blockhash_check(sanitized_tx)?;
        let compute_budget_limits = get_compute_budget_limits(sanitized_tx, &self.cfg.feature_set)?;
        self.maybe_history_check(sanitized_tx)?;
        let (result, compute_units_consumed, context, fee, payer_key) =
            self.process_transaction(sanitized_tx, compute_budget_limits, log_collector);
        Ok(CheckAndProcessTransactionSuccess {
            core: {
                CheckAndProcessTransactionSuccessCore { result, compute_units_consumed, context }
            },
            fee,
            payer_key,
        })
    }

    fn maybe_history_check(
        &self,
        sanitized_tx: &SanitizedTransaction,
    ) -> Result<(), ExecutionResult> {
        if self.cfg.sigverify && self.history.check_transaction(sanitized_tx.signature()) {
            return Err(ExecutionResult {
                tx_result: Err(TransactionError::AlreadyProcessed),
                ..Default::default()
            });
        }
        Ok(())
    }

    fn maybe_blockhash_check(
        &self,
        sanitized_tx: &SanitizedTransaction,
    ) -> Result<(), ExecutionResult> {
        if self.cfg.blockhash_check {
            self.check_transaction_age(sanitized_tx)?;
        }
        Ok(())
    }

    fn execute_transaction_readonly(
        &self,
        tx: VersionedTransaction,
        log_collector: Rc<RefCell<LogCollector>>,
    ) -> ExecutionResult {
        map_sanitize_result(self.sanitize_transaction(tx), |s_tx| {
            self.execute_sanitized_transaction_readonly(&s_tx, log_collector)
        })
    }

    fn execute_transaction_no_verify_readonly(
        &self,
        tx: VersionedTransaction,
        log_collector: Rc<RefCell<LogCollector>>,
    ) -> ExecutionResult {
        map_sanitize_result(self.sanitize_transaction_no_verify(tx), |s_tx| {
            self.execute_sanitized_transaction_readonly(&s_tx, log_collector)
        })
    }

    /// Submits a signed transaction and commits its post-state to this VM instance.
    ///
    /// This updates accounts, transaction history, and other in-memory runtime
    /// state, so it intentionally requires `&mut self`. `hpsvm` is optimized for
    /// fast, in-process testing of a single mutable environment rather than
    /// Sealevel-style concurrent scheduling within one instance.
    pub fn send_transaction(&mut self, tx: impl Into<VersionedTransaction>) -> TransactionResult {
        let log_collector = Rc::new(RefCell::new(LogCollector {
            bytes_limit: self.runtime_env.log_bytes_limit,
            ..Default::default()
        }));
        let execution = if self.cfg.sigverify {
            self.execute_transaction(tx.into(), log_collector.clone())
        } else {
            self.execute_transaction_no_verify(tx.into(), log_collector.clone())
        };
        let outcome = execution_into_outcome(self, execution, log_collector, "send_transaction");
        self.commit_transaction(outcome)
    }

    /// Executes a signed transaction without committing its post-state.
    ///
    /// The returned [`ExecutionOutcome`] is bound to this VM instance and its
    /// current state version. Commit it back to the same [`HPSVM`] before any
    /// intervening state or config mutation. Otherwise
    /// [`HPSVM::commit_transaction`] returns `ResanitizationNeeded`.
    #[must_use = "call HPSVM::commit_transaction to apply the returned execution outcome"]
    pub fn transact(&self, tx: impl Into<VersionedTransaction>) -> ExecutionOutcome {
        self.transact_inner(tx.into())
    }

    /// Commits a previously transacted execution outcome to this VM instance.
    ///
    /// Outcomes are valid only for the VM instance and state version that
    /// produced them. If this VM mutated after [`HPSVM::transact`] created the
    /// outcome, or if the outcome came from a different VM instance, this
    /// returns `ResanitizationNeeded`.
    pub fn commit_transaction(&mut self, outcome: ExecutionOutcome) -> TransactionResult {
        commit_execution_outcome(self, outcome)
    }

    /// Plans a conflict-aware transaction batch without committing any state.
    ///
    /// The returned stages contain indexes into the original input order. Stages
    /// are built greedily from account read/write conflicts and form the basis
    /// for higher-level batch schedulers.
    pub fn plan_transaction_batch<T>(
        &self,
        txs: impl IntoIterator<Item = T>,
    ) -> Result<TransactionBatchPlan, TransactionBatchError>
    where
        T: Into<VersionedTransaction>,
    {
        let transactions = txs.into_iter().map(Into::into).collect::<Vec<_>>();
        batch::plan_transaction_batch(self, &transactions)
    }

    /// Submits a batch of transactions and returns a conflict-aware schedule.
    ///
    /// Execution results are returned in the original input order. Transactions
    /// in the same conflict-free stage are executed against cloned snapshots in
    /// parallel, then their disjoint account deltas are merged back into this VM
    /// before the next stage begins.
    pub fn send_transaction_batch<T>(
        &mut self,
        txs: impl IntoIterator<Item = T>,
    ) -> Result<TransactionBatchExecutionResult, TransactionBatchError>
    where
        T: Into<VersionedTransaction>,
    {
        let transactions = txs.into_iter().map(Into::into).collect::<Vec<_>>();
        batch::send_transaction_batch(self, transactions)
    }

    /// Simulates a transaction without committing post-state.
    pub fn simulate_transaction(
        &self,
        tx: impl Into<VersionedTransaction>,
    ) -> Result<SimulatedTransactionInfo, FailedTransactionMetadata> {
        let ExecutionOutcome { meta, post_accounts, status, .. } = self.transact(tx);
        if let Err(tx_err) = status {
            Err(FailedTransactionMetadata { err: tx_err, meta })
        } else {
            Ok(SimulatedTransactionInfo { meta, post_accounts })
        }
    }

    /// Expires the current blockhash.
    pub fn expire_blockhash(&mut self) {
        self.block_env.latest_blockhash =
            create_blockhash(&self.block_env.latest_blockhash.to_bytes());
        #[expect(deprecated)]
        self.set_sysvar_internal(&RecentBlockhashes::from_iter([IterItem(
            0,
            &self.block_env.latest_blockhash,
            self.cfg.fee_structure.lamports_per_signature,
        )]));
        self.invalidate_execution_outcomes();
    }

    /// Warps the clock to the specified slot.
    pub fn warp_to_slot(&mut self, slot: u64) {
        let mut clock = self.get_sysvar::<Clock>();
        clock.slot = slot;
        self.set_sysvar_internal(&clock);
        self.invalidate_execution_outcomes();
    }

    /// Gets the current compute budget.
    pub const fn get_compute_budget(&self) -> Option<ComputeBudget> {
        self.runtime_env.compute_budget
    }

    #[expect(missing_docs)]
    pub const fn get_sigverify(&self) -> bool {
        self.cfg.sigverify
    }

    #[cfg(feature = "internal-test")]
    pub fn get_feature_set(&self) -> Arc<FeatureSet> {
        self.cfg.feature_set.clone().into()
    }

    fn check_transaction_age(&self, tx: &SanitizedTransaction) -> Result<(), ExecutionResult> {
        self.check_transaction_age_inner(tx)
            .map_err(|e| ExecutionResult { tx_result: Err(e), ..Default::default() })
    }

    fn check_transaction_age_inner(
        &self,
        tx: &SanitizedTransaction,
    ) -> solana_transaction_error::TransactionResult<()> {
        let recent_blockhash = tx.message().recent_blockhash();
        if recent_blockhash == &self.block_env.latest_blockhash ||
            self.check_transaction_for_nonce(
                tx,
                &DurableNonce::from_blockhash(&self.block_env.latest_blockhash),
            )
        {
            Ok(())
        } else {
            log::error!(
                "Blockhash {} not found. Expected blockhash {}",
                recent_blockhash,
                self.block_env.latest_blockhash
            );
            Err(TransactionError::BlockhashNotFound)
        }
    }

    fn check_message_for_nonce(&self, message: &SanitizedMessage) -> bool {
        message
            .get_durable_nonce()
            .and_then(|nonce_address| self.accounts.get_account(nonce_address))
            .and_then(|nonce_account| {
                solana_nonce_account::verify_nonce_account(
                    &nonce_account,
                    message.recent_blockhash(),
                )
            })
            .is_some_and(|nonce_data| {
                message
                    .get_ix_signers(NONCED_TX_MARKER_IX_INDEX as usize)
                    .any(|signer| signer == &nonce_data.authority)
            })
    }

    fn check_transaction_for_nonce(
        &self,
        tx: &SanitizedTransaction,
        next_durable_nonce: &DurableNonce,
    ) -> bool {
        let nonce_is_advanceable = tx.message().recent_blockhash() != next_durable_nonce.as_hash();
        nonce_is_advanceable && self.check_message_for_nonce(tx.message())
    }

    #[cfg(feature = "invocation-inspect-callback")]
    pub fn set_invocation_inspect_callback<C: InvocationInspectCallback + 'static>(
        &mut self,
        callback: C,
    ) {
        self.invocation_inspect_callback = Arc::new(callback);
        self.invalidate_execution_outcomes();
    }

    /// Registers a custom syscall in both program runtime environments (v1 and v2).
    ///
    /// This can be called on a freshly constructed [`HPSVM::new()`] instance or on
    /// an existing environment after programs have already been loaded. The runtime
    /// environments are refreshed and cached programs are rebuilt so subsequent
    /// executions see the new syscall.
    ///
    /// Returns an error if runtime refresh, syscall registration, or program cache
    /// rebuilding fails.
    pub fn with_custom_syscall(
        mut self,
        name: &str,
        syscall: BuiltinFunction<InvokeContext<'static, 'static>>,
    ) -> Result<Self, HPSVMError> {
        self.runtime_registry.register_custom_syscall(CustomSyscallRegistration {
            name: name.to_owned(),
            function: syscall,
        });

        self.try_refresh_runtime_environments()?;
        self.accounts.rebuild_program_cache().map_err(HPSVMError::from)?;
        self.invalidate_execution_outcomes();

        Ok(self)
    }
}

struct CheckAndProcessTransactionSuccessCore<'ix_data> {
    result: Result<(), TransactionError>,
    compute_units_consumed: u64,
    context: Option<TransactionContext<'ix_data>>,
}

struct CheckAndProcessTransactionSuccess<'ix_data> {
    core: CheckAndProcessTransactionSuccessCore<'ix_data>,
    fee: u64,
    payer_key: Option<Address>,
}

fn execution_result_if_context(
    sanitized_tx: &SanitizedTransaction,
    ctx: TransactionContext<'_>,
    result: Result<(), TransactionError>,
    compute_units_consumed: u64,
    fee: u64,
    fee_payer: Option<Address>,
) -> ExecutionResult {
    let (signature, return_data, inner_instructions, post_accounts) =
        execute_tx_helper(sanitized_tx, ctx);
    let fee_payer = fee_payer.filter(|_| result.is_err());
    ExecutionResult {
        tx_result: result,
        signature,
        post_accounts,
        inner_instructions,
        compute_units_consumed,
        return_data,
        included: true,
        fee,
        fee_payer,
    }
}

fn execution_into_outcome(
    vm: &HPSVM,
    execution: ExecutionResult,
    log_collector: Rc<RefCell<LogCollector>>,
    method_name: &str,
) -> ExecutionOutcome {
    let ExecutionResult {
        post_accounts,
        tx_result,
        signature,
        compute_units_consumed,
        inner_instructions,
        return_data,
        included,
        fee,
        fee_payer,
    } = execution;
    let Ok(logs) = Rc::try_unwrap(log_collector).map(|collector| collector.into_inner().messages)
    else {
        unreachable!("Log collector should not be used after {method_name} returns")
    };

    ExecutionOutcome {
        meta: TransactionMetadata {
            signature,
            logs,
            inner_instructions,
            compute_units_consumed,
            return_data,
            fee,
        },
        post_accounts,
        status: tx_result,
        included,
        origin_vm_instance_id: vm.instance_id,
        origin_state_version: vm.state_version,
        fee_payer,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CommitDelta {
    post_accounts: Vec<(Address, AccountSharedData)>,
    history_entry: Option<(Signature, TransactionResult)>,
}

impl CommitDelta {
    pub(crate) const fn new(
        post_accounts: Vec<(Address, AccountSharedData)>,
        history_entry: Option<(Signature, TransactionResult)>,
    ) -> Self {
        Self { post_accounts, history_entry }
    }

    pub(crate) const fn mutates_state(&self) -> bool {
        !self.post_accounts.is_empty() || self.history_entry.is_some()
    }
}

pub(crate) fn apply_commit_delta(
    accounts: &mut AccountsDb,
    history: &mut TransactionHistory,
    delta: CommitDelta,
) -> Result<(), HPSVMError> {
    accounts.sync_accounts(delta.post_accounts)?;
    if let Some((signature, entry)) = delta.history_entry {
        history.add_new_transaction(signature, entry);
    }
    Ok(())
}

pub(crate) fn outcome_into_result_and_delta(
    outcome: ExecutionOutcome,
) -> (TransactionResult, CommitDelta) {
    let ExecutionOutcome { meta, post_accounts, status, included, .. } = outcome;
    let result = match status {
        Ok(()) => TransactionResult::Ok(meta.clone()),
        Err(err) => TransactionResult::Err(FailedTransactionMetadata { err, meta: meta.clone() }),
    };
    let delta = if included {
        CommitDelta::new(post_accounts, Some((meta.signature, result.clone())))
    } else {
        CommitDelta::new(Vec::new(), None)
    };
    (result, delta)
}

fn commit_execution_outcome(vm: &mut HPSVM, outcome: ExecutionOutcome) -> TransactionResult {
    let origin_vm_instance_id = outcome.origin_vm_instance_id;
    let origin_state_version = outcome.origin_state_version;

    if origin_vm_instance_id != vm.instance_id || origin_state_version != vm.state_version {
        return TransactionResult::Err(FailedTransactionMetadata {
            err: TransactionError::ResanitizationNeeded,
            meta: outcome.meta,
        });
    }

    let (result, delta) = outcome_into_result_and_delta(outcome);
    let mutates_state = delta.mutates_state();

    apply_commit_delta(&mut vm.accounts, &mut vm.history, delta)
        .expect("It shouldn't be possible to write invalid sysvars in send_transaction.");
    if mutates_state {
        vm.invalidate_execution_outcomes();
    }
    result
}

fn execute_tx_helper(
    sanitized_tx: &SanitizedTransaction,
    ctx: TransactionContext<'_>,
) -> (
    Signature,
    solana_transaction_context::TransactionReturnData,
    InnerInstructionsList,
    Vec<(Address, AccountSharedData)>,
) {
    let signature = sanitized_tx.signature().to_owned();
    let inner_instructions = inner_instructions_list_from_instruction_trace(&ctx);
    let ExecutionRecord {
        accounts,
        return_data,
        touched_account_count: _,
        accounts_resize_delta: _,
    } = ctx.into();
    let msg = sanitized_tx.message();
    let post_accounts = accounts
        .into_iter()
        .enumerate()
        .filter_map(|(idx, pair)| msg.is_writable(idx).then_some(pair))
        .collect();
    (signature, return_data, inner_instructions, post_accounts)
}

fn get_compute_budget_limits(
    sanitized_tx: &SanitizedTransaction,
    feature_set: &FeatureSet,
) -> Result<ComputeBudgetLimits, ExecutionResult> {
    process_compute_budget_instructions(
        SVMMessage::program_instructions_iter(sanitized_tx),
        feature_set,
    )
    .map_err(|e| ExecutionResult { tx_result: Err(e), ..Default::default() })
}

/// Get the max number of accounts that a transaction may lock in this block
fn get_transaction_account_lock_limit(svm: &HPSVM) -> usize {
    if svm.cfg.feature_set.is_active(&agave_feature_set::increase_tx_account_lock_limit::id()) {
        MAX_TX_ACCOUNT_LOCKS
    } else {
        64
    }
}

/// Lighter version of the one in the solana-svm crate.
///
/// Check whether the payer_account is capable of paying the fee. The
/// side effect is to subtract the fee amount from the payer_account
/// balance of lamports. If the payer_account is not able to pay the
/// fee a specific error is returned.
fn validate_fee_payer(
    payer_address: &Address,
    payer_account: &mut AccountSharedData,
    payer_index: IndexOfAccount,
    rent: &Rent,
    fee: u64,
) -> solana_transaction_error::TransactionResult<()> {
    if payer_account.lamports() == 0 {
        error!("Payer account {payer_address} not found.");
        return Err(TransactionError::AccountNotFound);
    }
    let system_account_kind = get_system_account_kind(payer_account).ok_or_else(|| {
        error!("Payer account {payer_address} is not a system account");
        TransactionError::InvalidAccountForFee
    })?;
    let min_balance = match system_account_kind {
        SystemAccountKind::System => 0,
        SystemAccountKind::Nonce => {
            // Should we ever allow a fees charge to zero a nonce account's
            // balance. The state MUST be set to uninitialized in that case
            rent.minimum_balance(solana_nonce::state::State::size())
        }
    };

    let payer_lamports = payer_account.lamports();

    payer_lamports.checked_sub(min_balance).and_then(|v| v.checked_sub(fee)).ok_or_else(|| {
        error!(
            "Payer account {payer_address} has insufficient lamports for fee. Payer lamports: \
                {payer_lamports} min_balance: {min_balance} fee: {fee}"
        );
        TransactionError::InsufficientFundsForFee
    })?;

    let payer_len = payer_account.data().len();
    let payer_pre_rent_state = get_account_rent_state(rent, payer_account.lamports(), payer_len);
    // we already checked above if we have sufficient balance so this should never error.
    payer_account.checked_sub_lamports(fee).expect("fee should not exceed account balance");

    let payer_post_rent_state = get_account_rent_state(rent, payer_account.lamports(), payer_len);
    check_rent_state_with_account(
        &payer_pre_rent_state,
        &payer_post_rent_state,
        payer_address,
        payer_index,
    )
}

fn map_sanitize_result<F>(
    res: Result<SanitizedTransaction, ExecutionResult>,
    op: F,
) -> ExecutionResult
where
    F: FnOnce(SanitizedTransaction) -> ExecutionResult,
{
    match res {
        Ok(s_tx) => op(s_tx),
        Err(e) => e,
    }
}

impl HPSVM {
    fn transact_inner(&self, tx: VersionedTransaction) -> ExecutionOutcome {
        let log_collector = Rc::new(RefCell::new(LogCollector {
            bytes_limit: self.runtime_env.log_bytes_limit,
            ..Default::default()
        }));
        let execution = if self.cfg.sigverify {
            self.execute_transaction_readonly(tx, log_collector.clone())
        } else {
            self.execute_transaction_no_verify_readonly(tx, log_collector.clone())
        };
        execution_into_outcome(self, execution, log_collector, "transact")
    }
}

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

#[cfg(test)]
mod tests {
    use solana_instruction::{Instruction, account_meta::AccountMeta};
    use solana_message::{Message, VersionedMessage};

    use super::*;

    #[test]
    fn sysvar_accounts_are_demoted_to_readonly() {
        let payer = Keypair::new();
        let svm = HPSVM::new();
        let rent_key = solana_sdk_ids::sysvar::rent::id();
        let ix = Instruction {
            program_id: solana_sdk_ids::system_program::id(),
            accounts: vec![AccountMeta { pubkey: rent_key, is_signer: false, is_writable: true }],
            data: vec![],
        };
        let message = Message::new(&[ix], Some(&payer.pubkey()));
        let tx =
            VersionedTransaction::try_new(VersionedMessage::Legacy(message), &[&payer]).unwrap();
        let sanitized = svm.sanitize_transaction_no_verify_inner(tx).unwrap();

        assert!(!sanitized.message().is_writable(1));
    }
}
