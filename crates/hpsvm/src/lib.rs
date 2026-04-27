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
//!     svm.set_sysvar::<Clock>(&initial_clock);
//!     // This will fail because the program expects early 1970 timestamp
//!     let _err = svm.send_transaction(tx.clone()).unwrap_err();
//!     // So let's turn back time
//!     let mut clock = svm.get_sysvar::<Clock>();
//!     clock.unix_timestamp = 50;
//!     svm.set_sysvar::<Clock>(&clock);
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

use std::{cell::RefCell, path::Path, rc::Rc, sync::Arc};

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
    error::HPSVMError,
    history::TransactionHistory,
    message_processor::process_message,
    programs::{DEFAULT_PROGRAM_IDS, load_default_programs},
    types::{ExecutionResult, FailedTransactionMetadata, TransactionMetadata, TransactionResult},
    utils::{
        create_blockhash,
        rent::{RentState, check_rent_state_with_account, get_account_rent_state},
    },
};

#[derive(Clone)]
struct CustomSyscallRegistration {
    name: String,
    function: BuiltinFunction<InvokeContext<'static, 'static>>,
}

#[expect(missing_docs)]
pub mod error;
#[expect(missing_docs)]
pub mod types;

mod accounts_db;
mod callback;
mod format_logs;
mod history;
mod message_processor;
#[cfg(feature = "precompiles")]
mod precompiles;
mod programs;
#[cfg(feature = "register-tracing")]
pub mod register_tracing;
mod utils;

