# Tasks: Quick correctness and tooling fixes

Planned at commit `5ba1579` (2026-06-17).

## Phase 1: Tooling & Dependencies (Findings 3, 4, 14, 15, 16)

### Task 1.1: Add missing tools to Justfile setup

> **Context:** `just setup` doesn't install `rumdl` or `cargo-tarpaulin`, both used by Justfile recipes.
> **Verification:** `just setup` completes; `rumdl --version` and `cargo tarpaulin --version` succeed.
> **Scenario Coverage:** `features/quick-wins.feature` — "Justfile setup installs all required tools"

- **Loop Type:** `TDD-only`
- **Behavioral Contract:** `Preserve existing behavior`
- **Simplification Focus:** `N/A — tooling change`
- **Status:** 🟢 DONE
- [x] Step 1: Edit `Justfile` recipe `setup` (line 112-116). Add `cargo install rumdl` and `cargo install cargo-tarpaulin` after existing install lines.
- [x] Step 2: Run `just setup` on a clean environment (or verify the commands are present).
- [x] BDD Verification: N/A — Justfile recipe, not Rust code
- [x] Advanced Test Verification: N/A
- [x] Runtime Verification: `just setup && rumdl --version && cargo tarpaulin --version`

### Task 1.2: Remove duplicate CI test steps

> **Context:** CI runs `cargo test --features precompiles` and SPL token tests separately, then `just ci` runs the superset `cargo test --all-features`.
> **Verification:** CI workflow has no duplicate test steps.
> **Scenario Coverage:** `features/quick-wins.feature` — "CI does not run duplicate test steps"

- **Loop Type:** `TDD-only`
- **Behavioral Contract:** `Preserve existing behavior`
- **Simplification Focus:** `N/A — CI config change`
- **Status:** 🟢 DONE
- [x] Step 1: Read `.github/workflows/ci.yml`.
- [x] Step 2: Remove the "Run tests" step (line 44-47) and "Run SPL tests" step (line 49-52).
- [x] Step 3: Remove `leptosfmt` from the `taiki-e/install-action` tool list (line 30).
- [x] BDD Verification: N/A — CI config
- [x] Advanced Test Verification: N/A
- [x] Runtime Verification: Review the workflow file to confirm no duplicate test invocations.

### Task 1.3: Replace ansi_term with anstyle in format_logs

> **Context:** `ansi_term` is archived/unmaintained. `anstyle` is the de facto standard.
> **Verification:** `cargo check --all-features` passes; `format_logs` produces identical output.
> **Scenario Coverage:** `features/quick-wins.feature` — "ansi_term replaced with anstyle"

- **Loop Type:** `TDD-only`
- **Behavioral Contract:** `Preserve existing behavior`
- **Simplification Focus:** `Replace abandoned dependency`
- **Status:** 🟢 DONE
- [x] Step 1: Run `cargo add anstyle --workspace` to add the dependency.
- [x] Step 2: Edit `crates/hpsvm/src/format_logs.rs` — replace `use ansi_term::Colour;` with `use anstyle::{Style, Color};`.
- [x] Step 3: Rewrite `colourise` function to use `anstyle` API. Map: `Colour::Fixed(9).bold()` → `Style::new().fg_color(Some(Color::Ansi(AnsiColor::Red))).bold()`, `Colour::Green` → `Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)))`, etc.
- [x] Step 4: Remove `ansi_term` from workspace deps in root `Cargo.toml` and from `crates/hpsvm/Cargo.toml`.
- [x] Step 5: Run existing `format_logs` tests: `cargo test -p hpsvm format_logs`.
- [x] BDD Verification: N/A — internal dependency swap
- [x] Advanced Test Verification: `cargo test -p hpsvm format_logs` — all pass
- [x] Runtime Verification: `cargo check --all-features`

### Task 1.4: Replace serde_yaml with serde_yml

> **Context:** `serde_yaml` is archived; `serde_yml` is the drop-in replacement.
> **Verification:** CLI config parsing works identically.
> **Scenario Coverage:** `features/quick-wins.feature` — "serde_yaml replaced with serde_yml"

