# HPSVM Product Layer Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the first reusable product layer around `hpsvm` by shipping serializable execution snapshots, transaction-first fixtures, and a CLI that can inspect, run, and compare fixtures.

**Architecture:** Keep `crates/hpsvm` as the transaction-first execution engine, use `crates/hpsvm-fixture` as the single product-layer crate for snapshots, comparisons, fixture replay, and compute-unit reporting, keep `crates/hpsvm-fixture-fd` as a separate Firedancer adapter, and place the CLI binary crate under `bin/hpsvm-cli`.

**Implementation update, 2026-05-02:** This plan originally split `hpsvm-result`, `hpsvm-fixture`, and `hpsvm-bencher` into separate crates. Current implementation consolidates those layers into `crates/hpsvm-fixture` while preserving the historical task notes below for traceability.

**Tech Stack:** Rust 2024 workspace, `hpsvm`, `serde`, `serde_json`, `serde_yaml`, `clap` 4, `bincode` 1, `thiserror`, `cargo test`, and repo-wide `just` validation commands.

---

**Historical scope note:** The original plan treated `hpsvm-bencher` as a later follow-up. Current implementation folds compute-unit reporting into `hpsvm-fixture` and keeps only `hpsvm-fixture-fd` as a separate adapter crate.

## Original File Structure

### Workspace files

- Modify: `Cargo.toml`
  - Register `hpsvm-result`, `hpsvm-fixture`, and `hpsvm-cli` in `[workspace.dependencies]`
  - Add shared external dependencies used by the new crates: `bincode`, `clap`, `serde_json`, `serde_yaml`

### `crates/hpsvm-result`

- Create: `crates/hpsvm-result/Cargo.toml`
  - Manifest for the reusable snapshot and assertion crate
- Create: `crates/hpsvm-result/src/lib.rs`
  - Curated public facade
- Create: `crates/hpsvm-result/src/config.rs`
  - Runtime behavior for panic vs boolean-return validation
- Create: `crates/hpsvm-result/src/snapshot.rs`
  - Stable snapshot model plus conversions from `hpsvm` runtime outputs
- Create: `crates/hpsvm-result/src/check.rs`
  - Assertion DSL and `ExecutionSnapshot::run_checks`
- Create: `crates/hpsvm-result/src/compare.rs`
  - Comparison DSL and `ExecutionSnapshot::compare_with`
- Create: `crates/hpsvm-result/tests/snapshot.rs`
  - Snapshot conversion tests against live `hpsvm`
- Create: `crates/hpsvm-result/tests/checks.rs`
  - Check DSL tests
- Create: `crates/hpsvm-result/tests/compare.rs`
  - Compare DSL tests

### `crates/hpsvm-fixture`

- Create: `crates/hpsvm-fixture/Cargo.toml`
  - Manifest for transaction-first fixture capture and replay
- Create: `crates/hpsvm-fixture/src/lib.rs`
  - Public facade and top-level `Fixture::load` / `Fixture::save`
- Create: `crates/hpsvm-fixture/src/error.rs`
  - Shared fixture and replay errors
- Create: `crates/hpsvm-fixture/src/model.rs`
  - Canonical fixture data model
- Create: `crates/hpsvm-fixture/src/json.rs`
  - JSON codec implementation
- Create: `crates/hpsvm-fixture/src/capture.rs`
  - Explicit `CaptureBuilder` for transaction fixtures
- Create: `crates/hpsvm-fixture/src/runner.rs`
  - Replay engine that clones a baseline `HPSVM`
- Create: `crates/hpsvm-fixture/tests/json_roundtrip.rs`
  - JSON fixture round-trip coverage
- Create: `crates/hpsvm-fixture/tests/runner.rs`
  - End-to-end replay and validation tests

### `bin/hpsvm-cli`

- Create: `bin/hpsvm-cli/Cargo.toml`
  - Binary crate manifest, including `[[bin]] name = "hpsvm"`
- Create: `bin/hpsvm-cli/src/main.rs`
  - CLI parser and command dispatch
- Create: `bin/hpsvm-cli/src/error.rs`
  - CLI-facing error type
- Create: `bin/hpsvm-cli/src/program_map.rs`
  - `PROGRAM_ID=PATH` parsing and ELF preload helpers
- Create: `bin/hpsvm-cli/src/fixture.rs`
  - `inspect`, `run`, and `compare` command handlers
- Create: `bin/hpsvm-cli/src/config.rs`
  - YAML / JSON compare-config loader
- Create: `bin/hpsvm-cli/tests/fixture_inspect.rs`
  - Integration test for `hpsvm fixture inspect`
- Create: `bin/hpsvm-cli/tests/fixture_run.rs`
  - Integration test for `hpsvm fixture run`
- Create: `bin/hpsvm-cli/tests/fixture_compare.rs`
  - Integration test for `hpsvm fixture compare`

## Task 1: Scaffold `hpsvm-result` and Snapshot Conversion

**Files:**

- Modify: `Cargo.toml`
- Create: `crates/hpsvm-result/Cargo.toml`
- Create: `crates/hpsvm-result/src/lib.rs`
- Create: `crates/hpsvm-result/src/config.rs`
- Create: `crates/hpsvm-result/src/snapshot.rs`
- Create: `crates/hpsvm-result/tests/snapshot.rs`
- [ ] **Step 1: Write the failing test**

```rust
use hpsvm::HPSVM;
use hpsvm_result::{ExecutionSnapshot, ExecutionStatus};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::Transaction;

#[test]
fn snapshot_from_outcome_captures_post_accounts_and_metadata() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );

    let outcome = svm.transact(tx);
    let snapshot = ExecutionSnapshot::from_outcome(&outcome);

    assert!(matches!(snapshot.status, ExecutionStatus::Success));
    assert_eq!(snapshot.compute_units_consumed, outcome.meta().compute_units_consumed);
    assert_eq!(snapshot.fee, outcome.meta().fee);
    assert_eq!(snapshot.logs, outcome.meta().logs);
    assert!(snapshot.return_data.is_none());
    assert!(snapshot.inner_instructions.is_empty());
    assert!(snapshot
        .post_accounts
        .iter()
        .any(|account| account.address == recipient && account.lamports == 64));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p hpsvm-result --test snapshot snapshot_from_outcome_captures_post_accounts_and_metadata -- --exact`

Expected: FAIL with `package ID specification 'hpsvm-result' did not match any packages`.

- [ ] **Step 3: Write minimal implementation**

```toml
# Cargo.toml (add these lines under `[workspace.dependencies]`)
hpsvm-result = { path = "crates/hpsvm-result", version = "0.1.2" }

bincode = "1.3.3"
clap = "4.6.1"
serde_json = "1.0.149"
serde_yaml = "0.9.34"
```

```toml
# crates/hpsvm-result/Cargo.toml
[package]
name = "hpsvm-result"
description = "Serializable execution snapshots and result assertions for hpsvm"
license.workspace = true
version.workspace = true
edition.workspace = true
repository.workspace = true

[features]
default = []
serde = ["dep:serde"]

[dependencies]
hpsvm.workspace = true
serde = { workspace = true, optional = true, features = ["derive"] }
solana-account.workspace = true
solana-address.workspace = true
solana-message.workspace = true
solana-rent.workspace = true
solana-transaction-context.workspace = true
solana-transaction-error.workspace = true

[dev-dependencies]
solana-keypair.workspace = true
solana-message.workspace = true
solana-signer.workspace = true
solana-system-interface.workspace = true
solana-transaction.workspace = true
```

```rust
// crates/hpsvm-result/src/lib.rs
#![deny(broken_intra_doc_links)]

mod config;
mod snapshot;

pub use crate::{
    config::ResultConfig,
    snapshot::{
        AccountSnapshot,
        ExecutionSnapshot,
        ExecutionStatus,
        InnerInstructionSnapshot,
        ReturnDataSnapshot,
    },
};
```

