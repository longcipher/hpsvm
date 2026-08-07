# Tasks: Batch execution performance

Planned at commit `5ba1579` (2026-06-17).

## Phase 1: Arc-based batch sharing (Finding 5)

### Task 1.1: Wrap AccountsDb in Arc for batch workers

> **Context:** Each worker in `send_transaction_batch` deep-clones the entire `AccountsDb`. With N transactions, N full clones are created.
> **Verification:** Batch execution produces identical results; no per-worker AccountsDb clones.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `Batch execution must produce identical transaction results, account states, and error outcomes`
- **Simplification Focus:** `Replace N deep clones with 1 Arc + N small deltas`
- **Status:** 🟢 DONE
- [x] Step 1: Write a test that runs a batch of 10 conflict-free transactions and asserts all balances match expected values.
- [x] Step 2: RED — run test, confirm it passes (baseline).
- [x] Step 3: Edit `crates/hpsvm/src/batch.rs` — modify `BatchExecutionSnapshot` to wrap `AccountsDb` in `Arc`:

  ```rust
  pub(crate) struct BatchExecutionSnapshot {
      accounts: Arc<AccountsDb>,
      // ... other fields
  }
  ```

- [x] Step 4: Modify `execute_batch_stage` (line 228) — give each worker `Arc::clone(&snapshot.accounts)` instead of `snapshot.clone()`.
- [x] Step 5: Modify `BatchStageResult::new` to accept `Arc<AccountsDb>` and return `Vec<(Address, AccountSharedData)>` delta instead of mutating a clone.
- [x] Step 6: Modify the staging loop to collect deltas and apply via `apply_commit_delta`.
- [x] Step 7: GREEN — run test, confirm identical results.
- [x] Step 8: Run full test suite to verify no regressions.
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `just bench-runtime` — no regression

## Phase 2: Opt-in diagnostics (Finding 6)

### Task 2.1: Add compute_diagnostics flag to SvmCfg

> **Context:** Every `send_transaction`/`transact` call unconditionally computes full diagnostics (pre/post diffs, token balances).
> **Verification:** Diagnostics are skipped when flag is false; full diagnostics when true.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `Default behavior preserves diagnostics (compute_diagnostics: true) for backward compatibility`
- **Simplification Focus:** `Make expensive computation opt-in`
- **Status:** 🟢 DONE
- [x] Step 1: Edit `crates/hpsvm/src/lib.rs` — add `pub compute_diagnostics: bool` to `SvmCfg` struct, default `true`.
- [x] Step 2: Write a test that disables diagnostics and asserts `execution_diagnostics` is not called (use a counter or verify `ExecutionDiagnostics` is default).
- [x] Step 3: RED — run test, confirm it fails (diagnostics still computed).
- [x] Step 4: Edit `crates/hpsvm/src/lib.rs` `execution_into_outcome` — when `vm.cfg.compute_diagnostics` is false, return `ExecutionDiagnostics::default()` instead of calling `execution_diagnostics(...)`.
- [x] Step 5: GREEN — run test, confirm it passes.
- [x] Step 6: Verify default behavior: existing tests pass with `compute_diagnostics: true` (default).
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `just bench-runtime` — no regression