- **Loop Type:** `TDD-only`
- **Behavioral Contract:** `Preserve existing behavior`
- **Simplification Focus:** `Replace deprecated dependency`
- **Status:** 🟢 DONE
- [x] Step 1: Run `cargo add serde_yml --workspace` and remove `serde_yaml` from workspace deps.
- [x] Step 2: Edit `bin/hpsvm-cli/Cargo.toml` — replace `serde_yaml.workspace = true` with `serde_yml.workspace = true`.
- [x] Step 3: Edit `bin/hpsvm-cli/src/config.rs:20` — replace `serde_yaml::from_str` with `serde_yml::from_str`.
- [x] Step 4: Run `cargo check -p hpsvm-cli`.
- [x] BDD Verification: N/A — dependency swap
- [x] Advanced Test Verification: `cargo test -p hpsvm-cli` — all pass
- [x] Runtime Verification: `cargo check --all-features`

### Task 1.5: Update ed25519-dalek to v2

> **Context:** v1.0.1 is exact-pinned and unmaintained; v2 is the active line.
> **Verification:** Precompiles tests pass with v2.
> **Scenario Coverage:** `features/quick-wins.feature` — "ed25519-dalek updated to v2"

- **Loop Type:** `TDD-only`
- **Behavioral Contract:** `Preserve existing behavior`
- **Simplification Focus:** `Remove exact-pinned legacy dependency`
- **Status:** 🟢 DONE
- [x] Step 1: Update `Cargo.toml` workspace dep: `ed25519-dalek = "2"` (remove `=` prefix).
- [x] Step 2: Update `crates/hpsvm/Cargo.toml` dev-dep if needed.
- [x] Step 3: Edit `crates/hpsvm/tests/precompiles.rs` — update v1 API calls to v2: `SecretKey::parse_slice(&bytes)` → `SecretKey::from_bytes(&bytes.try_into().unwrap())`, `PublicKey::from_secret_key(&sk)` → `PublicKey::from(&sk)`.
- [x] Step 4: Run `cargo test -p hpsvm --test precompiles`.
- [x] BDD Verification: N/A — dev-dependency update
- [x] Advanced Test Verification: `cargo test -p hpsvm --test precompiles` — all pass
- [x] Runtime Verification: `cargo check --all-features`

## Phase 2: Correctness Fixes (Findings 1, 12, 13, 18)

### Task 2.1: Always populate fee_payer in ExecutionResult

> **Context:** `fee_payer` is filtered to `None` on success, making fee attribution invisible.
> **Verification:** `ExecutionOutcome.fee_payer` is `Some(payer)` for both success and failure.
> **Scenario Coverage:** `features/quick-wins.feature` — "Fee payer is recorded in ExecutionResult"

- **Loop Type:** `BDD+TDD`
- **Behavioral Contract:** `fee_payer field changes from None to Some(payer) on success — callers that check fee_payer.is_none() may need updating`
- **Simplification Focus:** `Remove conditional filter`
- **Status:** 🟢 DONE
- [x] Step 1: Write a test asserting `fee_payer` is `Some` after a successful `transact`.
- [x] Step 2: RED — run test, confirm it fails (fee_payer is None on success).
- [x] Step 3: Edit `crates/hpsvm/src/lib.rs:2280` — remove `.filter(|_| result.is_err())`. Change to just `let fee_payer = fee_payer;`.
- [x] Step 4: GREEN — run test, confirm it passes.
- [x] Step 5: Search codebase for callers of `outcome.fee_payer` or `result.fee_payer` that assume `None` on success. Update if needed.
- [x] BDD Verification: `cargo test -p hpsvm --test bdd` — all pass
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `cargo check --all-features`

### Task 2.2: Add safety comment to as_bytes transmute

> **Context:** `unsafe` block in `register_tracing.rs` lacks `// SAFETY:` documentation.
> **Verification:** `clippy::undocumented_unsafe_blocks` lint passes (if enabled).
> **Scenario Coverage:** `features/quick-wins.feature` — "Unsafe as_bytes transmute has safety documentation"

- **Loop Type:** `TDD-only`
- **Behavioral Contract:** `Preserve existing behavior`
- **Simplification Focus:** `N/A — documentation only`
- **Status:** 🟢 DONE
- [x] Step 1: Edit `crates/hpsvm/src/register_tracing.rs:329` — add `// SAFETY: T is Copy with no padding bytes; the resulting byte slice faithfully represents the original data.` above the `unsafe` block.
- [x] BDD Verification: N/A — documentation change
- [x] Advanced Test Verification: N/A
- [x] Runtime Verification: `cargo check --all-features`

