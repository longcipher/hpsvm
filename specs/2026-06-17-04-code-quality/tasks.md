# Tasks: Code quality and documentation

Planned at commit `5ba1579` (2026-06-17).

## Phase 1: Extract execution pipeline from lib.rs (Finding 8)

### Task 1.1: Create execution.rs and extract execution functions

> **Context:** `lib.rs` is 2880 lines containing the entire execution pipeline, free functions, and HPSVM struct.
> **Verification:** All existing tests pass; `lib.rs` is reduced by ~800 lines.
> **Scenario Coverage:** `features/code-quality.feature` — "Transaction execution logic is extracted from lib.rs"

- **Loop Type:** `TDD-only`
- **Behavioral Contract:** `All existing behavior preserved; no public API changes`
- **Simplification Focus:** `Split god module into focused modules`
- **Status:** 🟢 DONE
- [x] Step 1: Create `crates/hpsvm/src/execution.rs`.
- [x] Step 2: Move these functions from `lib.rs` to `execution.rs`:
  - `execute_sanitized_transaction`
  - `execute_sanitized_transaction_readonly`
  - `execute_transaction`
  - `execute_transaction_no_verify`
  - `check_and_process_transaction`
  - `process_message`
  - `sanitize_transaction`
  - `sanitize_transaction_no_verify`
  - `map_sanitize_result`
  - Helper types: `CheckAndProcessTransactionSuccess`, `CheckAndProcessTransactionSuccessCore`
- [x] Step 3: Add `mod execution;` to `lib.rs`.
- [x] Step 4: Update `use` imports — make extracted items `pub(crate)`.
- [x] Step 5: Run `cargo check --all-features` to verify compilation.
- [x] Step 6: Run `cargo test --all-features` — all pass.
- [x] BDD Verification: `cargo test -p hpsvm --test bdd` — all pass
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `cargo check --all-features`

### Task 1.2: Create helpers.rs and extract free functions

> **Context:** Free functions in `lib.rs` are not part of the HPSVM struct impl.
> **Verification:** All functions compile and tests pass.
> **Scenario Coverage:** `features/code-quality.feature` — "Transaction execution logic is extracted"

- **Loop Type:** `TDD-only`
- **Behavioral Contract:** `All existing behavior preserved`
- **Simplification Focus:** `Separate free functions from struct methods`
- **Status:** 🟢 DONE
- [x] Step 1: Create `crates/hpsvm/src/helpers.rs`.
- [x] Step 2: Move these functions from `lib.rs` to `helpers.rs`:
  - `validate_fee_payer`
  - `fee_payer_for_instructions`
  - `token_balances`
  - `execution_trace_from_transaction_context`
  - `public_account_from_shared`
  - `execution_result_with_account_source_error`
  - `sanitize_error_into_execution_result`
  - `execute_tx_helper`
- [x] Step 3: Add `mod helpers;` to `lib.rs`.
- [x] Step 4: Update `use` imports — make extracted items `pub(crate)`.
- [x] Step 5: Run `cargo check --all-features`.
- [x] Step 6: Run `cargo test --all-features` — all pass.
- [x] BDD Verification: `cargo test -p hpsvm --test bdd` — all pass
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `cargo check --all-features`

## Phase 2: Unify duplicate methods (Finding 9)

### Task 2.1: Unify execute_sanitized_transaction variants

> **Context:** `execute_sanitized_transaction` and `execute_sanitized_transaction_readonly` are structurally identical.
> **Verification:** Single implementation handles both mutable and readonly paths.
> **Scenario Coverage:** `features/code-quality.feature` — "Duplicate execute_sanitized_transaction methods are unified"

- **Loop Type:** `TDD-only`
- **Behavioral Contract:** `All existing behavior preserved; callers now use the unified method`
- **Simplification Focus:** `Eliminate code duplication`
- **Status:** 🟢 DONE
- [x] Step 1: Confirm `check_and_process_transaction` takes `&self` (not `&mut self`) — it already does (line 1810).
- [x] Step 2: Create `execute_sanitized_transaction_impl(&self, sanitized_tx, log_collector) -> ExecutionResult` in `execution.rs`.
- [x] Step 3: Replace both `execute_sanitized_transaction` and `execute_sanitized_transaction_readonly` with delegations to `_impl`.
- [x] Step 4: Update callers (`execute_transaction`, `execute_transaction_no_verify`, etc.) to use the unified method.
- [x] Step 5: Remove `execute_sanitized_transaction_readonly`.
- [x] Step 6: Run `cargo test --all-features` — all pass.
- [x] BDD Verification: `cargo test -p hpsvm --test bdd` — all pass
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `cargo check --all-features`

