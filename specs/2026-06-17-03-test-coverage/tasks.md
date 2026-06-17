# Tasks: Test coverage for critical paths

Planned at commit `5ba1579` (2026-06-17).

## Phase 1: BDD failure scenarios (Finding 7)

### Task 1.1: Add BDD scenario for compute budget exceeded

> **Context:** No BDD coverage for compute budget errors — the most common transaction failure.
> **Verification:** Scenario passes with correct error assertion.
> **Scenario Coverage:** `features/test-coverage.feature` — "Transaction compute budget exceeded error is handled"

- **Loop Type:** `BDD+TDD`
- **Behavioral Contract:** `N/A — new test`
- **Simplification Focus:** `N/A — test addition`
- **Status:** 🟢 DONE
- [x] Step 1: Add scenario to `features/transaction_errors.feature` (or inline in new feature file):

  ```gherkin
  Scenario: Transaction compute budget exceeded error is handled
    Given a default HPSVM instance
    And a transaction that exceeds the compute budget
    When the transaction is executed
    Then the result should be an error
    And the error should indicate compute budget exceeded
  ```

- [x] Step 2: Add step definitions to `crates/hpsvm/tests/bdd.rs`:
  - `Given a transaction that exceeds the compute budget` — create transaction with `ComputeBudgetInstruction::set_compute_unit_limit(1)` calling a program that uses more.
  - `Then the result should be an error` — assert `svm.send_transaction(tx)` returns `Err`.
  - `And the error should indicate compute budget exceeded` — assert error contains `ComputationalBudgetExceeded`.
- [x] Step 3: RED — run `cargo test -p hpsvm --test bdd`, confirm scenario fails (no step definition).
- [x] Step 4: Implement step definitions.
- [x] Step 5: GREEN — run BDD, confirm scenario passes.
- [x] BDD Verification: `cargo test -p hpsvm --test bdd` — all pass
- [x] Advanced Test Verification: N/A
- [x] Runtime Verification: N/A

### Task 1.2: Add BDD scenario for insufficient funds

> **Context:** No BDD coverage for insufficient funds errors.
> **Verification:** Scenario passes.
> **Scenario Coverage:** `features/test-coverage.feature` — "Transaction with insufficient funds fails gracefully"

- **Loop Type:** `BDD+TDD`
- **Behavioral Contract:** `N/A — new test`
- **Simplification Focus:** `N/A — test addition`
- **Status:** 🟢 DONE
- [x] Step 1: Add scenario to feature file.
- [x] Step 2: Add step definitions: create sender with 0 lamports, attempt transfer, assert error.
- [x] Step 3: RED → GREEN cycle.
- [x] BDD Verification: `cargo test -p hpsvm --test bdd` — all pass
- [x] Advanced Test Verification: N/A
- [x] Runtime Verification: N/A

### Task 1.3: Add BDD scenario for invalid program

> **Context:** No BDD coverage for invalid program errors.
> **Verification:** Scenario passes.
> **Scenario Coverage:** `features/test-coverage.feature` — "Transaction with invalid program fails gracefully"

- **Loop Type:** `BDD+TDD`
- **Behavioral Contract:** `N/A — new test`
- **Simplification Focus:** `N/A — test addition`
- **Status:** 🟢 DONE
- [x] Step 1: Add scenario to feature file.
- [x] Step 2: Add step definitions: create tx targeting non-existent program account, assert error.
- [x] Step 3: RED → GREEN cycle.
- [x] BDD Verification: `cargo test -p hpsvm --test bdd` — all pass
- [x] Advanced Test Verification: N/A
- [x] Runtime Verification: N/A

### Task 1.4: Add BDD scenario for expired blockhash

> **Context:** No BDD coverage for blockhash-not-found errors.
> **Verification:** Scenario passes.
> **Scenario Coverage:** `features/test-coverage.feature` — "Transaction with expired blockhash fails"

- **Loop Type:** `BDD+TDD`
- **Behavioral Contract:** `N/A — new test`
- **Simplification Focus:** `N/A — test addition`
- **Status:** 🟢 DONE
- [x] Step 1: Add scenario to feature file.
- [x] Step 2: Add step definitions: create tx with expired blockhash (use `svm.expire_blockhash()`), assert error.
- [x] Step 3: RED → GREEN cycle.
- [x] BDD Verification: `cargo test -p hpsvm --test bdd` — all pass
- [x] Advanced Test Verification: N/A
- [x] Runtime Verification: N/A

## Phase 2: Rent state unit tests (Finding 17)

### Task 2.1: Add unit tests for transition_allowed

> **Context:** `rent.rs` has zero unit tests for the rent state transition logic.
> **Verification:** All branches of `transition_allowed` are covered.
> **Scenario Coverage:** `features/test-coverage.feature` — "RentPaying account cannot be credited", "RentPaying account can be debited", "Any state can transition to RentExempt"

- **Loop Type:** `BDD+TDD`
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
- [x] BDD Verification: N/A — unit tests
- [x] Advanced Test Verification: `cargo test -p hpsvm rent` — all pass
- [x] Runtime Verification: N/A

### Task 2.2: Add unit tests for check_rent_state_with_account

> **Context:** `check_rent_state_with_account` has no tests for the incinerator special case.
> **Verification:** Incinerator address bypasses rent checks; normal address with invalid transition returns error.
> **Scenario Coverage:** `features/test-coverage.feature` — "Incinerator address bypasses rent state checks"

- **Loop Type:** `TDD-only`
- **Behavioral Contract:** `N/A — new tests`
- **Simplification Focus:** `N/A — test addition`
- **Status:** 🟢 DONE
- [x] Step 1: Add test `check_rent_state_incinerator_bypass` — use `solana_sdk_ids::incinerator::id()`, assert `Ok(())` for any transition.
- [x] Step 2: Add test `check_rent_state_invalid_transition` — use a random address, assert `Err(InsufficientFundsForRent)` for invalid transition.
- [x] Step 3: Run tests.
- [x] BDD Verification: N/A — unit tests
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
- [x] BDD Verification: N/A — unit tests
- [x] Advanced Test Verification: `cargo test -p hpsvm rent` — all pass
- [x] Runtime Verification: N/A
