# HPSVM Core Architecture Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce a staged, revm-inspired architecture in `hpsvm` that unifies execution output, separates state sources from local overlays, and opens stable seams for RPC fork state, richer inspection, and runtime extensions without breaking the current ergonomic API.

**Architecture:** Keep `HPSVM` as the public facade. First, lift transaction execution onto a single `transact -> commit` core so `send_transaction`, `simulate_transaction`, and batch execution share one result model. Second, split `AccountsDb` into a local mutable overlay plus a read-through `AccountSource` boundary. Third, extract internal environment/config structs, then add inspector and runtime-registry adapters incrementally so existing APIs remain source-compatible.

**Tech Stack:** Rust 2024 workspace, `hpsvm` core crate, a new `hpsvm-fork-rpc` companion crate under `crates/`, existing Solana runtime crates, `cargo test`, and repo-wide `just` validation commands.

**Status:** The phased refactor described here is now implemented in-tree. `ExecutionOutcome`, `AccountSource`, `RpcForkSource`, `BlockEnv`, `Inspector`, and `RuntimeExtensionRegistry` all landed behind the existing `HPSVM` facade; treat Task 7's validation sequence as the release gate before shipping.

---

This plan is intentionally phased. Each task is independently mergeable and leaves the crate in a releasable state. Do not start Task 4 before Task 3 lands. Do not start Task 6 before Tasks 1 and 5 land.

## Task 1: Unify Execution Around `ExecutionOutcome`

**Files:**

- Create: `crates/hpsvm/tests/execution_outcome.rs`
- Modify: `crates/hpsvm/src/types.rs`
- Modify: `crates/hpsvm/src/lib.rs`
- Modify: `crates/hpsvm/src/batch.rs`
- [ ] **Step 1: Write the failing test**

```rust
use hpsvm::HPSVM;
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::Transaction;

#[test]
fn transact_returns_state_without_committing_it() {
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

    assert!(outcome.status.is_ok());
    assert_eq!(svm.get_balance(&recipient), None);
    assert!(outcome
        .post_accounts
        .iter()
        .any(|(key, account)| key == &recipient && account.lamports() == 64));
}

#[test]
fn commit_transaction_applies_a_transacted_outcome() {
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
    let result = svm.commit_transaction(outcome);

    assert!(result.is_ok());
    assert_eq!(svm.get_balance(&recipient), Some(64));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p hpsvm --test execution_outcome transact_returns_state_without_committing_it -- --exact`

Expected: FAIL with a compile error because `HPSVM::transact` and `HPSVM::commit_transaction` do not exist.

- [ ] **Step 3: Write minimal implementation**

```rust
// crates/hpsvm/src/types.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionOutcome {
    pub meta: TransactionMetadata,
    pub post_accounts: Vec<(Address, AccountSharedData)>,
    pub status: solana_transaction_error::TransactionResult<()>,
    pub included: bool,
}

// crates/hpsvm/src/lib.rs
impl HPSVM {
    pub fn transact(&self, tx: impl Into<VersionedTransaction>) -> ExecutionOutcome {
        let log_collector = Rc::new(RefCell::new(LogCollector {
            bytes_limit: self.log_bytes_limit,
            ..Default::default()
        }));
        let execution = if self.sigverify {
            self.execute_transaction_readonly(tx.into(), log_collector.clone())
        } else {
            self.execute_transaction_no_verify_readonly(tx.into(), log_collector.clone())
        };
        execution_into_outcome(execution, log_collector)
    }

    pub fn commit_transaction(&mut self, outcome: ExecutionOutcome) -> TransactionResult {
        commit_execution_outcome(self, outcome)
    }
}
```

- [ ] **Step 4: Run focused validation**

Run: `cargo test -p hpsvm --test execution_outcome -- --nocapture`

Run: `cargo test -p hpsvm --test transaction_batch send_transaction_batch_executes_transactions_and_returns_results_in_input_order -- --exact`

Expected: PASS. The new outcome API works and existing batch behavior still compiles against the refactored internals.

- [ ] **Step 5: Commit**

```bash
git add crates/hpsvm/src/types.rs crates/hpsvm/src/lib.rs crates/hpsvm/src/batch.rs crates/hpsvm/tests/execution_outcome.rs
git commit -m "feat: add explicit execution outcome API"
```