## Phase 3: Deduplicate token builder boilerplate (Finding 10)

### Task 3.1: Create shared sign_and_send helper in token crate

> **Context:** 20+ token builders repeat ~25 lines of identical transaction construction.
> **Verification:** All token builder tests pass with the new helper.
> **Scenario Coverage:** `features/code-quality.feature` — "Token builder send() boilerplate is deduplicated"

- **Loop Type:** `TDD-only`
- **Behavioral Contract:** `Public API unchanged; each builder's send() produces identical results`
- **Simplification Focus:** `Extract shared transaction construction logic`
- **Status:** 🟢 DONE
- [x] Step 1: Add `sign_and_send` helper to `crates/token/src/lib.rs`.
- [x] Step 2: Update `crates/token/src/approve.rs` `send()` to use the helper.
- [x] Step 3: Run `cargo test -p hpsvm-token` — all pass.
- [x] Step 4: Update remaining builders: `approve_checked`, `burn`, `burn_checked`, `close_account`, `mint_to`, `mint_to_checked`, `transfer`, `transfer_checked`, `freeze_account`, `revoke`, `set_authority`, `thaw_account`, `create_ata`, `create_ata_idempotent`, `create_native_mint_2022`, `sync_native`.
- [x] Step 5: Run `cargo test -p hpsvm-token --all-features` — all pass.
- [x] BDD Verification: N/A — token crate tests
- [x] Advanced Test Verification: `cargo test -p hpsvm-token --all-features` — all pass
- [x] Runtime Verification: `cargo check --all-features`

**Note:** `create_account.rs`, `create_mint.rs`, `create_multisig.rs` unchanged — they send multi-instruction transactions with extra keypairs, which is a different pattern.

## Phase 4: Add documentation to public types (Finding 11)

### Task 4.1: Document types.rs public structs and fields

> **Context:** 21 `#[expect(missing_docs)]` suppressions on public API types.
> **Verification:** `cargo doc --no-deps` produces complete docs; no `missing_docs` warnings.
> **Scenario Coverage:** `features/code-quality.feature` — "Public types in types.rs have complete documentation"

- **Loop Type:** `TDD-only`
- **Behavioral Contract:** `N/A — documentation only`
- **Simplification Focus:** `Fulfill "Well Documented" promise in README`
- **Status:** 🟢 DONE
- [x] Step 1: Remove `#[expect(missing_docs)]` from `TransactionMetadata` (line 12). Add doc comment.
- [x] Step 2: Add doc comments to all `TransactionMetadata` fields: `signature`, `logs`, `inner_instructions`, `compute_units_consumed`, `return_data`, `fee`, `diagnostics`.
- [x] Step 3: Remove `#[expect(missing_docs)]` from `ExecutionDiagnostics` (line 35). Add doc comment and field docs.
- [x] Step 4: Repeat for `AccountSourceFailure`, `AccountDiff`, `TokenBalance`, `ExecutionTrace`, `ExecutedInstruction`, `SimulatedTransactionInfo`, `FailedTransactionMetadata`, `TransactionResult`.
- [x] Step 5: Remove `#[expect(missing_docs)]` from `pretty_logs` method (line 28).
- [x] Step 6: Edit `crates/hpsvm/src/lib.rs:387-394` — remove module-level `#[expect(missing_docs)]` for `batch`, `error`, `instruction`, `types`. Add `//!` doc comments to each module.
- [x] Step 7: Run `cargo doc --no-deps` — verify no warnings.
- [x] BDD Verification: N/A — documentation
- [x] Advanced Test Verification: `cargo doc --no-deps` — clean
- [x] Runtime Verification: `cargo check --all-features`
