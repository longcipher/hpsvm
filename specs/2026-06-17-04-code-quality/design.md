# Design: Code quality and documentation

| Metadata | Details |
| :--- | :--- |
| **Status** | Draft |
| **Created** | 2026-06-17 |
| **Mode** | Full |
| **Priority** | P2 |
| **Planned at** | commit `5ba1579`, 2026-06-17 |

## Summary

> Structural improvements to the codebase: split the 2880-line `lib.rs` god module, unify duplicate method pairs, deduplicate token builder boilerplate, and add documentation to public API types. These changes improve maintainability and fulfill the README's "Well Documented" promise.

## Why this matters

`lib.rs` is 4.3x the next-largest file and contains the entire execution pipeline, free functions, and the HPSVM struct. Every change to the VM core touches one file, making diffs hard to review and merge conflicts frequent. The token crate has 20+ builders that copy-paste ~25 lines of identical transaction construction code. The public API types in `types.rs` have 21 `#[expect(missing_docs)]` suppressions, making docs.rs unusable for the primary return types.

## Findings

### Finding 8: lib.rs is a 2880-line god module

- **Category:** tech debt
- **Impact:** HIGH
- **Effort:** L
- **Risk:** MED — moving code can change module-private visibility; careful `pub(crate)` management needed.

#### Requirements (EARS Notation)

- **[REQ-01]:** The transaction execution pipeline SHALL be extracted to `crates/hpsvm/src/execution.rs`.
- **[REQ-02]:** Free functions SHALL be extracted to `crates/hpsvm/src/helpers.rs`.
- **[REQ-03]:** The `HPSVM` struct and its public API methods SHALL remain in `lib.rs`.
- **[REQ-04]:** All existing tests SHALL continue to pass after extraction.
- **[REQ-05]:** Public API surface SHALL NOT change (no breaking changes for downstream users).

#### Current state

- `crates/hpsvm/src/lib.rs` — 2880 lines containing:
  - `HPSVM` struct definition (lines 427-455)
  - Manual `Clone` impl (lines 479-504)
  - Manual `Debug` impl (lines 457-477)
  - `impl HPSVM` block (~1800 lines of methods)
  - Free functions: `validate_fee_payer`, `execution_into_outcome`, `commit_execution_outcome`, `fee_payer_for_instructions`, `token_balances`, `execution_trace_from_transaction_context`, etc.
  - `InvocationInspectCallback` trait
  - Inline `#[cfg(test)]` module

#### Approach

1. Extract to `execution.rs`: `execute_sanitized_transaction`, `execute_sanitized_transaction_readonly`, `execute_transaction`, `execute_transaction_no_verify`, `check_and_process_transaction`, `process_message`, `sanitize_transaction`, `sanitize_transaction_no_verify`, `map_sanitize_result`, and related helper types (`CheckAndProcessTransactionSuccess`, etc.).
2. Extract to `helpers.rs`: `validate_fee_payer`, `execution_into_outcome`, `execution_result_if_context`, `commit_execution_outcome`, `fee_payer_for_instructions`, `token_balances`, `execution_trace_from_transaction_context`, `execution_diagnostics`, `public_account_from_shared`.
3. Keep in `lib.rs`: `HPSVM` struct, its `impl` block (public API methods that delegate to `execution.rs`), `SvmCfg`, builder, traits.
4. Use `pub(crate)` visibility for extracted items. Update `use` imports in `lib.rs`.

#### Architecture Decisions (MADR Format)

- **AD-01:** Two new modules (`execution.rs`, `helpers.rs`) rather than more granular splits — keeps the module count manageable and the boundary clear.
- **AD-02:** Keep the `impl HPSVM` block in `lib.rs` but have methods delegate to `execution.rs` functions — preserves the public API while moving implementation detail out.

### Finding 9: Duplicate execute_sanitized_transaction methods

- **Category:** tech debt
- **Impact:** MEDIUM
- **Effort:** S
- **Risk:** LOW — the methods are private; unification is internal.

#### Requirements (EARS Notation)

- **[REQ-01]:** `execute_sanitized_transaction` and `execute_sanitized_transaction_readonly` SHALL share a single implementation.
- **[REQ-02]:** The mutable/readonly distinction SHALL be handled at the call site.

#### Current state

- `crates/hpsvm/src/lib.rs:1740-1773` vs `1775-1808` — structurally identical, differing only in `&mut self` vs `&self`.
- `check_and_process_transaction` at line 1810 already takes `&self` (not `&mut self`), using interior mutability.

#### Approach

Since `check_and_process_transaction` already takes `&self`, unify into a single `execute_sanitized_transaction_impl(&self, ...)` private method. The `send_transaction` path calls it with `&self` for execution and only needs `&mut self` for the commit phase. Remove the `_readonly` variant.