### Task 2: Rebase Single and Batch Commits on One Delta Path

**Files:**

- Modify: `crates/hpsvm/src/accounts_db.rs`
- Modify: `crates/hpsvm/src/batch.rs`
- Modify: `crates/hpsvm/src/lib.rs`
- Modify: `crates/hpsvm/tests/transaction_batch.rs`
- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn sequential_commit_matches_batch_commit() {
    let mut serial_vm = HPSVM::new();
    let mut batch_vm = HPSVM::new();
    let payer_a = solana_keypair::Keypair::new();
    let payer_b = solana_keypair::Keypair::new();
    let recipient_a = solana_address::Address::new_unique();
    let recipient_b = solana_address::Address::new_unique();

    for svm in [&mut serial_vm, &mut batch_vm] {
        svm.airdrop(&payer_a.pubkey(), 1_000_000_000).unwrap();
        svm.airdrop(&payer_b.pubkey(), 1_000_000_000).unwrap();
    }

    let blockhash = serial_vm.latest_blockhash();
    let tx_a = transfer_tx(&payer_a, &recipient_a, 10, blockhash);
    let tx_b = transfer_tx(&payer_b, &recipient_b, 20, blockhash);

    let outcome_a = serial_vm.transact(tx_a.clone());
    serial_vm.commit_transaction(outcome_a).unwrap();
    let outcome_b = serial_vm.transact(tx_b.clone());
    serial_vm.commit_transaction(outcome_b).unwrap();

    let batch = batch_vm.send_transaction_batch([tx_a, tx_b]).unwrap();

    assert!(batch.results.iter().all(Result::is_ok));
    assert_eq!(serial_vm.get_balance(&recipient_a), batch_vm.get_balance(&recipient_a));
    assert_eq!(serial_vm.get_balance(&recipient_b), batch_vm.get_balance(&recipient_b));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p hpsvm --test transaction_batch sequential_commit_matches_batch_commit -- --exact`

Expected: FAIL because batch execution and single-transaction commit still build different delta objects and do not yet share one commit helper.

- [ ] **Step 3: Write minimal implementation**

```rust
// crates/hpsvm/src/lib.rs
struct CommitDelta {
    post_accounts: Vec<(Address, AccountSharedData)>,
    history_entry: Option<(Signature, TransactionResult)>,
}

fn commit_delta(vm: &mut HPSVM, delta: CommitDelta) -> Result<(), HPSVMError> {
    vm.accounts.sync_accounts(delta.post_accounts)?;
    if let Some((signature, entry)) = delta.history_entry {
        vm.history.add_new_transaction(signature, entry);
    }
    Ok(())
}

// crates/hpsvm/src/batch.rs
// Replace BatchExecutionDelta with the shared CommitDelta and route merge_into_vm through commit_delta.
```

- [ ] **Step 4: Run focused validation**

Run: `cargo test -p hpsvm --test transaction_batch -- --nocapture`

Expected: PASS. Batch execution now reuses the same delta/commit path as `transact + commit_transaction`.

- [ ] **Step 5: Commit**

```bash
git add crates/hpsvm/src/accounts_db.rs crates/hpsvm/src/lib.rs crates/hpsvm/src/batch.rs crates/hpsvm/tests/transaction_batch.rs
git commit -m "refactor: share commit delta across single and batch execution"
```

### Task 3: Add an `AccountSource` Boundary Under `AccountsDb`

**Files:**

- Create: `crates/hpsvm/src/account_source.rs`
- Create: `crates/hpsvm/tests/account_source.rs`
- Modify: `crates/hpsvm/src/accounts_db.rs`
- Modify: `crates/hpsvm/src/lib.rs`
- [ ] **Step 1: Write the failing test**

```rust
use std::collections::HashMap;
use std::sync::Arc;

use hpsvm::{AccountSource, HPSVM};
use solana_account::AccountSharedData;
use solana_address::Address;

#[derive(Clone, Default)]
struct StaticAccountSource {
    accounts: Arc<HashMap<Address, AccountSharedData>>,
}

impl AccountSource for StaticAccountSource {
    fn get_account(
        &self,
        pubkey: &Address,
    ) -> Result<Option<AccountSharedData>, hpsvm::AccountSourceError> {
        Ok(self.accounts.get(pubkey).cloned())
    }
}

#[test]
fn vm_reads_missing_accounts_from_the_configured_source() {
    let address = Address::new_unique();
    let mut account = AccountSharedData::default();
    account.set_lamports(77);
    let source = StaticAccountSource {
        accounts: Arc::new(HashMap::from([(address, account.clone())])),
    };

    let svm = HPSVM::default().with_account_source(source);

    assert_eq!(svm.get_account(&address).unwrap().lamports, 77);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p hpsvm --test account_source vm_reads_missing_accounts_from_the_configured_source -- --exact`

Expected: FAIL because `AccountSource` and `HPSVM::with_account_source` do not exist.

- [ ] **Step 3: Write minimal implementation**

```rust
// crates/hpsvm/src/account_source.rs
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
    fn get_account(&self, pubkey: &Address) -> Result<Option<AccountSharedData>, AccountSourceError>;
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

// crates/hpsvm/src/accounts_db.rs
pub(crate) struct AccountsDb {
    source: Arc<dyn AccountSource>,
    inner: HashMap<Address, AccountSharedData>,
    // keep the existing caches and environments unchanged in this task
}
```

- [ ] **Step 4: Run focused validation**

Run: `cargo test -p hpsvm --test account_source -- --nocapture`

Run: `cargo test -p hpsvm --test accounts_view -- --nocapture`

Expected: PASS. Missing accounts are resolved through the source boundary, while the existing in-memory behavior remains unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/hpsvm/src/account_source.rs crates/hpsvm/src/accounts_db.rs crates/hpsvm/src/lib.rs crates/hpsvm/tests/account_source.rs
git commit -m "feat: add read-through account source boundary"
```

### Task 4: Add a Feature-Gated RPC Fork Companion Crate

**Files:**

- Create: `crates/fork-rpc/Cargo.toml`
- Create: `crates/fork-rpc/src/lib.rs`
- Create: `crates/fork-rpc/tests/rpc_fork.rs`
- Modify: `Cargo.toml`
- Modify: `README.md`
- [ ] **Step 1: Write the failing test**

```rust
use hpsvm::HPSVM;
use hpsvm_fork_rpc::RpcForkSource;
use solana_address::Address;

#[test]
fn rpc_fork_source_serves_cached_accounts_without_refetching() {
    let source = RpcForkSource::builder()
        .with_rpc_url("http://127.0.0.1:8899")
        .with_slot(1)
        .build();

    let vm = HPSVM::default().with_account_source(source.clone());
    let key = Address::new_unique();

    let _ = vm.get_account(&key);
    let _ = vm.get_account(&key);

    assert_eq!(source.cache_hits() + source.cache_misses(), 2);
    assert_eq!(source.cache_misses(), 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p hpsvm-fork-rpc --test rpc_fork rpc_fork_source_serves_cached_accounts_without_refetching -- --exact`

Expected: FAIL because the new crate and builder do not exist.

- [ ] **Step 3: Write minimal implementation**

```rust
// crates/fork-rpc/src/lib.rs
#[derive(Clone)]
pub struct RpcForkSource {
    client: solana_rpc_client::rpc_client::RpcClient,
    slot: u64,
    cache: Arc<parking_lot::Mutex<HashMap<Address, AccountSharedData>>>,
    cache_hits: Arc<AtomicUsize>,
    cache_misses: Arc<AtomicUsize>,
}

impl RpcForkSource {
    pub fn builder() -> RpcForkSourceBuilder {
        RpcForkSourceBuilder::default()
    }
}

impl hpsvm::AccountSource for RpcForkSource {
    fn get_account(
        &self,
        pubkey: &Address,
    ) -> Result<Option<AccountSharedData>, hpsvm::AccountSourceError> {
        if let Some(account) = self.cache.lock().get(pubkey).cloned() {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(Some(account));
        }
        self.cache_misses.fetch_add(1, Ordering::Relaxed);
        fetch_and_cache_account(self, pubkey).map_err(|error| hpsvm::AccountSourceError::new(error.to_string()))
    }
}
```

- [ ] **Step 4: Run focused validation**

Run: `cargo test -p hpsvm-fork-rpc --test rpc_fork -- --nocapture`

Expected: PASS against a local RPC fixture or mocked client. The source caches fetched accounts and exposes deterministic counters for regression tests.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/fork-rpc/Cargo.toml crates/fork-rpc/src/lib.rs crates/fork-rpc/tests/rpc_fork.rs README.md
git commit -m "feat: add rpc fork account source crate"
```

### Task 5: Extract Internal `SvmEnv` and `SvmCfg` From `HPSVM`

**Files:**

- Create: `crates/hpsvm/src/env.rs`
- Create: `crates/hpsvm/tests/env_config.rs`
- Modify: `crates/hpsvm/src/lib.rs`
- Modify: `crates/hpsvm/src/accounts_db.rs`
- [ ] **Step 1: Write the failing test**

```rust
use hpsvm::HPSVM;
use solana_clock::Clock;

#[test]
fn warp_to_slot_updates_block_env_and_clock_sysvar() {
    let mut svm = HPSVM::new();

    svm.warp_to_slot(42);

    assert_eq!(svm.get_sysvar::<Clock>().slot, 42);
    assert_eq!(svm.block_env().slot, 42);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p hpsvm --test env_config warp_to_slot_updates_block_env_and_clock_sysvar -- --exact`

Expected: FAIL because `HPSVM::block_env` does not exist and block state is still spread across top-level fields.

- [ ] **Step 3: Write minimal implementation**

```rust
// crates/hpsvm/src/env.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockEnv {
    pub latest_blockhash: Hash,
    pub slot: u64,
}

#[derive(Debug, Clone)]
pub struct SvmCfg {
    pub feature_set: FeatureSet,
    pub sigverify: bool,
    pub blockhash_check: bool,
    pub fee_structure: FeeStructure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeEnv {
    pub compute_budget: Option<ComputeBudget>,
    pub log_bytes_limit: Option<usize>,
}
```

- [ ] **Step 4: Run focused validation**

Run: `cargo test -p hpsvm --test env_config -- --nocapture`

Run: `cargo test -p hpsvm --test compute_budget -- --nocapture`

Expected: PASS. Public behavior is unchanged, but internal configuration is no longer a flat field pile inside `HPSVM`.

- [ ] **Step 5: Commit**

```bash
git add crates/hpsvm/src/env.rs crates/hpsvm/src/lib.rs crates/hpsvm/src/accounts_db.rs crates/hpsvm/tests/env_config.rs
git commit -m "refactor: extract internal svm env structs"
```

### Task 6: Introduce `Inspector` and a Runtime Extension Registry

**Files:**

- Create: `crates/hpsvm/src/inspector.rs`
- Create: `crates/hpsvm/src/runtime_registry.rs`
- Create: `crates/hpsvm/tests/inspector.rs`
- Modify: `crates/hpsvm/src/lib.rs`
- Modify: `crates/hpsvm/src/message_processor.rs`
- Modify: `crates/hpsvm/src/register_tracing.rs`
- Modify: `crates/hpsvm/src/precompiles.rs`
- Modify: `crates/hpsvm/tests/custom_syscall.rs`
- [ ] **Step 1: Write the failing tests**

```rust
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

use hpsvm::{HPSVM, Inspector};

#[derive(Default)]
struct CountingInspector {
    top_level_instructions: Arc<AtomicUsize>,
}

impl Inspector for CountingInspector {
    fn on_instruction(&self, _svm: &HPSVM, _index: usize, _program_id: &solana_address::Address) {
        self.top_level_instructions.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn inspector_observes_top_level_instructions() {
    let inspector = CountingInspector::default();
    let observed = Arc::clone(&inspector.top_level_instructions);
    let mut svm = HPSVM::new().with_inspector(inspector);
    let payer = solana_keypair::Keypair::new();

    svm.airdrop(&payer.pubkey(), 10_000).unwrap();
    let tx = solana_transaction::Transaction::new(
        &[&payer],
        solana_message::Message::new(
            &[solana_system_interface::instruction::transfer(
                &payer.pubkey(),
                &solana_address::Address::new_unique(),
                1,
            )],
            Some(&payer.pubkey()),
        ),
        svm.latest_blockhash(),
    );

    svm.send_transaction(tx).unwrap();
    assert_eq!(observed.load(Ordering::SeqCst), 1);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p hpsvm --test inspector inspector_observes_top_level_instructions -- --exact`

Expected: FAIL because `Inspector` and `HPSVM::with_inspector` do not exist.

- [ ] **Step 3: Write minimal implementation**

```rust
// crates/hpsvm/src/inspector.rs
pub trait Inspector: Send + Sync {
    fn on_transaction_start(&self, _svm: &HPSVM, _tx: &SanitizedTransaction) {}
    fn on_instruction(&self, _svm: &HPSVM, _index: usize, _program_id: &Address) {}
    fn on_transaction_end(&self, _svm: &HPSVM, _result: &solana_transaction_error::TransactionResult<()>) {}
}

// crates/hpsvm/src/runtime_registry.rs
#[derive(Clone, Default, Debug)]
pub(crate) struct RuntimeExtensionRegistry {
    pub custom_syscalls: Vec<CustomSyscallRegistration>,
    #[cfg(feature = "precompiles")]
    pub load_standard_precompiles: bool,
}
```

- [ ] **Step 4: Run focused validation**

Run: `cargo test -p hpsvm --test inspector -- --nocapture`

Run: `cargo test -p hpsvm --test custom_syscall -- --nocapture`

Run: `cargo test -p hpsvm --test counter_test -- --nocapture`

Expected: PASS. The new inspector path captures top-level instruction events, register tracing adapts through the new hook layer, and custom syscalls/precompiles are materialized through one registry-owned refresh path.

- [ ] **Step 5: Commit**

```bash
git add crates/hpsvm/src/inspector.rs crates/hpsvm/src/runtime_registry.rs crates/hpsvm/src/lib.rs crates/hpsvm/src/message_processor.rs crates/hpsvm/src/register_tracing.rs crates/hpsvm/src/precompiles.rs crates/hpsvm/tests/inspector.rs crates/hpsvm/tests/custom_syscall.rs
git commit -m "feat: add inspector and runtime extension registry"
```

### Task 7: Update Docs and Run Full Validation Gates

**Files:**

- Modify: `README.md`
- Modify: `CHANGELOG.md`
- Modify: `docs/superpowers/plans/2026-04-28-hpsvm-core-architecture.md`
- [ ] **Step 1: Write the failing docs check**

```markdown
## New public API to document

- `HPSVM::transact`
- `HPSVM::commit_transaction`
- `HPSVM::with_account_source`
- `hpsvm-fork-rpc::RpcForkSource`
- `HPSVM::block_env`
- `HPSVM::with_inspector`
```

- [ ] **Step 2: Run the narrow checks before editing docs**

Run: `cargo doc --no-deps -p hpsvm`

Expected: PASS, but the new APIs are undocumented or under-documented relative to the new architecture.

- [ ] **Step 3: Write the docs updates**

```markdown
## Architecture highlights

`hpsvm` now exposes a two-step execution flow: `transact` computes an `ExecutionOutcome` without mutating the VM, and `commit_transaction` applies it explicitly. Advanced users can swap account sources, including a cached RPC fork source, without losing the default in-memory workflow.

The VM keeps a stable `HPSVM` facade while internally separating block/config/runtime state and exposing richer inspection hooks for tracing and profiling.
```

- [ ] **Step 4: Run full repo validation in order**

Run: `just format`

Run: `just lint`

Run: `just test`

Run: `just bdd`

Run: `just test-all`

Expected: PASS on all five commands. Run them sequentially, never in parallel, because the workspace is Cargo-based.

- [ ] **Step 5: Commit**

```bash
git add README.md CHANGELOG.md docs/superpowers/plans/2026-04-28-hpsvm-core-architecture.md
git commit -m "docs: document hpsvm core architecture refactor"
```

## Self-Review Notes

- Spec coverage: The plan covers the three highest-value refactors first (`ExecutionOutcome`, `AccountSource`, and internal env extraction), then stages the heavier fork and inspection work behind those seams.
- Placeholder scan: No `TODO`, `TBD`, or undefined hand-waves remain. Every task names concrete files, tests, commands, and target types.
- Type consistency: The plan uses one shared vocabulary throughout: `ExecutionOutcome`, `commit_transaction`, `AccountSource`, `RpcForkSource`, `BlockEnv`, `SvmCfg`, `Inspector`, and `RuntimeExtensionRegistry`.

## Recommended Execution Order

1. Finish Tasks 1 and 2 in one branch before opening Task 3.
2. Merge Task 3 before starting the RPC fork crate in Task 4.
3. Merge Task 5 before Task 6 so the inspector/registry work lands on a cleaner internal layout.
4. Treat Task 7 as the release gate after the code refactors are green.
