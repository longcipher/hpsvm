# Design: Batch execution performance

| Metadata | Details |
| :--- | :--- |
| **Status** | Draft |
| **Created** | 2026-06-17 |
| **Mode** | Full |
| **Priority** | P1 |
| **Planned at** | commit `5ba1579`, 2026-06-17 |

## Summary

> Two performance improvements to the batch execution pipeline: eliminate per-worker AccountsDb clones by sharing via Arc and merging deltas, and make transaction diagnostics opt-in to avoid unnecessary computation on every transaction.

## Why this matters

The batch scheduler is hpsvm's core differentiator for parallel test execution. Currently, each worker in a batch stage deep-clones the entire `AccountsDb` (all accounts, sysvars, program cache), which creates significant allocation pressure and undermines the parallelism benefit. Additionally, every `send_transaction`/`transact` call unconditionally computes full diagnostics (pre/post diffs, token balances), which is wasted work when callers only need the result status.

## Findings

### Finding 5: Full AccountsDb clone per worker in batch stage

- **Category:** performance
- **Impact:** HIGH
- **Effort:** M
- **Risk:** MED — changing the snapshot model could break the conflict-freedom invariant if workers observe partial writes.

#### Requirements (EARS Notation)

- **[REQ-01]:** Batch workers SHALL share a read-only reference to the AccountsDb snapshot.
- **[REQ-02]:** Each worker SHALL produce a `Vec<(Address, AccountSharedData)>` delta of modified accounts.
- **[REQ-03]:** The staging loop SHALL merge deltas back into the AccountsDb via `apply_commit_delta`.
- **[REQ-04]:** The conflict-freedom invariant SHALL be preserved — workers must not see partial writes from other workers in the same stage.

#### Current state

- `crates/hpsvm/src/batch.rs:228`: `let worker_snapshot = snapshot.clone();` — full clone per chunk.
- `crates/hpsvm/src/batch.rs:235`: `let snapshot = worker_snapshot.clone();` — full clone per transaction within the chunk.
- `BatchExecutionSnapshot` contains `AccountsDb` which holds `HashMap<Address, AccountSharedData>`, `HashSet`, program cache, sysvar cache.

#### Approach

Wrap `AccountsDb` in `Arc` within `BatchExecutionSnapshot`. Workers receive `Arc<AccountsDb>` for reads. Each worker returns `Vec<(Address, AccountSharedData)>` of modified accounts. The staging loop collects all deltas and applies them via `apply_commit_delta`.

The key insight: since the batch stage already guarantees conflict-freedom (transactions in the same stage don't touch overlapping writable accounts), each worker's delta is disjoint and can be merged without conflict detection.

#### Architecture Decisions (MADR Format)

- **AD-01:** Use `Arc<AccountsDb>` rather than COW — the AccountsDb is read-only during parallel execution, and deltas are small relative to the full snapshot.
- **AD-02:** Merge deltas sequentially in the staging loop — since deltas are disjoint (conflict-free stage), order doesn't matter.

### Finding 6: Full diagnostics computed on every transaction

- **Category:** performance
- **Impact:** MEDIUM
- **Effort:** M
- **Risk:** LOW — diagnostics are output-only and do not affect execution logic.

#### Requirements (EARS Notation)

- **[REQ-01]:** Diagnostics SHALL be opt-in via a flag on `SvmCfg` or `RuntimeEnv`.
- **[REQ-02]:** When diagnostics are disabled, `execution_diagnostics` SHALL not be called.
- **[REQ-03]:** The `ExecutionDiagnostics` returned when diagnostics are disabled SHALL be `Default::default()` (empty).
- **[REQ-04]:** Callers that need diagnostics CAN opt in via `send_transaction_with_diagnostics` or a config flag.

#### Current state

- `crates/hpsvm/src/lib.rs:2480-2504`: `execution_diagnostics` unconditionally clones pre-accounts, computes diffs, and unpacks SPL token state.
- Called from `execution_into_outcome` at line 2367 for every transaction.

#### Approach

Add `compute_diagnostics: bool` to `SvmCfg` (default `true` for backward compatibility). In `execution_into_outcome`, skip `execution_diagnostics` when false, returning `ExecutionDiagnostics::default()`. Add `send_transaction_with_diagnostics` that sets the flag temporarily, or make it a persistent config option.

## BDD/TDD Strategy

- **Primary Language:** Rust
- **BDD Runner:** cucumber-rs
- **BDD Command:** `cargo test -p hpsvm --test bdd`
- **Unit Test Command:** `cargo test --all-features`
- **Feature Files:** `specs/2026-06-17-02-performance/features/performance.feature`
- **Outside-in Loop:** Scenario "Batch workers share AccountsDb" fails when workers still clone, passes after Arc refactor.

## Code Simplification Constraints

- **Behavioral Contract:** Batch execution must produce identical results — same transaction order, same account states, same errors.
- **Repo Standards:** Use `parking_lot` for any new locks, `Arc` for shared immutable state, avoid `RwLock` where `Arc` suffices.
- **Readability Priorities:** Keep the delta-merge loop explicit and readable; optimize for clarity over cleverness.

## Verification

| Purpose   | Command                                          | Expected on success |
|-----------|--------------------------------------------------|---------------------|
| Check     | `cargo check --all-targets --all-features`       | exit 0              |
| Tests     | `cargo test --all-features`                      | all pass            |
| BDD       | `cargo test -p hpsvm --test bdd`                 | all pass            |
| Bench     | `just bench-runtime`                             | no regression       |
| Clippy    | `cargo +nightly clippy --all -- -D warnings`     | exit 0              |