### Finding 10: Token builder send() boilerplate

- **Category:** tech debt
- **Impact:** MEDIUM
- **Effort:** M
- **Risk:** LOW — internal crate code; public API stays the same.

#### Requirements (EARS Notation)

- **[REQ-01]:** A shared `sign_and_send` helper SHALL handle common transaction construction, signing, and submission.
- **[REQ-02]:** Each builder's `send()` SHALL delegate to the helper after building its instruction.
- **[REQ-03]:** The public API (struct names + method signatures) SHALL NOT change.

#### Current state

- 20+ files in `crates/token/src/` repeat the same pattern:

  ```rust
  let payer_pk = self.payer.pubkey();
  let token_program_id = self.token_program_id.unwrap_or(&TOKEN_ID);
  let authority = self.owner.unwrap_or(payer_pk);
  let signing_keys = self.signers.pubkeys();
  let signer_keys = get_multisig_signers(&authority, &signing_keys);
  let ix = /* build instruction */;
  let block_hash = self.svm.latest_blockhash();
  let mut tx = Transaction::new_with_payer(&[ix], Some(&payer_pk));
  tx.partial_sign(&[self.payer], block_hash);
  tx.partial_sign(self.signers.as_ref(), block_hash);
  self.svm.send_transaction(tx)?;
  ```

#### Approach

Add to `crates/token/src/lib.rs`:

```rust
pub(crate) fn sign_and_send(
    svm: &mut HPSVM,
    payer: &Keypair,
    signers: &[&Keypair],
    ix: Instruction,
) -> Result<(), FailedTransactionMetadata> {
    let payer_pk = payer.pubkey();
    let block_hash = svm.latest_blockhash();
    let mut tx = Transaction::new_with_payer(&[ix], Some(&payer_pk));
    tx.partial_sign(&[payer], block_hash);
    tx.partial_sign(signers, block_hash);
    svm.send_transaction(tx)?;
    Ok(())
}
```

Each builder's `send()` becomes: build instruction → call `sign_and_send`.

### Finding 11: Public types in types.rs missing documentation

- **Category:** docs
- **Impact:** MEDIUM
- **Effort:** M
- **Risk:** LOW — additive doc comments only.

#### Requirements (EARS Notation)

- **[REQ-01]:** Every public struct in `types.rs` SHALL have a `///` doc comment on the struct.
- **[REQ-02]:** Every public field SHALL have a `///` doc comment describing its purpose.
- **[REQ-03]:** The `#[expect(missing_docs)]` attributes SHALL be removed after documentation is added.
- **[REQ-04]:** Module-level `#[expect(missing_docs)]` in `lib.rs:387-394` SHALL be replaced with doc comments on the modules.

#### Current state

- `crates/hpsvm/src/types.rs` — 21 `#[expect(missing_docs)]` instances on: `TransactionMetadata`, `ExecutionDiagnostics`, `AccountSourceFailure`, `AccountDiff`, `TokenBalance`, `ExecutionTrace`, `ExecutedInstruction`, `SimulatedTransactionInfo`, `FailedTransactionMetadata`, `TransactionResult`, and their fields.
- `crates/hpsvm/src/lib.rs:387-394` — module-level suppressions for `batch`, `error`, `instruction`, `types`.

#### Approach

For each public type, add doc comments that describe:

- What the type represents in the Solana transaction lifecycle
- When it is populated (e.g., "Populated after transaction execution")
- Constraints or invariants (e.g., "Indices correspond to the transaction message's account keys")

Priority order: `ExecutionOutcome` (most used), `TransactionMetadata`, `ExecutionDiagnostics`, `FailedTransactionMetadata`.

## BDD/TDD Strategy

- **Primary Language:** Rust
- **BDD Runner:** cucumber-rs (existing)
- **BDD Command:** `cargo test -p hpsvm --test bdd`
- **Unit Test Command:** `cargo test --all-features`
- **Feature Files:** `specs/2026-06-17-04-code-quality/features/code-quality.feature`

## Code Simplification Constraints

- **Behavioral Contract:** All existing behavior must be preserved. No public API changes.
- **Repo Standards:** Follow existing module organization patterns. Use `pub(crate)` for internal items.
- **Refactor Scope:** Limit to the modules being split/deduped; do not restructure unrelated code.

## Verification

| Purpose   | Command                                          | Expected on success |
|-----------|--------------------------------------------------|---------------------|
| Check     | `cargo check --all-targets --all-features`       | exit 0              |
| Tests     | `cargo test --all-features`                      | all pass            |
| BDD       | `cargo test -p hpsvm --test bdd`                 | all pass            |
| Clippy    | `cargo +nightly clippy --all -- -D warnings`     | exit 0              |
| Docs      | `cargo doc --no-deps`                            | no missing_docs warnings |