```rust
// crates/hpsvm-result/src/config.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResultConfig {
    pub panic: bool,
    pub verbose: bool,
}

impl Default for ResultConfig {
    fn default() -> Self {
        Self {
            panic: true,
            verbose: false,
        }
    }
}
```

```rust
// crates/hpsvm-result/src/snapshot.rs
use hpsvm::{ExecutionOutcome, FailedTransactionMetadata, SimulatedTransactionInfo};
use solana_account::{AccountSharedData, ReadableAccount};
use solana_address::Address;
use solana_message::{inner_instruction::InnerInstructionsList, InnerInstruction};
use solana_transaction_error::{TransactionError, TransactionResult};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ExecutionSnapshot {
    pub status: ExecutionStatus,
    pub included: bool,
    pub compute_units_consumed: u64,
    pub fee: u64,
    pub logs: Vec<String>,
    pub return_data: Option<ReturnDataSnapshot>,
    pub inner_instructions: Vec<InnerInstructionSnapshot>,
    pub post_accounts: Vec<AccountSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum ExecutionStatus {
    Success,
    Failure { kind: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct AccountSnapshot {
    pub address: Address,
    pub lamports: u64,
    pub owner: Address,
    pub executable: bool,
    pub rent_epoch: u64,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ReturnDataSnapshot {
    pub program_id: Address,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct InnerInstructionSnapshot {
    pub stack_height: u32,
    pub program_id_index: u8,
    pub accounts: Vec<u8>,
    pub data: Vec<u8>,
}

impl ExecutionSnapshot {
    pub fn from_outcome(outcome: &ExecutionOutcome) -> Self {
        Self {
            status: ExecutionStatus::from_result(outcome.status()),
            included: outcome.included(),
            compute_units_consumed: outcome.meta().compute_units_consumed,
            fee: outcome.meta().fee,
            logs: outcome.meta().logs.clone(),
            return_data: ReturnDataSnapshot::from_meta(&outcome.meta().return_data),
            inner_instructions: flatten_inner_instructions(&outcome.meta().inner_instructions),
            post_accounts: outcome
                .post_accounts()
                .iter()
                .map(|(address, account)| AccountSnapshot::from_shared(*address, account))
                .collect(),
        }
    }

    pub fn from_simulation(result: &SimulatedTransactionInfo) -> Self {
        Self {
            status: ExecutionStatus::Success,
            included: true,
            compute_units_consumed: result.meta.compute_units_consumed,
            fee: result.meta.fee,
            logs: result.meta.logs.clone(),
            return_data: ReturnDataSnapshot::from_meta(&result.meta.return_data),
            inner_instructions: flatten_inner_instructions(&result.meta.inner_instructions),
            post_accounts: result
                .post_accounts
                .iter()
                .map(|(address, account)| AccountSnapshot::from_shared(*address, account))
                .collect(),
        }
    }

    pub fn from_failed_simulation(error: &FailedTransactionMetadata) -> Self {
        Self {
            status: ExecutionStatus::from_error(&error.err),
            included: false,
            compute_units_consumed: error.meta.compute_units_consumed,
            fee: error.meta.fee,
            logs: error.meta.logs.clone(),
            return_data: ReturnDataSnapshot::from_meta(&error.meta.return_data),
            inner_instructions: flatten_inner_instructions(&error.meta.inner_instructions),
            post_accounts: Vec::new(),
        }
    }
}

impl ExecutionStatus {
    fn from_result(result: &TransactionResult<()>) -> Self {
        match result {
            Ok(()) => Self::Success,
            Err(error) => Self::from_error(error),
        }
    }

    fn from_error(error: &TransactionError) -> Self {
        Self::Failure {
            kind: format!("{error:?}"),
            message: error.to_string(),
        }
    }
}

impl AccountSnapshot {
    fn from_shared(address: Address, account: &AccountSharedData) -> Self {
        Self {
            address,
            lamports: account.lamports(),
            owner: *account.owner(),
            executable: account.executable(),
            rent_epoch: account.rent_epoch(),
            data: account.data().to_vec(),
        }
    }
}

impl ReturnDataSnapshot {
    fn from_meta(return_data: &solana_transaction_context::TransactionReturnData) -> Option<Self> {
        if return_data.data.is_empty() {
            None
        } else {
            Some(Self {
                program_id: return_data.program_id,
                data: return_data.data.clone(),
            })
        }
    }
}

fn flatten_inner_instructions(groups: &InnerInstructionsList) -> Vec<InnerInstructionSnapshot> {
    groups
        .iter()
        .flat_map(|group| group.iter().map(InnerInstructionSnapshot::from_inner_instruction))
        .collect()
}

impl InnerInstructionSnapshot {
    fn from_inner_instruction(inner: &InnerInstruction) -> Self {
        Self {
            stack_height: inner.stack_height,
            program_id_index: inner.instruction.program_id_index,
            accounts: inner.instruction.accounts.clone(),
            data: inner.instruction.data.clone(),
        }
    }
}
```

- [ ] **Step 4: Run focused validation**

Run: `cargo test -p hpsvm-result --test snapshot -- --nocapture`

Expected: PASS. The snapshot crate compiles and converts live `hpsvm` outcomes.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml \
  crates/hpsvm-result/Cargo.toml \
  crates/hpsvm-result/src/lib.rs \
  crates/hpsvm-result/src/config.rs \
  crates/hpsvm-result/src/snapshot.rs \
  crates/hpsvm-result/tests/snapshot.rs
git commit -m "feat: add hpsvm-result snapshot layer"
```

## Task 2: Add `Check` and `Compare` DSLs to `hpsvm-result`

**Files:**

- Modify: `crates/hpsvm-result/src/lib.rs`
- Create: `crates/hpsvm-result/src/check.rs`
- Create: `crates/hpsvm-result/src/compare.rs`
- Create: `crates/hpsvm-result/tests/checks.rs`
- Create: `crates/hpsvm-result/tests/compare.rs`
- [ ] **Step 1: Write the failing tests**

```rust
// crates/hpsvm-result/tests/checks.rs
use hpsvm::HPSVM;
use hpsvm_result::{Check, ExecutionSnapshot, ResultConfig};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::Transaction;

#[test]
fn checks_can_assert_success_and_resulting_lamports() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );

    let snapshot = ExecutionSnapshot::from_outcome(&svm.transact(tx));
    let checks = vec![
        Check::Success,
        Check::ComputeUnits(snapshot.compute_units_consumed),
        Check::account(&recipient).lamports(64).build(),
    ];

    assert!(snapshot.run_checks(
        &checks,
        &ResultConfig {
            panic: false,
            verbose: true,
        },
    ));
}
```

```rust
// crates/hpsvm-result/tests/compare.rs
use hpsvm::HPSVM;
use hpsvm_result::{AccountCompareScope, Compare, ExecutionSnapshot, ResultConfig};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::Transaction;

#[test]
fn compare_can_ignore_unselected_accounts() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    );

    let baseline = ExecutionSnapshot::from_outcome(&svm.transact(tx));
    let mut candidate = baseline.clone();
    let payer_account = candidate
        .post_accounts
        .iter_mut()
        .find(|account| account.address == payer.pubkey())
        .unwrap();
    payer_account.lamports = payer_account.lamports.saturating_sub(1);

    let config = ResultConfig {
        panic: false,
        verbose: true,
    };

    assert!(baseline.compare_with(
        &candidate,
        &[Compare::Accounts(AccountCompareScope::Only(vec![recipient]))],
        &config,
    ));
    assert!(!baseline.compare_with(&candidate, &[Compare::Accounts(AccountCompareScope::All)], &config));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p hpsvm-result --test checks checks_can_assert_success_and_resulting_lamports -- --exact`

Expected: FAIL with unresolved imports for `Check` and `ExecutionSnapshot::run_checks`.

Run: `cargo test -p hpsvm-result --test compare compare_can_ignore_unselected_accounts -- --exact`

Expected: FAIL with unresolved imports for `Compare` and `ExecutionSnapshot::compare_with`.

- [ ] **Step 3: Write minimal implementation**

```rust
// crates/hpsvm-result/src/lib.rs
#![deny(broken_intra_doc_links)]

