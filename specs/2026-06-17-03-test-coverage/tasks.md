# Tasks: Test coverage for critical paths

Planned at commit `5ba1579` (2026-06-17).

## Phase 2: Rent state unit tests (Finding 17)

### Task 2.1: Add unit tests for transition_allowed

> **Context:** `rent.rs` has zero unit tests for the rent state transition logic.
> **Verification:** All branches of `transition_allowed` are covered.

- **Loop Type:** `TDD-only`
- **Behavioral Contract:** `N/A — new tests`
- **Simplification Focus:** `N/A — test addition`
- **Status:** 🟢 DONE
- [x] Step 1: Add `#[cfg(test)] mod tests` to `crates/hpsvm/src/utils/rent.rs`.
- [x] Step 2: Add test cases:
  - `transition_uninitialized_to_rent_exempt` — Uninitialized→RentExempt = true
  - `transition_rent_paying_debit` — RentPaying(1000, 100)→RentPaying(900, 100) = true
  - `transition_rent_paying_credit` — RentPaying(1000, 100)→RentPaying(1100, 100) = false
  - `transition_rent_paying_resize` — RentPaying(1000, 100)→RentPaying(1000, 200) = false
  - `transition_rent_exempt_to_uninitialized` — RentExempt→Uninitialized = true
  - `transition_any_to_rent_exempt` — any→RentExempt = true
- [x] Step 3: Run `cargo test -p hpsvm rent` — all pass.
- [x] Advanced Test Verification: `cargo test -p hpsvm rent` — all pass
- [x] Runtime Verification: N/A

### Task 2.2: Add unit tests for check_rent_state_with_account

> **Context:** `check_rent_state_with_account` has no tests for the incinerator special case.
> **Verification:** Incinerator address bypasses rent checks; normal address with invalid transition returns error.

- **Loop Type:** `TDD-only`
- **Behavioral Contract:** `N/A — new tests`
- **Simplification Focus:** `N/A — test addition`
- **Status:** 🟢 DONE
- [x] Step 1: Add test `check_rent_state_incinerator_bypass` — use `solana_sdk_ids::incinerator::id()`, assert `Ok(())` for any transition.
- [x] Step 2: Add test `check_rent_state_invalid_transition` — use a random address, assert `Err(InsufficientFundsForRent)` for invalid transition.
- [x] Step 3: Run tests.
- [x] Advanced Test Verification: `cargo test -p hpsvm rent` — all pass
- [x] Runtime Verification: N/A

### Task 2.3: Add unit tests for get_account_rent_state

> **Context:** `get_account_rent_state` has no tests.
> **Verification:** All three states are covered.
> **Scenario Coverage:** N/A — covered by transition tests above

- **Loop Type:** `TDD-only`
- **Behavioral Contract:** `N/A — new tests`
- **Simplification Focus:** `N/A — test addition`
- **Status:** 🟢 DONE
- [x] Step 1: Add test `get_rent_state_uninitialized` — 0 lamports → Uninitialized.
- [x] Step 2: Add test `get_rent_state_rent_exempt` — rent-exempt amount → RentExempt.
- [x] Step 3: Add test `get_rent_state_rent_paying` — below rent-exempt → RentPaying.
- [x] Step 4: Run tests.
- [x] Advanced Test Verification: `cargo test -p hpsvm rent` — all pass
- [x] Runtime Verification: N/A