### Task 2.3: Validate SBF_TRACE_DIR path in register_tracing

> **Context:** `SBF_TRACE_DIR` env var is trusted without sanitization — path traversal risk.
> **Verification:** Absolute paths outside working directory are rejected.
> **Scenario Coverage:** `features/quick-wins.feature` — "register_tracing validates SBF_TRACE_DIR path"

- **Loop Type:** `BDD+TDD`
- **Behavioral Contract:** `Paths that resolve outside cwd will error instead of creating directories`
- **Simplification Focus:** `Add path validation`
- **Status:** 🟢 DONE
- [x] Step 1: Write a test that sets `SBF_TRACE_DIR` to `/etc` and asserts `handler()` returns an error.
- [x] Step 2: RED — run test, confirm it fails (handler creates dirs).
- [x] Step 3: Edit `crates/hpsvm/src/register_tracing.rs:214-216` — after joining, check if the resolved path starts with `current_dir`. If not, return an error.
- [x] Step 4: GREEN — run test, confirm it passes.
- [x] BDD Verification: N/A — new test, not BDD scenario
- [x] Advanced Test Verification: `cargo test -p hpsvm register_tracing` — all pass
- [x] Runtime Verification: `cargo check --all-features`

### Task 2.4: Use u64 for loader chunk offset

> **Context:** `u32` offset can overflow for programs > 4GB (unlikely but unguarded).
> **Verification:** Offset uses u64; oversized programs return error.
> **Scenario Coverage:** `features/quick-wins.feature` — "Loader chunk offset uses u64"

- **Loop Type:** `TDD-only`
- **Behavioral Contract:** `Preserve existing behavior for realistic program sizes`
- **Simplification Focus:** `Widen integer type`
- **Status:** 🟢 DONE
- [x] Step 1: Edit `crates/loader/src/lib.rs:82` — change `let mut offset = 0;` to `let mut offset: u64 = 0;`.
- [x] Step 2: Edit line 85 — change `offset` argument to `offset as u64` (or adjust if `write` already accepts u64).
- [x] Step 3: Edit line 92 — change `offset += chunk_size as u32;` to `offset += chunk_size as u64;`.
- [x] Step 4: Optionally add guard: `if program_bytes.len() > u32::MAX as usize { return Err(...) }`.
- [x] BDD Verification: N/A — internal fix
- [x] Advanced Test Verification: `cargo test -p hpsvm-loader` — all pass
- [x] Runtime Verification: `cargo check --all-features`

## Phase 3: N+1 Fix (Finding 2)

### Task 3.1: Add batch fetch method to AccountSource trait

> **Context:** Pre-rent-state lookups call `try_get_account` per writable account, falling through to external source.
> **Verification:** External account source receives a single batch call per transaction.
> **Scenario Coverage:** `features/quick-wins.feature` — "Pre-rent-state lookups are batched"

- **Loop Type:** `BDD+TDD`
- **Behavioral Contract:** `External account source batch method has default impl calling get_account per-key`
- **Simplification Focus:** `Batch N+1 external calls into one`
- **Status:** 🟢 DONE
- [x] Step 1: Edit `crates/hpsvm/src/accounts_db.rs` — add to `AccountSource` trait:

  ```rust
  fn get_accounts(&self, pubkeys: &[Address]) -> Result<Vec<Option<AccountSharedData>>, AccountSourceError> {
      pubkeys.iter().map(|pk| self.get_account(pk)).collect()
  }
  ```

- [x] Step 2: Edit `crates/hpsvm/src/lib.rs:1670-1718` (`check_accounts_rent`) — before the loop, collect writable pubkeys and call `self.accounts.source.get_accounts(&writable_pubkeys)`. Use the cached results inside the loop instead of calling `try_get_account` per-account.
- [x] Step 3: Write a mock `AccountSource` that counts calls. Assert batch method is called once, not N times.
- [x] BDD Verification: `cargo test -p hpsvm --test bdd` — all pass
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `cargo check --all-features`
