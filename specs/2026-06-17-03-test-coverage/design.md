# Design: Test coverage for critical paths

| Metadata | Details |
| :--- | :--- |
| **Status** | Draft |
| **Created** | 2026-06-17 |
| **Mode** | Full |
| **Priority** | P1 |
| **Planned at** | commit `5ba1579`, 2026-06-17 |

## Summary

> Add BDD scenarios for transaction failure paths (the most common error conditions users encounter) and unit tests for the rent state transition logic (a correctness-critical boundary). Currently only 2 BDD scenarios exist, both testing success paths only.

## Why this matters

The BDD layer currently provides zero regression coverage for error conditions. Changes to error propagation in the core VM pipeline have no BDD-level safety net. The rent state transition logic determines whether transactions succeed or fail with `InsufficientFundsForRent` — a core Solana protocol invariant — and has zero unit tests.

## Findings

### Finding 7: No BDD scenarios for transaction failure paths

- **Category:** test coverage
- **Impact:** HIGH
- **Effort:** M
- **Risk:** LOW — adding tests cannot break existing behavior.

#### Requirements (EARS Notation)

- **[REQ-01]:** BDD scenarios SHALL cover compute budget exceeded errors.
- **[REQ-02]:** BDD scenarios SHALL cover insufficient funds errors.
- **[REQ-03]:** BDD scenarios SHALL cover invalid program errors.
- **[REQ-04]:** BDD scenarios SHALL cover expired blockhash errors.
- **[REQ-05]:** Each scenario SHALL have step definitions that reuse existing domain modules (HPSVM, Account builders).

#### Current state

- `features/instruction_first_execution.feature` — 1 scenario, success path only.
- `features/feature_set_reconfiguration.feature` — 1 scenario, success path only.
- `crates/hpsvm/tests/bdd.rs` — step definitions for the above 2 scenarios.

#### Approach

Add a new `features/transaction_errors.feature` file with scenarios for each error type. Add corresponding step definitions in `bdd.rs` (or a new step definition file). Each scenario:

1. Creates a default HPSVM instance
2. Sets up the specific error condition (zero balance, compute budget, etc.)
3. Executes the transaction
4. Asserts the error type

Pattern to follow: existing `bdd.rs` `FeatureSetWorld` pattern with `#[derive(cucumber::World)]`.

### Finding 17: No unit tests for rent state transition logic

- **Category:** test coverage
- **Impact:** MEDIUM
- **Effort:** S
- **Risk:** LOW — adding tests cannot break existing behavior.

#### Requirements (EARS Notation)

- **[REQ-01]:** Unit tests SHALL cover all branches of `transition_allowed`.
- **[REQ-02]:** Unit tests SHALL cover `check_rent_state_with_account` including the incinerator special case.
- **[REQ-03]:** Unit tests SHALL cover `get_account_rent_state` for all three states.
- **[REQ-04]:** Tests SHALL be colocated in `crates/hpsvm/src/utils/rent.rs` as a `#[cfg(test)] mod tests`.

#### Current state

- `crates/hpsvm/src/utils/rent.rs` — 83 lines, zero `#[cfg(test)]` module.
- Functions: `RentState` enum, `check_rent_state_with_account`, `get_account_rent_state`, `transition_allowed`.

#### Approach

Add a `#[cfg(test)] mod tests` at the bottom of `rent.rs`. Test cases:

- `transition_allowed`: Uninitialized→RentExempt (ok), RentPaying→RentPaying debit (ok), RentPaying→RentPaying credit (reject), RentPaying→RentPaying resize (reject), RentExempt→Uninitialized (ok), any→RentExempt (ok).
- `check_rent_state_with_account`: incinerator address bypass, normal address with invalid transition.
- `get_account_rent_state`: zero lamports (Uninitialized), rent-exempt amount (RentExempt), below rent-exempt (RentPaying).

## BDD/TDD Strategy

- **Primary Language:** Rust
- **BDD Runner:** cucumber-rs
- **BDD Command:** `cargo test -p hpsvm --test bdd`
- **Unit Test Command:** `cargo test --all-features`
- **Feature Files:** `specs/2026-06-17-03-test-coverage/features/test-coverage.feature`
- **Outside-in Loop:** BDD scenarios fail first (RED), step definitions added, scenarios pass (GREEN), step definitions cleaned up (REFACTOR).

## Code Simplification Constraints

- **Behavioral Contract:** Existing test scenarios must continue to pass. New tests must not modify production code.
- **Repo Standards:** Follow existing `cucumber-rs` patterns in `bdd.rs`. Use `#[cfg(test)]` colocated tests for unit tests.
- **Readability Priorities:** Step definitions should be thin wrappers that delegate to domain modules.

## Verification

| Purpose   | Command                                          | Expected on success |
|-----------|--------------------------------------------------|---------------------|
| Check     | `cargo check --all-targets --all-features`       | exit 0              |
| Tests     | `cargo test --all-features`                      | all pass            |
| BDD       | `cargo test -p hpsvm --test bdd`                 | all pass (including new scenarios) |
| Clippy    | `cargo +nightly clippy --all -- -D warnings`     | exit 0              |