#[expect(missing_docs)]
#[derive(Clone)]
pub struct HPSVM {
    accounts: AccountsDb,
    airdrop_kp: [u8; 64],
    builtins_loaded: bool,
    custom_syscalls: Vec<CustomSyscallRegistration>,
    default_programs_loaded: bool,
    feature_set: FeatureSet,
    feature_accounts_loaded: bool,
    reserved_account_keys: ReservedAccountKeys,
    latest_blockhash: Hash,
    history: TransactionHistory,
    compute_budget: Option<ComputeBudget>,
    sigverify: bool,
    blockhash_check: bool,
    fee_structure: FeeStructure,
    log_bytes_limit: Option<usize>,
    #[cfg(feature = "precompiles")]
    precompiles_loaded: bool,
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
            .field("feature_set", &self.feature_set)
            .field("feature_accounts_loaded", &self.feature_accounts_loaded)
            .field("latest_blockhash", &self.latest_blockhash)
            .field("sigverify", &self.sigverify)
            .field("blockhash_check", &self.blockhash_check)
            .field("sysvars_loaded", &self.sysvars_loaded);
        #[cfg(feature = "precompiles")]
        debug.field("precompiles_loaded", &self.precompiles_loaded);
        #[cfg(feature = "invocation-inspect-callback")]
        debug.field("enable_register_tracing", &self.enable_register_tracing);
        debug.finish_non_exhaustive()
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
    fn new_inner(_enable_register_tracing: bool) -> Self {
        let feature_set = FeatureSet::default();

        Self {
            accounts: Default::default(),
            airdrop_kp: Keypair::new().to_bytes(),
            builtins_loaded: false,
            custom_syscalls: Vec::new(),
            default_programs_loaded: false,
            reserved_account_keys: Self::reserved_account_keys_for_feature_set(&feature_set),
            feature_set,
            feature_accounts_loaded: false,
            latest_blockhash: create_blockhash(b"genesis"),
            history: TransactionHistory::new(),
            compute_budget: None,
            sigverify: false,
            blockhash_check: false,
            fee_structure: FeeStructure::default(),
            log_bytes_limit: Some(10_000),
            #[cfg(feature = "precompiles")]
            precompiles_loaded: false,
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
            self.accounts.inner.remove(feature_id);
        });
    }

    fn clear_builtin_accounts(&mut self) {
        for builtin in BUILTINS {
            self.accounts.inner.remove(&builtin.program_id);
        }
    }

    fn clear_default_programs(&mut self) {
        for program_id in &DEFAULT_PROGRAM_IDS {
            if self
                .accounts
                .get_account_ref(program_id)
                .is_some_and(|account| account.owner() == &bpf_loader_upgradeable::id())
            {
                self.accounts.inner.remove(&get_program_data_address(program_id));
            }
            self.accounts.inner.remove(program_id);
        }
    }

    #[cfg(feature = "precompiles")]
    fn clear_precompile_accounts(&mut self) {
        agave_precompiles::get_precompiles().iter().for_each(|precompile| {
            self.accounts.inner.remove(&precompile.program_id);
        });
    }

    fn refresh_runtime_environments(&mut self) {
        #[cfg(feature = "register-tracing")]
        let enable_register_tracing = self.enable_register_tracing;
        #[cfg(not(feature = "register-tracing"))]
        let enable_register_tracing = false;

        let compute_budget = self.compute_budget.unwrap_or_else(|| {
            ComputeBudget::new_with_defaults(
                self.feature_set.is_active(&raise_cpi_nesting_limit_to_8::ID),
                self.feature_set.is_active(&increase_cpi_account_info_limit::ID),
            )
        });
        let mut program_runtime_v1 = create_program_runtime_environment_v1(
            &self.feature_set.runtime_features(),
            &compute_budget.to_budget(),
            false,
            enable_register_tracing,
        )
        .expect("failed to create program runtime environment v1");

        let mut program_runtime_v2 = create_program_runtime_environment_v2(
            &compute_budget.to_budget(),
            enable_register_tracing,
        );

        for syscall in &self.custom_syscalls {
            program_runtime_v1.register_function(&syscall.name, syscall.function).unwrap_or_else(
                |e| panic!("failed to register syscall '{}' in runtime_v1: {e}", syscall.name),
            );
            program_runtime_v2.register_function(&syscall.name, syscall.function).unwrap_or_else(
                |e| panic!("failed to register syscall '{}' in runtime_v2: {e}", syscall.name),
            );
        }

        self.accounts.environments.program_runtime_v1 = Arc::new(program_runtime_v1);
        self.accounts.environments.program_runtime_v2 = Arc::new(program_runtime_v2);
    }

    fn reconfigure_materialized_feature_state(&mut self, previous_feature_set: &FeatureSet) {
        if self.feature_accounts_loaded {
            self.clear_feature_accounts(previous_feature_set);
            self.set_feature_accounts();
        }

        if self.default_programs_loaded {
            self.clear_default_programs();
        }

        if self.builtins_loaded {
            self.clear_builtin_accounts();
            self.set_builtins();
        } else if !self.custom_syscalls.is_empty() {
            self.refresh_runtime_environments();
        }

        #[cfg(feature = "precompiles")]
        if self.precompiles_loaded {
            self.clear_precompile_accounts();
            self.set_precompiles();
        }

        if self.default_programs_loaded {
            self.set_default_programs();
        }

        assert!(
            self.accounts.rebuild_program_cache().is_ok(),
            "feature-set reconfiguration produced invalid program cache state"
        );
    }

    #[cfg_attr(feature = "nodejs-internal", qualifiers(pub))]
    const fn set_compute_budget(&mut self, compute_budget: ComputeBudget) {
        self.compute_budget = Some(compute_budget);
    }

    /// Sets the compute budget.
    pub const fn with_compute_budget(mut self, compute_budget: ComputeBudget) -> Self {
        self.set_compute_budget(compute_budget);
        self
    }

    #[cfg_attr(feature = "nodejs-internal", qualifiers(pub))]
    const fn set_sigverify(&mut self, sigverify: bool) {
        self.sigverify = sigverify;
    }

    /// Enables or disables sigverify.
    pub const fn with_sigverify(mut self, sigverify: bool) -> Self {
        self.set_sigverify(sigverify);
        self
    }

    #[cfg_attr(feature = "nodejs-internal", qualifiers(pub))]
    const fn set_blockhash_check(&mut self, check: bool) {
        self.blockhash_check = check;
    }

    /// Enables or disables the blockhash check.
    pub const fn with_blockhash_check(mut self, check: bool) -> Self {
        self.set_blockhash_check(check);
        self
    }

    #[cfg_attr(feature = "nodejs-internal", qualifiers(pub))]
    fn set_sysvars(&mut self) {
        self.sysvars_loaded = true;
        self.set_sysvar(&Clock::default());
        self.set_sysvar(&EpochRewards::default());
        self.set_sysvar(&EpochSchedule::default());
        #[expect(deprecated)]
        let fees = Fees::default();
        self.set_sysvar(&fees);
        self.set_sysvar(&LastRestartSlot::default());
        let latest_blockhash = self.latest_blockhash;
        #[expect(deprecated)]
        self.set_sysvar(&RecentBlockhashes::from_iter([IterItem(
            0,
            &latest_blockhash,
            fees.fee_calculator.lamports_per_signature,
        )]));

        // Rent account differs based off feature gating
        #[expect(deprecated)]
        {
            let mut rent_account = Rent::default();
            if self
                .feature_set
                .is_active(&agave_feature_set::deprecate_rent_exemption_threshold::id())
            {
                rent_account.exemption_threshold = 1.0;
                rent_account.lamports_per_byte_year = solana_rent::DEFAULT_LAMPORTS_PER_BYTE;
            }
            self.set_sysvar(&rent_account);
        }
        self.set_sysvar(&SlotHashes::new(&[(
            self.accounts
                .sysvar_cache
                .get_clock()
                .expect("clock sysvar should always be available")
                .slot,
            latest_blockhash,
        )]));
        self.set_sysvar(&SlotHistory::default());
        self.set_sysvar(&StakeHistory::default());
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
        let previous_feature_set = self.feature_set.clone();
        self.feature_set = feature_set;
        self.reserved_account_keys = Self::reserved_account_keys_for_feature_set(&self.feature_set);
        self.reconfigure_materialized_feature_state(&previous_feature_set);
    }

    #[cfg_attr(feature = "nodejs-internal", qualifiers(pub))]
    fn set_feature_accounts(&mut self) {
        self.feature_accounts_loaded = true;
        for (feature_id, activation_slot) in self.feature_set.active() {
            let feature_account = Feature { activated_at: Some(*activation_slot) };
            let lamports = self.minimum_balance_for_rent_exemption(Feature::size_of());
            let account = feature_gate::create_account(&feature_account, lamports);
            self.accounts.add_account_no_checks(*feature_id, account);
        }
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

    #[cfg_attr(feature = "nodejs-internal", qualifiers(pub))]
    fn set_builtins(&mut self) {
        self.builtins_loaded = true;
        self.refresh_runtime_environments();
        for builtint in BUILTINS {
            if builtint.enable_feature_id.is_none_or(|x| self.feature_set.is_active(&x)) {
                let loaded_program =
                    ProgramCacheEntry::new_builtin(0, builtint.name.len(), builtint.entrypoint);
                self.accounts
                    .programs_cache
                    .replenish(builtint.program_id, Arc::new(loaded_program));
                self.accounts.add_builtin_account(
                    builtint.program_id,
                    crate::utils::create_loadable_account_for_test(builtint.name),
                );
            }
        }
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
    }

    /// Changes the initial lamports in HPSVM's airdrop account.
    pub fn with_lamports(mut self, lamports: u64) -> Self {
        self.set_lamports(lamports);
        self
    }

    #[cfg_attr(feature = "nodejs-internal", qualifiers(pub))]
    fn set_default_programs(&mut self) {
        self.default_programs_loaded = true;
        load_default_programs(self);
    }

    /// Includes the standard SPL programs.
    pub fn with_default_programs(mut self) -> Self {
        self.set_default_programs();
        self
    }

    #[cfg_attr(feature = "nodejs-internal", qualifiers(pub))]
    fn set_transaction_history(&mut self, capacity: usize) {
        self.history.set_capacity(capacity);
    }

    /// Changes the capacity of the transaction history.
    /// Set this to 0 to disable transaction history and allow duplicate transactions.
    pub fn with_transaction_history(mut self, capacity: usize) -> Self {
        self.set_transaction_history(capacity);
        self
    }

    #[cfg_attr(feature = "nodejs-internal", qualifiers(pub))]
    const fn set_log_bytes_limit(&mut self, limit: Option<usize>) {
        self.log_bytes_limit = limit;
    }

    #[expect(missing_docs)]
    pub const fn with_log_bytes_limit(mut self, limit: Option<usize>) -> Self {
        self.set_log_bytes_limit(limit);
        self
    }

    #[cfg_attr(feature = "nodejs-internal", qualifiers(pub))]
    #[cfg(feature = "precompiles")]
    fn set_precompiles(&mut self) {
        self.precompiles_loaded = true;
        load_precompiles(self);
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
        1.max(self.accounts.sysvar_cache.get_rent().unwrap_or_default().minimum_balance(data_len))
    }

    /// Returns all information associated with the account of the provided pubkey.
    pub fn get_account(&self, address: &Address) -> Option<Account> {
        self.accounts.get_account(address).map(Into::into)
    }

    /// Sets all information associated with the account of the provided pubkey.
    pub fn set_account(&mut self, address: Address, data: Account) -> Result<(), HPSVMError> {
        self.accounts.add_account(address, data.into())
    }

    /// **⚠️ ADVANCED USE ONLY ⚠️**
    ///
    /// Returns a reference to the internal accounts database.
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
    /// // Read-only access to accounts database
    /// let accounts_db = svm.accounts_db();
    /// // ... inspect internal state if needed
    /// ```
    pub const fn accounts_db(&self) -> &AccountsDb {
        &self.accounts
    }

    /// Gets the balance of the provided account pubkey.
    pub fn get_balance(&self, address: &Address) -> Option<u64> {
        self.accounts.get_account_ref(address).map(|x| x.lamports())
    }

    /// Gets the latest blockhash.
    pub const fn latest_blockhash(&self) -> Hash {
        self.latest_blockhash
    }

    /// Sets the sysvar to the test environment.
    pub fn set_sysvar<T>(&mut self, sysvar: &T)
    where
        T: Sysvar + SysvarId + SysvarSerialize,
    {
        let mut account = AccountSharedData::new(1, T::size_of(), &solana_sdk_ids::sysvar::id());
        account.serialize_data(sysvar).expect("sysvar serialization should never fail");
        self.accounts.add_account(T::id(), account).expect("failed to add sysvar account");
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

    /// Airdrops the account with the lamports specified.
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
                &self.latest_blockhash,
            )),
            &[payer],
        )
        .expect("failed to create airdrop transaction");

        self.send_transaction(tx)
    }

    /// Adds a builtin program to the test environment.
    pub fn add_builtin(&mut self, program_id: Address, entrypoint: BuiltinFunctionWithContext) {
        let builtin = ProgramCacheEntry::new_builtin(
            self.accounts.sysvar_cache.get_clock().unwrap_or_default().slot,
            1,
            entrypoint,
        );

        self.accounts.programs_cache.replenish(program_id, Arc::new(builtin));

        let mut account = AccountSharedData::new(1, 1, &bpf_loader::id());
        account.set_executable(true);
        self.accounts.add_account_no_checks(program_id, account);
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
        let current_slot = self.accounts.sysvar_cache.get_clock().unwrap_or_default().slot;

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
            return Err(HPSVMError::InvalidLoader(format!("Unsupported loader: {loader_id}")));
        };

        let mut loaded_program = solana_bpf_loader_program::load_program_from_bytes(
            None,
            &mut LoadProgramMetrics::default(),
            program_bytes,
            loader_id,
            program_size,
            current_slot,
            self.accounts.environments.program_runtime_v1.clone(),
            PREVERIFIED,
        )
        .map_err(HPSVMError::from)?;
        loaded_program.effective_slot = current_slot;

        self.accounts.programs_cache.replenish(program_id, Arc::new(loaded_program));

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
        self.add_program_internal::<false>(program_id, program_bytes, &bpf_loader_upgradeable::id())
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
        self.add_program_internal::<false>(program_id, program_bytes, &loader_id)
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
        let compute_budget = self.compute_budget.unwrap_or_else(|| ComputeBudget {
            compute_unit_limit: u64::from(compute_budget_limits.compute_unit_limit),
            heap_size: compute_budget_limits.updated_heap_bytes,
            ..ComputeBudget::new_with_defaults(
                self.feature_set.is_active(&raise_cpi_nesting_limit_to_8::ID),
                self.feature_set.is_active(&increase_cpi_account_info_limit::ID),
            )
        });
        let rent =
            self.accounts.sysvar_cache.get_rent().expect("rent sysvar should always be available");
        let message = tx.message();
        let blockhash = message.recent_blockhash();
        // reload program cache
        let mut program_cache_for_tx_batch = self.accounts.programs_cache.clone();
        let mut accumulated_consume_units = 0;
        let account_keys = message.account_keys();
        let prioritization_fee = compute_budget_limits.get_prioritization_fee();
        let fee = solana_fee::calculate_fee(
            message,
            false,
            self.fee_structure.lamports_per_signature,
            prioritization_fee,
            FeeFeatures::from(&self.feature_set),
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
                        self.accounts.programs_cache.find(key).is_some()
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

                let feature_set = self.feature_set.runtime_features();
                let mut invoke_context = InvokeContext::new(
                    &mut context,
                    &mut program_cache_for_tx_batch,
                    EnvironmentConfig::new(
                        *blockhash,
                        self.fee_structure.lamports_per_signature,
                        self,
                        &feature_set,
                        &self.accounts.environments,
                        &self.accounts.environments,
                        &self.accounts.sysvar_cache,
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

                let tx_result = process_message(
                    message,
                    &program_indices,
                    &mut invoke_context,
                    &mut ExecuteTimings::default(),
                    &mut accumulated_consume_units,
                );

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
                    self.accounts.get_account_ref(pubkey).map_or(RentState::Uninitialized, |acc| {
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
            let mut exec_result =
                execution_result_if_context(sanitized_tx, ctx, result, compute_units_consumed, fee);

            if let Some(payer) = payer_key.filter(|_| exec_result.tx_result.is_err()) {
                exec_result.tx_result =
                    self.accounts.withdraw(&payer, fee).and(exec_result.tx_result);
            }
            exec_result
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
            ..
        } = match self.check_and_process_transaction(sanitized_tx, log_collector) {
            Ok(value) => value,
            Err(value) => return value,
        };
        if let Some(ctx) = context {
            execution_result_if_context(sanitized_tx, ctx, result, compute_units_consumed, fee)
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
        let compute_budget_limits = get_compute_budget_limits(sanitized_tx, &self.feature_set)?;
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
        if self.sigverify && self.history.check_transaction(sanitized_tx.signature()) {
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
        if self.blockhash_check {
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

    /// Submits a signed transaction.
    pub fn send_transaction(&mut self, tx: impl Into<VersionedTransaction>) -> TransactionResult {
        let log_collector =
            LogCollector { bytes_limit: self.log_bytes_limit, ..Default::default() };
        let log_collector = Rc::new(RefCell::new(log_collector));
        let vtx: VersionedTransaction = tx.into();
        let ExecutionResult {
            post_accounts,
            tx_result,
            signature,
            compute_units_consumed,
            inner_instructions,
            return_data,
            included,
            fee,
        } = if self.sigverify {
            self.execute_transaction(vtx, log_collector.clone())
        } else {
            self.execute_transaction_no_verify(vtx, log_collector.clone())
        };
        let Ok(logs) = Rc::try_unwrap(log_collector).map(|lc| lc.into_inner().messages) else {
            unreachable!("Log collector should not be used after send_transaction returns")
        };
        let meta = TransactionMetadata {
            signature,
            logs,
            inner_instructions,
            compute_units_consumed,
            return_data,
            fee,
        };

        if let Err(tx_err) = tx_result {
            let err = TransactionResult::Err(FailedTransactionMetadata { err: tx_err, meta });
            if included {
                self.history.add_new_transaction(signature, err.clone());
            }
            err
        } else {
            self.history.add_new_transaction(signature, Ok(meta.clone()));
            self.accounts
                .sync_accounts(post_accounts)
                .expect("It shouldn't be possible to write invalid sysvars in send_transaction.");

            TransactionResult::Ok(meta)
        }
    }

    /// Simulates a transaction.
    pub fn simulate_transaction(
        &self,
        tx: impl Into<VersionedTransaction>,
    ) -> Result<SimulatedTransactionInfo, FailedTransactionMetadata> {
        let log_collector =
            LogCollector { bytes_limit: self.log_bytes_limit, ..Default::default() };
        let log_collector = Rc::new(RefCell::new(log_collector));
        let ExecutionResult {
            post_accounts,
            tx_result,
            signature,
            compute_units_consumed,
            inner_instructions,
            return_data,
            fee,
            ..
        } = if self.sigverify {
            self.execute_transaction_readonly(tx.into(), log_collector.clone())
        } else {
            self.execute_transaction_no_verify_readonly(tx.into(), log_collector.clone())
        };
        let Ok(logs) = Rc::try_unwrap(log_collector).map(|lc| lc.into_inner().messages) else {
            unreachable!("Log collector should not be used after simulate_transaction returns")
        };
        let meta = TransactionMetadata {
            signature,
            logs,
            inner_instructions,
            compute_units_consumed,
            return_data,
            fee,
        };

        if let Err(tx_err) = tx_result {
            Err(FailedTransactionMetadata { err: tx_err, meta })
        } else {
            Ok(SimulatedTransactionInfo { meta, post_accounts })
        }
    }

    /// Expires the current blockhash.
    pub fn expire_blockhash(&mut self) {
        self.latest_blockhash = create_blockhash(&self.latest_blockhash.to_bytes());
        #[expect(deprecated)]
        self.set_sysvar(&RecentBlockhashes::from_iter([IterItem(
            0,
            &self.latest_blockhash,
            self.fee_structure.lamports_per_signature,
        )]));
    }

    /// Warps the clock to the specified slot.
    pub fn warp_to_slot(&mut self, slot: u64) {
        let mut clock = self.get_sysvar::<Clock>();
        clock.slot = slot;
        self.set_sysvar(&clock);
    }

    /// Gets the current compute budget.
    pub const fn get_compute_budget(&self) -> Option<ComputeBudget> {
        self.compute_budget
    }

    #[expect(missing_docs)]
    pub const fn get_sigverify(&self) -> bool {
        self.sigverify
    }

    #[cfg(feature = "internal-test")]
    pub fn get_feature_set(&self) -> Arc<FeatureSet> {
        self.feature_set.clone().into()
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
        if recent_blockhash == &self.latest_blockhash ||
            self.check_transaction_for_nonce(
                tx,
                &DurableNonce::from_blockhash(&self.latest_blockhash),
            )
        {
            Ok(())
        } else {
            log::error!(
                "Blockhash {} not found. Expected blockhash {}",
                recent_blockhash,
                self.latest_blockhash
            );
            Err(TransactionError::BlockhashNotFound)
        }
    }

    fn check_message_for_nonce(&self, message: &SanitizedMessage) -> bool {
        message
            .get_durable_nonce()
            .and_then(|nonce_address| self.accounts.get_account_ref(nonce_address))
            .and_then(|nonce_account| {
                solana_nonce_account::verify_nonce_account(
                    nonce_account,
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
    }

    /// Registers a custom syscall in both program runtime environments (v1 and v2).
    ///
    /// **Must be called after `with_builtins()`** (which recreates the environments
    /// from scratch) and **before `with_default_programs()`** (which clones the
    /// environment Arcs into program cache entries, preventing further mutation).
    ///
    /// Panics if the runtime environments cannot be mutated or if registration
    /// fails. This is intentional — a misconfigured syscall should fail loudly
    /// rather than silently.
    pub fn with_custom_syscall(
        mut self,
        name: &str,
        syscall: BuiltinFunction<InvokeContext<'static, 'static>>,
    ) -> Self {
        self.custom_syscalls
            .push(CustomSyscallRegistration { name: name.to_owned(), function: syscall });

        self.refresh_runtime_environments();

        assert!(
            self.accounts.rebuild_program_cache().is_ok(),
            "with_custom_syscall: failed to rebuild program cache after runtime refresh"
        );

        self
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
) -> ExecutionResult {
    let (signature, return_data, inner_instructions, post_accounts) =
        execute_tx_helper(sanitized_tx, ctx);
    ExecutionResult {
        tx_result: result,
        signature,
        post_accounts,
        inner_instructions,
        compute_units_consumed,
        return_data,
        included: true,
        fee,
    }
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
    if svm.feature_set.is_active(&agave_feature_set::increase_tx_account_lock_limit::id()) {
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