mod check;
mod compare;
mod config;
mod snapshot;

pub use crate::{
    check::{AccountExpectation, AccountExpectationBuilder, Check},
    compare::{AccountCompareScope, Compare},
    config::ResultConfig,
    snapshot::{
        AccountSnapshot,
        ExecutionSnapshot,
        ExecutionStatus,
        InnerInstructionSnapshot,
        ReturnDataSnapshot,
    },
};
```

```rust
// crates/hpsvm-result/src/check.rs
use crate::{ExecutionSnapshot, ExecutionStatus, ResultConfig};
use solana_address::Address;
use solana_rent::Rent;

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum Check {
    Success,
    Failure,
    Included(bool),
    ComputeUnits(u64),
    Fee(u64),
    ReturnData(Vec<u8>),
    LogContains(String),
    InnerInstructionCount(usize),
    Account(AccountExpectation),
    AllRentExempt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct AccountExpectation {
    pub address: Address,
    pub lamports: Option<u64>,
    pub owner: Option<Address>,
    pub executable: Option<bool>,
    pub data: Option<Vec<u8>>,
    pub data_slice: Option<(usize, Vec<u8>)>,
    pub closed: Option<bool>,
    pub rent_exempt: Option<bool>,
}

#[derive(Debug, Clone)]
#[must_use = "builder methods return a new builder"]
pub struct AccountExpectationBuilder {
    inner: AccountExpectation,
}

impl Check {
    pub fn account(address: &Address) -> AccountExpectationBuilder {
        AccountExpectationBuilder {
            inner: AccountExpectation {
                address: *address,
                lamports: None,
                owner: None,
                executable: None,
                data: None,
                data_slice: None,
                closed: None,
                rent_exempt: None,
            },
        }
    }
}

impl AccountExpectationBuilder {
    pub fn lamports(mut self, lamports: u64) -> Self {
        self.inner.lamports = Some(lamports);
        self
    }

    pub fn owner(mut self, owner: Address) -> Self {
        self.inner.owner = Some(owner);
        self
    }

    pub fn executable(mut self, executable: bool) -> Self {
        self.inner.executable = Some(executable);
        self
    }

    pub fn data(mut self, data: Vec<u8>) -> Self {
        self.inner.data = Some(data);
        self
    }

    pub fn data_slice(mut self, offset: usize, data: Vec<u8>) -> Self {
        self.inner.data_slice = Some((offset, data));
        self
    }

    pub fn closed(mut self) -> Self {
        self.inner.closed = Some(true);
        self
    }

    pub fn rent_exempt(mut self) -> Self {
        self.inner.rent_exempt = Some(true);
        self
    }

    pub fn build(self) -> Check {
        Check::Account(self.inner)
    }
}

impl ExecutionSnapshot {
    pub fn run_checks(&self, checks: &[Check], config: &ResultConfig) -> bool {
        for check in checks {
            let pass = match check {
                Check::Success => matches!(self.status, ExecutionStatus::Success),
                Check::Failure => !matches!(self.status, ExecutionStatus::Success),
                Check::Included(expected) => self.included == *expected,
                Check::ComputeUnits(expected) => self.compute_units_consumed == *expected,
                Check::Fee(expected) => self.fee == *expected,
                Check::ReturnData(expected) => self
                    .return_data
                    .as_ref()
                    .map_or(expected.is_empty(), |return_data| return_data.data == *expected),
                Check::LogContains(expected) => self.logs.iter().any(|line| line.contains(expected)),
                Check::InnerInstructionCount(expected) => self.inner_instructions.len() == *expected,
                Check::Account(expected) => match self
                    .post_accounts
                    .iter()
                    .find(|account| account.address == expected.address)
                {
                    Some(account) => account_matches(account, expected),
                    None => false,
                },
                Check::AllRentExempt => self.post_accounts.iter().all(|account| {
                    account.lamports == 0 || Rent::default().is_exempt(account.lamports, account.data.len())
                }),
            };

            if !pass {
                return fail(config, format!("check failed: {check:?}"));
            }
        }

        true
    }
}

fn account_matches(account: &crate::AccountSnapshot, expected: &AccountExpectation) -> bool {
    if expected.lamports.is_some_and(|lamports| account.lamports != lamports) {
        return false;
    }
    if expected.owner.is_some_and(|owner| account.owner != owner) {
        return false;
    }
    if expected
        .executable
        .is_some_and(|executable| account.executable != executable)
    {
        return false;
    }
    if expected
        .data
        .as_ref()
        .is_some_and(|data| account.data != *data)
    {
        return false;
    }
    if let Some((offset, data)) = &expected.data_slice {
        let end = offset.saturating_add(data.len());
        if end > account.data.len() || &account.data[*offset..end] != data.as_slice() {
            return false;
        }
    }
    if expected.closed.is_some_and(|closed| closed) && !(account.lamports == 0 && account.data.is_empty()) {
        return false;
    }
    if expected.rent_exempt.is_some_and(|rent_exempt| rent_exempt)
        && !Rent::default().is_exempt(account.lamports, account.data.len())
    {
        return false;
    }
    true
}

fn fail(config: &ResultConfig, message: String) -> bool {
    if config.panic {
        panic!("{message}");
    }
    if config.verbose {
        eprintln!("{message}");
    }
    false
}
```

```rust
// crates/hpsvm-result/src/compare.rs
use crate::{ExecutionSnapshot, ResultConfig};
use solana_address::Address;

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum Compare {
    Status,
    Included,
    ComputeUnits,
    Fee,
    ReturnData,
    Logs,
    InnerInstructionCount,
    Accounts(AccountCompareScope),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum AccountCompareScope {
    All,
    Only(Vec<Address>),
    AllExcept(Vec<Address>),
}

impl Compare {
    pub fn everything() -> Vec<Self> {
        vec![
            Self::Status,
            Self::Included,
            Self::ComputeUnits,
            Self::Fee,
            Self::ReturnData,
            Self::Logs,
            Self::InnerInstructionCount,
            Self::Accounts(AccountCompareScope::All),
        ]
    }

    pub fn everything_but_compute_units() -> Vec<Self> {
        vec![
            Self::Status,
            Self::Included,
            Self::Fee,
            Self::ReturnData,
            Self::Logs,
            Self::InnerInstructionCount,
            Self::Accounts(AccountCompareScope::All),
        ]
    }
}

impl ExecutionSnapshot {
    pub fn compare_with(
        &self,
        other: &ExecutionSnapshot,
        compares: &[Compare],
        config: &ResultConfig,
    ) -> bool {
        for compare in compares {
            let pass = match compare {
                Compare::Status => self.status == other.status,
                Compare::Included => self.included == other.included,
                Compare::ComputeUnits => self.compute_units_consumed == other.compute_units_consumed,
                Compare::Fee => self.fee == other.fee,
                Compare::ReturnData => self.return_data == other.return_data,
                Compare::Logs => self.logs == other.logs,
                Compare::InnerInstructionCount => self.inner_instructions.len() == other.inner_instructions.len(),
                Compare::Accounts(scope) => compare_accounts(self, other, scope),
            };

            if !pass {
                return fail(config, format!("comparison failed: {compare:?}"));
            }
        }

        true
    }
}

fn compare_accounts(
    left: &ExecutionSnapshot,
    right: &ExecutionSnapshot,
    scope: &AccountCompareScope,
) -> bool {
    let should_compare = |address: &Address| match scope {
        AccountCompareScope::All => true,
        AccountCompareScope::Only(addresses) => addresses.contains(address),
        AccountCompareScope::AllExcept(addresses) => !addresses.contains(address),
    };

    for account in &left.post_accounts {
        if !should_compare(&account.address) {
            continue;
        }
        let Some(other_account) = right.post_accounts.iter().find(|candidate| candidate.address == account.address) else {
            return false;
        };
        if account != other_account {
            return false;
        }
    }

    true
}

fn fail(config: &ResultConfig, message: String) -> bool {
    if config.panic {
        panic!("{message}");
    }
    if config.verbose {
        eprintln!("{message}");
    }
    false
}
```

- [ ] **Step 4: Run focused validation**

Run: `cargo test -p hpsvm-result --test checks -- --nocapture`

Expected: PASS.

Run: `cargo test -p hpsvm-result --test compare -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/hpsvm-result/src/lib.rs \
  crates/hpsvm-result/src/check.rs \
  crates/hpsvm-result/src/compare.rs \
  crates/hpsvm-result/tests/checks.rs \
  crates/hpsvm-result/tests/compare.rs
git commit -m "feat: add result checks and comparisons"
```

## Task 3: Add `hpsvm-fixture` Model, JSON Codec, and Explicit Capture Builder

**Files:**

- Modify: `Cargo.toml`
- Create: `crates/hpsvm-fixture/Cargo.toml`
- Create: `crates/hpsvm-fixture/src/lib.rs`
- Create: `crates/hpsvm-fixture/src/error.rs`
- Create: `crates/hpsvm-fixture/src/model.rs`
- Create: `crates/hpsvm-fixture/src/json.rs`
- Create: `crates/hpsvm-fixture/src/capture.rs`
- Create: `crates/hpsvm-fixture/tests/json_roundtrip.rs`
- [ ] **Step 1: Write the failing test**

```rust
use hpsvm::HPSVM;
use hpsvm_fixture::{CaptureBuilder, Fixture, FixtureFormat, RuntimeFixtureConfig};
use hpsvm_result::{AccountSnapshot, Compare, ExecutionSnapshot};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::versioned::VersionedTransaction;

fn snapshot_account(svm: &HPSVM, address: Address) -> AccountSnapshot {
    let account = svm.get_account(&address).unwrap();
    AccountSnapshot {
        address,
        lamports: account.lamports,
        owner: account.owner,
        executable: account.executable,
        rent_epoch: account.rent_epoch,
        data: account.data,
    }
}

#[test]
fn json_fixture_roundtrip_preserves_transaction_fixture() {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = VersionedTransaction::from(solana_transaction::Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    ));
    let baseline = ExecutionSnapshot::from_outcome(&svm.transact(tx.clone()));

    let fixture = CaptureBuilder::new("system-transfer")
        .runtime(RuntimeFixtureConfig {
            slot: svm.block_env().slot,
            log_bytes_limit: None,
            sigverify: true,
            blockhash_check: false,
        })
        .pre_accounts(vec![
            snapshot_account(&svm, payer.pubkey()),
            snapshot_account(&svm, recipient),
        ])
        .baseline(baseline)
        .compares(Compare::everything())
        .capture_transaction(&tx)
        .unwrap();

    let path = std::env::temp_dir().join("hpsvm-fixture-roundtrip.json");
    fixture.save(&path, FixtureFormat::Json).unwrap();
    let loaded = Fixture::load(&path).unwrap();

    assert_eq!(loaded, fixture);

    std::fs::remove_file(path).ok();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p hpsvm-fixture --test json_roundtrip json_fixture_roundtrip_preserves_transaction_fixture -- --exact`

Expected: FAIL with `package ID specification 'hpsvm-fixture' did not match any packages`.

- [ ] **Step 3: Write minimal implementation**

```toml
# Cargo.toml (add this line under `[workspace.dependencies]`)
hpsvm-fixture = { path = "crates/hpsvm-fixture", version = "0.1.2" }
```

```toml
# crates/hpsvm-fixture/Cargo.toml
[package]
name = "hpsvm-fixture"
description = "Transaction-first fixture capture and replay for hpsvm"
license.workspace = true
version.workspace = true
edition.workspace = true
repository.workspace = true

[features]
default = ["json-codec"]
serde = ["dep:serde", "hpsvm-result/serde"]
json-codec = ["serde"]

[dependencies]
bincode.workspace = true
hpsvm.workspace = true
hpsvm-result = { workspace = true, features = ["serde"] }
serde = { workspace = true, optional = true, features = ["derive"] }
serde_json.workspace = true
solana-account.workspace = true
solana-address.workspace = true
solana-transaction.workspace = true
thiserror.workspace = true

[dev-dependencies]
solana-keypair.workspace = true
solana-message.workspace = true
solana-signer.workspace = true
solana-system-interface.workspace = true
```

```rust
// crates/hpsvm-fixture/src/lib.rs
#![deny(broken_intra_doc_links)]

mod capture;
mod error;
#[cfg(feature = "json-codec")]
mod json;
mod model;

pub use crate::{
    capture::CaptureBuilder,
    error::FixtureError,
    model::{
        Fixture,
        FixtureExpectations,
        FixtureFormat,
        FixtureHeader,
        FixtureInput,
        FixtureKind,
        ProgramBinding,
        RuntimeFixtureConfig,
        TransactionFixture,
    },
};

impl Fixture {
    #[cfg(feature = "json-codec")]
    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self, FixtureError> {
        json::load(path.as_ref())
    }

    #[cfg(feature = "json-codec")]
    pub fn save(
        &self,
        path: impl AsRef<std::path::Path>,
        format: FixtureFormat,
    ) -> Result<(), FixtureError> {
        match format {
            FixtureFormat::Json => json::save(self, path.as_ref()),
        }
    }
}
```

```rust
// crates/hpsvm-fixture/src/error.rs
use solana_address::Address;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum FixtureError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[cfg(feature = "json-codec")]
    #[error("JSON codec error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("failed to encode transaction: {0}")]
    EncodeTransaction(Box<bincode::ErrorKind>),
    #[error("unsupported fixture format for {path}")]
    UnsupportedFormat { path: String },
    #[error("missing required field {field}")]
    MissingField { field: &'static str },
    #[error("missing program ELF for {program_id}")]
    MissingProgramElf { program_id: Address },
    #[error(transparent)]
    Hpsvm(#[from] hpsvm::HPSVMError),
}
```

```rust
// crates/hpsvm-fixture/src/model.rs
use hpsvm_result::{AccountSnapshot, Compare, ExecutionSnapshot};
use solana_address::Address;

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Fixture {
    pub header: FixtureHeader,
    pub input: FixtureInput,
    pub expectations: FixtureExpectations,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct FixtureHeader {
    pub schema_version: u16,
    pub name: String,
    pub kind: FixtureKind,
    pub source: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum FixtureKind {
    Transaction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum FixtureInput {
    Transaction(TransactionFixture),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct TransactionFixture {
    pub runtime: RuntimeFixtureConfig,
    pub programs: Vec<ProgramBinding>,
    pub pre_accounts: Vec<AccountSnapshot>,
    pub transaction_bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct RuntimeFixtureConfig {
    pub slot: u64,
    pub log_bytes_limit: Option<usize>,
    pub sigverify: bool,
    pub blockhash_check: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ProgramBinding {
    pub program_id: Address,
    pub loader_id: Address,
    pub role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct FixtureExpectations {
    pub baseline: ExecutionSnapshot,
    pub compares: Vec<Compare>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureFormat {
    Json,
}
```

```rust
// crates/hpsvm-fixture/src/json.rs
use crate::{Fixture, FixtureError};

pub fn load(path: &std::path::Path) -> Result<Fixture, FixtureError> {
    if path.extension().and_then(|value| value.to_str()) != Some("json") {
        return Err(FixtureError::UnsupportedFormat {
            path: path.display().to_string(),
        });
    }

    let file = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&file)?)
}

pub fn save(fixture: &Fixture, path: &std::path::Path) -> Result<(), FixtureError> {
    let json = serde_json::to_string_pretty(fixture)?;
    std::fs::write(path, json)?;
    Ok(())
}
```

```rust
// crates/hpsvm-fixture/src/capture.rs
use crate::{
    Fixture,
    FixtureError,
    FixtureExpectations,
    FixtureHeader,
    FixtureInput,
    FixtureKind,
    RuntimeFixtureConfig,
    TransactionFixture,
};
use hpsvm_result::{AccountSnapshot, Compare, ExecutionSnapshot};
use solana_transaction::versioned::VersionedTransaction;

#[derive(Debug, Default, Clone)]
#[must_use = "capture builders do nothing unless you finish them into a fixture"]
pub struct CaptureBuilder {
    header: Option<FixtureHeader>,
    runtime: Option<RuntimeFixtureConfig>,
    programs: Vec<crate::ProgramBinding>,
    pre_accounts: Vec<AccountSnapshot>,
    baseline: Option<ExecutionSnapshot>,
    compares: Vec<Compare>,
}

impl CaptureBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            header: Some(FixtureHeader {
                schema_version: 1,
                name: name.into(),
                kind: FixtureKind::Transaction,
                source: None,
                tags: Vec::new(),
            }),
            ..Self::default()
        }
    }

    pub fn source(mut self, source: impl Into<String>) -> Self {
        if let Some(header) = self.header.as_mut() {
            header.source = Some(source.into());
        }
        self
    }

    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        if let Some(header) = self.header.as_mut() {
            header.tags.push(tag.into());
        }
        self
    }

    pub fn runtime(mut self, runtime: RuntimeFixtureConfig) -> Self {
        self.runtime = Some(runtime);
        self
    }

    pub fn programs(mut self, programs: Vec<crate::ProgramBinding>) -> Self {
        self.programs = programs;
        self
    }

    pub fn pre_accounts(mut self, pre_accounts: Vec<AccountSnapshot>) -> Self {
        self.pre_accounts = pre_accounts;
        self
    }

    pub fn baseline(mut self, baseline: ExecutionSnapshot) -> Self {
        self.baseline = Some(baseline);
        self
    }

    pub fn compares(mut self, compares: Vec<Compare>) -> Self {
        self.compares = compares;
        self
    }

    pub fn capture_transaction(self, tx: &VersionedTransaction) -> Result<Fixture, FixtureError> {
        let transaction_bytes = bincode::serialize(tx).map_err(FixtureError::EncodeTransaction)?;
        let header = self.header.ok_or(FixtureError::MissingField { field: "header" })?;
        let runtime = self.runtime.ok_or(FixtureError::MissingField { field: "runtime" })?;
        let baseline = self.baseline.ok_or(FixtureError::MissingField { field: "baseline" })?;

        Ok(Fixture {
            header,
            input: FixtureInput::Transaction(TransactionFixture {
                runtime,
                programs: self.programs,
                pre_accounts: self.pre_accounts,
                transaction_bytes,
            }),
            expectations: FixtureExpectations {
                baseline,
                compares: if self.compares.is_empty() {
                    Compare::everything()
                } else {
                    self.compares
                },
            },
        })
    }
}
```

- [ ] **Step 4: Run focused validation**

Run: `cargo test -p hpsvm-fixture --test json_roundtrip -- --nocapture`

Expected: PASS. The fixture crate can capture and round-trip JSON fixtures.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml \
  crates/hpsvm-fixture/Cargo.toml \
  crates/hpsvm-fixture/src/lib.rs \
  crates/hpsvm-fixture/src/error.rs \
  crates/hpsvm-fixture/src/model.rs \
  crates/hpsvm-fixture/src/json.rs \
  crates/hpsvm-fixture/src/capture.rs \
  crates/hpsvm-fixture/tests/json_roundtrip.rs
git commit -m "feat: add transaction fixture model and json codec"
```

## Task 4: Add `FixtureRunner` and End-to-End Replay Validation

**Files:**

- Modify: `crates/hpsvm-fixture/src/lib.rs`
- Modify: `crates/hpsvm-fixture/src/error.rs`
- Create: `crates/hpsvm-fixture/src/runner.rs`
- Create: `crates/hpsvm-fixture/tests/runner.rs`
- [ ] **Step 1: Write the failing tests**

```rust
use hpsvm::HPSVM;
use hpsvm_fixture::{CaptureBuilder, FixtureRunner, RuntimeFixtureConfig};
use hpsvm_result::{AccountSnapshot, Compare, ExecutionSnapshot, ResultConfig};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::versioned::VersionedTransaction;

fn snapshot_account(svm: &HPSVM, address: Address) -> AccountSnapshot {
    let account = svm.get_account(&address).unwrap();
    AccountSnapshot {
        address,
        lamports: account.lamports,
        owner: account.owner,
        executable: account.executable,
        rent_epoch: account.rent_epoch,
        data: account.data,
    }
}

fn build_fixture() -> hpsvm_fixture::Fixture {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = VersionedTransaction::from(solana_transaction::Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    ));
    let baseline = ExecutionSnapshot::from_outcome(&svm.transact(tx.clone()));

    CaptureBuilder::new("runner-transfer")
        .runtime(RuntimeFixtureConfig {
            slot: svm.block_env().slot,
            log_bytes_limit: None,
            sigverify: true,
            blockhash_check: false,
        })
        .pre_accounts(vec![
            snapshot_account(&svm, payer.pubkey()),
            snapshot_account(&svm, recipient),
        ])
        .baseline(baseline)
        .compares(Compare::everything())
        .capture_transaction(&tx)
        .unwrap()
}

#[test]
fn runner_replays_transaction_fixture_against_a_cloned_vm() {
    let fixture = build_fixture();
    let mut runner = FixtureRunner::new(HPSVM::new());

    let execution = runner.run(&fixture).unwrap();

    assert!(execution
        .snapshot
        .compare_with(&fixture.expectations.baseline, &Compare::everything(), &ResultConfig {
            panic: false,
            verbose: true,
        }));
}

#[test]
fn runner_can_apply_fixture_default_compares() {
    let mut fixture = build_fixture();
    fixture.expectations.baseline.compute_units_consumed += 1;
    fixture.expectations.compares = Compare::everything_but_compute_units();

    let mut runner = FixtureRunner::new(HPSVM::new());
    let pass = runner
        .run_and_validate(
            &fixture,
            &ResultConfig {
                panic: false,
                verbose: true,
            },
        )
        .unwrap();

    assert!(pass);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p hpsvm-fixture --test runner runner_replays_transaction_fixture_against_a_cloned_vm -- --exact`

Expected: FAIL with unresolved imports for `FixtureRunner`.

- [ ] **Step 3: Write minimal implementation**

```rust
// crates/hpsvm-fixture/src/lib.rs
#![deny(broken_intra_doc_links)]

mod capture;
mod error;
mod json;
mod model;
mod runner;

pub use crate::{
    capture::CaptureBuilder,
    error::FixtureError,
    model::{
        Fixture,
        FixtureExpectations,
        FixtureFormat,
        FixtureHeader,
        FixtureInput,
        FixtureKind,
        ProgramBinding,
        RuntimeFixtureConfig,
        TransactionFixture,
    },
    runner::{FixtureExecution, FixtureRunner},
};

impl Fixture {
    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self, FixtureError> {
        json::load(path.as_ref())
    }

    pub fn save(
        &self,
        path: impl AsRef<std::path::Path>,
        format: FixtureFormat,
    ) -> Result<(), FixtureError> {
        match format {
            FixtureFormat::Json => json::save(self, path.as_ref()),
        }
    }
}
```

```rust
// crates/hpsvm-fixture/src/error.rs
use solana_address::Address;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum FixtureError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[cfg(feature = "json-codec")]
    #[error("JSON codec error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("failed to encode transaction: {0}")]
    EncodeTransaction(Box<bincode::ErrorKind>),
    #[error("failed to decode transaction: {0}")]
    DecodeTransaction(Box<bincode::ErrorKind>),
    #[error("unsupported fixture format for {path}")]
    UnsupportedFormat { path: String },
    #[error("missing required field {field}")]
    MissingField { field: &'static str },
    #[error("missing program ELF for {program_id}")]
    MissingProgramElf { program_id: Address },
    #[error(transparent)]
    Hpsvm(#[from] hpsvm::HPSVMError),
}
```

```rust
// crates/hpsvm-fixture/src/runner.rs
use std::collections::HashMap;

use hpsvm::HPSVM;
use hpsvm_result::{ExecutionSnapshot, ResultConfig};
use solana_account::Account;
use solana_address::Address;
use solana_transaction::versioned::VersionedTransaction;

use crate::{Fixture, FixtureError, FixtureInput, TransactionFixture};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureExecution {
    pub snapshot: ExecutionSnapshot,
    pub pass: Option<bool>,
}

#[must_use = "fixture runners must be used to execute fixtures"]
pub struct FixtureRunner {
    base_vm: HPSVM,
    program_elfs: HashMap<Address, Vec<u8>>,
}

impl FixtureRunner {
    pub fn new(vm: HPSVM) -> Self {
        Self {
            base_vm: vm,
            program_elfs: HashMap::new(),
        }
    }

    pub fn with_program_elf(mut self, program_id: Address, elf: Vec<u8>) -> Self {
        self.program_elfs.insert(program_id, elf);
        self
    }

    pub fn run(&mut self, fixture: &Fixture) -> Result<FixtureExecution, FixtureError> {
        let snapshot = match &fixture.input {
            FixtureInput::Transaction(transaction) => self.run_transaction_fixture(transaction)?,
        };

        Ok(FixtureExecution {
            snapshot,
            pass: None,
        })
    }

    pub fn run_and_validate(
        &mut self,
        fixture: &Fixture,
        config: &ResultConfig,
    ) -> Result<bool, FixtureError> {
        let snapshot = self.run(fixture)?.snapshot;
        Ok(snapshot.compare_with(
            &fixture.expectations.baseline,
            &fixture.expectations.compares,
            config,
        ))
    }

    fn run_transaction_fixture(
        &self,
        fixture: &TransactionFixture,
    ) -> Result<ExecutionSnapshot, FixtureError> {
        let mut vm = self.base_vm.clone();
        vm.set_sigverify(fixture.runtime.sigverify);
        vm.set_blockhash_check(fixture.runtime.blockhash_check);
        vm.set_log_bytes_limit(fixture.runtime.log_bytes_limit);
        vm.warp_to_slot(fixture.runtime.slot);

        for program in &fixture.programs {
            let elf = self
                .program_elfs
                .get(&program.program_id)
                .ok_or(FixtureError::MissingProgramElf {
                    program_id: program.program_id,
                })?;
            vm.add_program_with_loader(program.program_id, elf, program.loader_id)?;
        }

        for account in &fixture.pre_accounts {
            vm.set_account(
                account.address,
                Account {
                    lamports: account.lamports,
                    data: account.data.clone(),
                    owner: account.owner,
                    executable: account.executable,
                    rent_epoch: account.rent_epoch,
                },
            )?;
        }

        let tx: VersionedTransaction =
            bincode::deserialize(&fixture.transaction_bytes).map_err(FixtureError::DecodeTransaction)?;
        Ok(ExecutionSnapshot::from_outcome(&vm.transact(tx)))
    }
}
```

- [ ] **Step 4: Run focused validation**

Run: `cargo test -p hpsvm-fixture --test runner -- --nocapture`

Expected: PASS. Fixtures replay on cloned VMs and compare against their saved baselines.

- [ ] **Step 5: Commit**

```bash
git add crates/hpsvm-fixture/src/lib.rs \
  crates/hpsvm-fixture/src/error.rs \
  crates/hpsvm-fixture/src/runner.rs \
  crates/hpsvm-fixture/tests/runner.rs
git commit -m "feat: add fixture replay runner"
```

## Task 5: Scaffold `hpsvm-cli` with `fixture inspect` and `fixture run`

**Files:**

- Modify: `Cargo.toml`
- Create: `bin/hpsvm-cli/Cargo.toml`
- Create: `bin/hpsvm-cli/src/main.rs`
- Create: `bin/hpsvm-cli/src/error.rs`
- Create: `bin/hpsvm-cli/src/fixture.rs`
- Create: `bin/hpsvm-cli/src/program_map.rs`
- Create: `bin/hpsvm-cli/tests/fixture_inspect.rs`
- Create: `bin/hpsvm-cli/tests/fixture_run.rs`
- [ ] **Step 1: Write the failing tests**

```rust
// bin/hpsvm-cli/tests/fixture_inspect.rs
use std::process::Command;

use hpsvm::HPSVM;
use hpsvm_fixture::{CaptureBuilder, FixtureFormat, RuntimeFixtureConfig};
use hpsvm_result::{AccountSnapshot, Compare, ExecutionSnapshot};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::versioned::VersionedTransaction;

fn snapshot_account(svm: &HPSVM, address: Address) -> AccountSnapshot {
    let account = svm.get_account(&address).unwrap();
    AccountSnapshot {
        address,
        lamports: account.lamports,
        owner: account.owner,
        executable: account.executable,
        rent_epoch: account.rent_epoch,
        data: account.data,
    }
}

fn write_fixture() -> std::path::PathBuf {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = VersionedTransaction::from(solana_transaction::Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    ));
    let baseline = ExecutionSnapshot::from_outcome(&svm.transact(tx.clone()));
    let fixture = CaptureBuilder::new("cli-inspect")
        .runtime(RuntimeFixtureConfig {
            slot: svm.block_env().slot,
            log_bytes_limit: None,
            sigverify: true,
            blockhash_check: false,
        })
        .pre_accounts(vec![
            snapshot_account(&svm, payer.pubkey()),
            snapshot_account(&svm, recipient),
        ])
        .baseline(baseline)
        .compares(Compare::everything())
        .capture_transaction(&tx)
        .unwrap();

    let path = std::env::temp_dir().join("hpsvm-cli-inspect.json");
    fixture.save(&path, FixtureFormat::Json).unwrap();
    path
}

#[test]
fn fixture_inspect_prints_fixture_name() {
    let path = write_fixture();

    let output = Command::new(env!("CARGO_BIN_EXE_hpsvm"))
        .args(["fixture", "inspect", path.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("cli-inspect"));

    std::fs::remove_file(path).ok();
}
```

```rust
// bin/hpsvm-cli/tests/fixture_run.rs
use std::process::Command;

use hpsvm::HPSVM;
use hpsvm_fixture::{CaptureBuilder, FixtureFormat, RuntimeFixtureConfig};
use hpsvm_result::{AccountSnapshot, Compare, ExecutionSnapshot};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::versioned::VersionedTransaction;

fn snapshot_account(svm: &HPSVM, address: Address) -> AccountSnapshot {
    let account = svm.get_account(&address).unwrap();
    AccountSnapshot {
        address,
        lamports: account.lamports,
        owner: account.owner,
        executable: account.executable,
        rent_epoch: account.rent_epoch,
        data: account.data,
    }
}

fn write_fixture() -> std::path::PathBuf {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = VersionedTransaction::from(solana_transaction::Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    ));
    let baseline = ExecutionSnapshot::from_outcome(&svm.transact(tx.clone()));
    let fixture = CaptureBuilder::new("cli-run")
        .runtime(RuntimeFixtureConfig {
            slot: svm.block_env().slot,
            log_bytes_limit: None,
            sigverify: true,
            blockhash_check: false,
        })
        .pre_accounts(vec![
            snapshot_account(&svm, payer.pubkey()),
            snapshot_account(&svm, recipient),
        ])
        .baseline(baseline)
        .compares(Compare::everything())
        .capture_transaction(&tx)
        .unwrap();

    let path = std::env::temp_dir().join("hpsvm-cli-run.json");
    fixture.save(&path, FixtureFormat::Json).unwrap();
    path
}

#[test]
fn fixture_run_reports_pass_for_matching_fixture() {
    let path = write_fixture();

    let output = Command::new(env!("CARGO_BIN_EXE_hpsvm"))
        .args(["fixture", "run", path.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("PASS"));

    std::fs::remove_file(path).ok();
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p hpsvm-cli --test fixture_inspect fixture_inspect_prints_fixture_name -- --exact`

Expected: FAIL with `package ID specification 'hpsvm-cli' did not match any packages`.

- [ ] **Step 3: Write minimal implementation**

```toml
# Cargo.toml (add this line under `[workspace.dependencies]`)
hpsvm-cli = { path = "bin/hpsvm-cli", version = "0.1.2" }
```

```toml
# bin/hpsvm-cli/Cargo.toml
[package]
name = "hpsvm-cli"
description = "Command-line fixture tools for hpsvm"
license.workspace = true
version.workspace = true
edition.workspace = true
repository.workspace = true

[[bin]]
name = "hpsvm"
path = "src/main.rs"

[dependencies]
clap = { workspace = true, features = ["derive"] }
hpsvm.workspace = true
hpsvm-fixture = { workspace = true, features = ["json-codec"] }
hpsvm-result = { workspace = true, features = ["serde"] }
serde = { workspace = true, features = ["derive"] }
serde_json.workspace = true
serde_yaml.workspace = true
thiserror.workspace = true

[dev-dependencies]
solana-keypair.workspace = true
solana-message.workspace = true
solana-signer.workspace = true
solana-system-interface.workspace = true
solana-transaction.workspace = true
```

```rust
// bin/hpsvm-cli/src/error.rs
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CliError {
    #[error(transparent)]
    Fixture(#[from] hpsvm_fixture::FixtureError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("failed to parse config {path}: {reason}")]
    ConfigParse { path: String, reason: String },
    #[error("unsupported config format for {path}")]
    UnsupportedConfigFormat { path: String },
    #[error("invalid --program value {value}, expected <program-id>=<path>")]
    InvalidProgramMapping { value: String },
    #[error("invalid program id {value}: {reason}")]
    InvalidProgramId { value: String, reason: String },
}
```

```rust
// bin/hpsvm-cli/src/program_map.rs
use std::{collections::HashMap, fs};

use hpsvm_fixture::{Fixture, FixtureInput, FixtureRunner};
use solana_address::Address;

use crate::error::CliError;

pub fn parse_program_map(values: &[String]) -> Result<HashMap<Address, Vec<u8>>, CliError> {
    let mut parsed = HashMap::new();

    for value in values {
        let Some((program_id, path)) = value.split_once('=') else {
            return Err(CliError::InvalidProgramMapping {
                value: value.clone(),
            });
        };

        let program_id = program_id.parse::<Address>().map_err(|error| CliError::InvalidProgramId {
            value: program_id.to_string(),
            reason: error.to_string(),
        })?;
        parsed.insert(program_id, fs::read(path)?);
    }

    Ok(parsed)
}

pub fn preload_runner(
    mut runner: FixtureRunner,
    fixture: &Fixture,
    programs: &HashMap<Address, Vec<u8>>,
) -> FixtureRunner {
    match &fixture.input {
        FixtureInput::Transaction(transaction) => {
            for binding in &transaction.programs {
                if let Some(bytes) = programs.get(&binding.program_id) {
                    runner = runner.with_program_elf(binding.program_id, bytes.clone());
                }
            }
            runner
        }
    }
}
```

```rust
// bin/hpsvm-cli/src/fixture.rs
use std::path::Path;

use hpsvm::HPSVM;
use hpsvm_fixture::{Fixture, FixtureRunner};
use hpsvm_result::ResultConfig;

use crate::{
    error::CliError,
    program_map::{parse_program_map, preload_runner},
};

pub fn inspect_fixture(path: &Path) -> Result<(), CliError> {
    let fixture = Fixture::load(path)?;
    println!("{}", serde_json::to_string_pretty(&fixture)?);
    Ok(())
}

pub fn run_fixture(path: &Path, program_args: &[String]) -> Result<(), CliError> {
    let fixture = Fixture::load(path)?;
    let programs = parse_program_map(program_args)?;
    let mut runner = preload_runner(FixtureRunner::new(HPSVM::new()), &fixture, &programs);

    let pass = runner.run_and_validate(
        &fixture,
        &ResultConfig {
            panic: false,
            verbose: true,
        },
    )?;

    if pass {
        println!("PASS: {}", path.display());
        Ok(())
    } else {
        eprintln!("FAIL: {}", path.display());
        std::process::exit(1);
    }
}
```

```rust
// bin/hpsvm-cli/src/main.rs
mod error;
mod fixture;
mod program_map;

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::fixture::{inspect_fixture, run_fixture};

#[derive(Parser)]
#[command(name = "hpsvm")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Fixture(FixtureArgs),
}

#[derive(Args)]
struct FixtureArgs {
    #[command(subcommand)]
    command: FixtureCommand,
}

#[derive(Subcommand)]
enum FixtureCommand {
    Inspect {
        fixture: PathBuf,
    },
    Run {
        fixture: PathBuf,
        #[arg(long = "program")]
        programs: Vec<String>,
    },
}

fn main() -> Result<(), error::CliError> {
    match Cli::parse().command {
        Command::Fixture(args) => match args.command {
            FixtureCommand::Inspect { fixture } => inspect_fixture(&fixture),
            FixtureCommand::Run { fixture, programs } => run_fixture(&fixture, &programs),
        },
    }
}
```

- [ ] **Step 4: Run focused validation**

Run: `cargo test -p hpsvm-cli --test fixture_inspect -- --nocapture`

Expected: PASS.

Run: `cargo test -p hpsvm-cli --test fixture_run -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml \
  bin/hpsvm-cli/Cargo.toml \
  bin/hpsvm-cli/src/main.rs \
  bin/hpsvm-cli/src/error.rs \
  bin/hpsvm-cli/src/fixture.rs \
  bin/hpsvm-cli/src/program_map.rs \
  bin/hpsvm-cli/tests/fixture_inspect.rs \
  bin/hpsvm-cli/tests/fixture_run.rs
git commit -m "feat: add fixture inspect and run cli commands"
```

## Task 6: Add `fixture compare`, Compare-Config Loading, and Phase 1 Release Validation

**Files:**

- Modify: `bin/hpsvm-cli/Cargo.toml`
- Modify: `bin/hpsvm-cli/src/main.rs`
- Modify: `bin/hpsvm-cli/src/error.rs`
- Modify: `bin/hpsvm-cli/src/fixture.rs`
- Create: `bin/hpsvm-cli/src/config.rs`
- Create: `bin/hpsvm-cli/tests/fixture_compare.rs`
- [ ] **Step 1: Write the failing test**

```rust
use std::process::Command;

use hpsvm::HPSVM;
use hpsvm_fixture::{CaptureBuilder, FixtureFormat, RuntimeFixtureConfig};
use hpsvm_result::{AccountSnapshot, Compare, ExecutionSnapshot};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::versioned::VersionedTransaction;

fn snapshot_account(svm: &HPSVM, address: Address) -> AccountSnapshot {
    let account = svm.get_account(&address).unwrap();
    AccountSnapshot {
        address,
        lamports: account.lamports,
        owner: account.owner,
        executable: account.executable,
        rent_epoch: account.rent_epoch,
        data: account.data,
    }
}

fn write_fixture() -> std::path::PathBuf {
    let mut svm = HPSVM::new();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = VersionedTransaction::from(solana_transaction::Transaction::new(
        &[&payer],
        Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
        svm.latest_blockhash(),
    ));
    let baseline = ExecutionSnapshot::from_outcome(&svm.transact(tx.clone()));
    let fixture = CaptureBuilder::new("cli-compare")
        .runtime(RuntimeFixtureConfig {
            slot: svm.block_env().slot,
            log_bytes_limit: None,
            sigverify: true,
            blockhash_check: false,
        })
        .pre_accounts(vec![
            snapshot_account(&svm, payer.pubkey()),
            snapshot_account(&svm, recipient),
        ])
        .baseline(baseline)
        .compares(Compare::everything())
        .capture_transaction(&tx)
        .unwrap();

    let fixture_path = std::env::temp_dir().join("hpsvm-cli-compare.json");
    fixture.save(&fixture_path, FixtureFormat::Json).unwrap();

    fixture_path
}

#[test]
fn fixture_compare_passes_for_identical_inputs() {
    let fixture = write_fixture();

    let output = Command::new(env!("CARGO_BIN_EXE_hpsvm"))
        .args(["fixture", "compare", fixture.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("PASS"));

    std::fs::remove_file(fixture).ok();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p hpsvm-cli --test fixture_compare fixture_compare_passes_for_identical_inputs -- --exact`

Expected: FAIL with `unexpected argument 'compare' found` or unresolved config module imports.

- [ ] **Step 3: Write minimal implementation**

```rust
// bin/hpsvm-cli/src/error.rs
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CliError {
    #[error(transparent)]
    Fixture(#[from] hpsvm_fixture::FixtureError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("failed to parse config {path}: {reason}")]
    ConfigParse { path: String, reason: String },
    #[error("unsupported config format for {path}")]
    UnsupportedConfigFormat { path: String },
    #[error("invalid --program value {value}, expected <program-id>=<path>")]
    InvalidProgramMapping { value: String },
    #[error("invalid program id {value}: {reason}")]
    InvalidProgramId { value: String, reason: String },
}
```

```rust
// bin/hpsvm-cli/src/config.rs
use std::path::Path;

use hpsvm_result::Compare;

use crate::error::CliError;

#[derive(Debug, serde::Deserialize)]
pub struct CompareConfigFile {
    pub compares: Vec<Compare>,
}

pub fn load_compares(
    path: Option<&Path>,
    fallback: &[Compare],
    ignore_compute_units: bool,
) -> Result<Vec<Compare>, CliError> {
    let mut compares = if let Some(path) = path {
        let file = std::fs::read_to_string(path)?;
        match path.extension().and_then(|value| value.to_str()) {
            Some("yaml") | Some("yml") => serde_yaml::from_str::<CompareConfigFile>(&file)
                .map(|config| config.compares)
                .map_err(|error| CliError::ConfigParse {
                    path: path.display().to_string(),
                    reason: error.to_string(),
                })?,
            Some("json") => serde_json::from_str::<CompareConfigFile>(&file)?.compares,
            _ => return Err(CliError::UnsupportedConfigFormat {
                path: path.display().to_string(),
            }),
        }
    } else {
        fallback.to_vec()
    };

    if ignore_compute_units {
        compares.retain(|compare| !matches!(compare, Compare::ComputeUnits));
    }

    Ok(compares)
}
```

```rust
// bin/hpsvm-cli/src/fixture.rs
use std::path::Path;

use hpsvm::HPSVM;
use hpsvm_fixture::{Fixture, FixtureRunner};
use hpsvm_result::ResultConfig;

use crate::{
    config::load_compares,
    error::CliError,
    program_map::{parse_program_map, preload_runner},
};

pub fn inspect_fixture(path: &Path) -> Result<(), CliError> {
    let fixture = Fixture::load(path)?;
    println!("{}", serde_json::to_string_pretty(&fixture)?);
    Ok(())
}

pub fn run_fixture(path: &Path, program_args: &[String]) -> Result<(), CliError> {
    let fixture = Fixture::load(path)?;
    let programs = parse_program_map(program_args)?;
    let mut runner = preload_runner(FixtureRunner::new(HPSVM::new()), &fixture, &programs);

    let pass = runner.run_and_validate(
        &fixture,
        &ResultConfig {
            panic: false,
            verbose: true,
        },
    )?;

    if pass {
        println!("PASS: {}", path.display());
        Ok(())
    } else {
        eprintln!("FAIL: {}", path.display());
        std::process::exit(1);
    }
}

pub fn compare_fixture(
    path: &Path,
    baseline_program_args: &[String],
    candidate_program_args: &[String],
    config_path: Option<&Path>,
    ignore_compute_units: bool,
) -> Result<(), CliError> {
    let fixture = Fixture::load(path)?;
    let compares = load_compares(config_path, &fixture.expectations.compares, ignore_compute_units)?;

    let baseline_programs = parse_program_map(baseline_program_args)?;
    let candidate_programs = parse_program_map(candidate_program_args)?;

    let mut baseline_runner = preload_runner(FixtureRunner::new(HPSVM::new()), &fixture, &baseline_programs);
    let mut candidate_runner = preload_runner(FixtureRunner::new(HPSVM::new()), &fixture, &candidate_programs);

    let baseline_snapshot = baseline_runner.run(&fixture)?.snapshot;
    let candidate_snapshot = candidate_runner.run(&fixture)?.snapshot;

    let pass = baseline_snapshot.compare_with(
        &candidate_snapshot,
        &compares,
        &ResultConfig {
            panic: false,
            verbose: true,
        },
    );

    if pass {
        println!("PASS: {}", path.display());
        Ok(())
    } else {
        eprintln!("FAIL: {}", path.display());
        std::process::exit(1);
    }
}
```

```rust
// bin/hpsvm-cli/src/main.rs
mod config;
mod error;
mod fixture;
mod program_map;

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::fixture::{compare_fixture, inspect_fixture, run_fixture};

#[derive(Parser)]
#[command(name = "hpsvm")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Fixture(FixtureArgs),
}

#[derive(Args)]
struct FixtureArgs {
    #[command(subcommand)]
    command: FixtureCommand,
}

#[derive(Subcommand)]
enum FixtureCommand {
    Inspect {
        fixture: PathBuf,
    },
    Run {
        fixture: PathBuf,
        #[arg(long = "program")]
        programs: Vec<String>,
    },
    Compare {
        fixture: PathBuf,
        #[arg(long = "baseline-program")]
        baseline_programs: Vec<String>,
        #[arg(long = "candidate-program")]
        candidate_programs: Vec<String>,
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        ignore_compute_units: bool,
    },
}

fn main() -> Result<(), error::CliError> {
    match Cli::parse().command {
        Command::Fixture(args) => match args.command {
            FixtureCommand::Inspect { fixture } => inspect_fixture(&fixture),
            FixtureCommand::Run { fixture, programs } => run_fixture(&fixture, &programs),
            FixtureCommand::Compare {
                fixture,
                baseline_programs,
                candidate_programs,
                config,
                ignore_compute_units,
            } => compare_fixture(
                &fixture,
                &baseline_programs,
                &candidate_programs,
                config.as_deref(),
                ignore_compute_units,
            ),
        },
    }
}
```

- [ ] **Step 4: Run focused validation and release gate**

Run: `cargo test -p hpsvm-cli --test fixture_compare -- --nocapture`

Expected: PASS.

Run: `cargo test -p hpsvm-result && cargo test -p hpsvm-fixture && cargo test -p hpsvm-cli`

Expected: PASS.

Run: `just lint && just test && just bdd && just test-all`

Expected: PASS. The new crates integrate cleanly with the repo-wide validation gate.

- [ ] **Step 5: Commit**

```bash
git add bin/hpsvm-cli/Cargo.toml \
    bin/hpsvm-cli/src/main.rs \
    bin/hpsvm-cli/src/error.rs \
  bin/hpsvm-cli/src/fixture.rs \
  bin/hpsvm-cli/src/config.rs \
  bin/hpsvm-cli/tests/fixture_compare.rs
git commit -m "feat: add fixture compare command"
```

## Follow-Up Plans

After this plan lands and the release gate passes, write separate plans for:

1. `hpsvm-bencher` and `hpsvm cu report`
2. `hpsvm` instruction-case convenience APIs
3. `hpsvm-fixture-fd` compatibility adapters
