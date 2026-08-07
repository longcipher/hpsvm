# Design: Test coverage for critical paths

| Metadata | Details |
| :--- | :--- |
| **Status** | Draft |
| **Created** | 2026-06-17 |
| **Mode** | Full |
| **Priority** | P1 |
| **Planned at** | commit `5ba1579`, 2026-06-17 |

## Summary

> Add unit tests for the rent state transition logic (a correctness-critical boundary) that currently has zero coverage.

## Why this matters

The rent state transition logic determines whether transactions succeed or fail with `InsufficientFundsForRent` — a core Solana protocol invariant — and has zero unit tests.

## Findings

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

## Test Strategy

- **Primary Language:** Rust
- **Unit Test Command:** `cargo test --all-features`
- **Test Location:** colocated `#[cfg(test)]` modules in the affected crate

## Code Simplification Constraints

- **Behavioral Contract:** Existing test scenarios must continue to pass. New tests must not modify production code.
- **Repo Standards:** Follow existing `#[cfg(test)]` colocated test patterns.
- **Readability Priorities:** Keep tests example-based and readable.

## Verification

| Purpose   | Command                                          | Expected on success |
|-----------|--------------------------------------------------|---------------------|
| Check     | `cargo check --all-targets --all-features`       | exit 0              |
| Tests     | `cargo test --all-features`                      | all pass            |
| Clippy    | `cargo +nightly clippy --all -- -D warnings`     | exit 0              |
