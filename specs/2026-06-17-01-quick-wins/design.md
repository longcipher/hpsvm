# Design: Quick correctness and tooling fixes

| Metadata | Details |
| :--- | :--- |
| **Status** | Draft |
| **Created** | 2026-06-17 |
| **Mode** | Lightweight |
| **Priority** | P1 |
| **Planned at** | commit `5ba1579`, 2026-06-17 |

## Summary

> Ten independent S-effort fixes across correctness, security, dependencies, and DX. Each is a self-contained change with low risk and clear verification. No cross-finding dependencies.

## Why this matters

These are the highest-leverage, lowest-effort fixes in the audit. They close correctness gaps (fee payer visibility, integer overflows), remove abandoned dependencies, and fix onboarding friction. Each can be shipped independently.

## Findings

### Finding 1: Fee payer omitted from ExecutionResult on success

- **Category:** correctness
- **Impact:** HIGH
- **Effort:** S

#### Requirements (EARS Notation)

- **[REQ-01]:** The `ExecutionResult.fee_payer` field SHALL be populated with the validated payer key for all transactions, regardless of success or failure.

#### Current state

- `crates/hpsvm/src/lib.rs:2280`: `let fee_payer = fee_payer.filter(|_| result.is_err());` — unconditionally sets `fee_payer` to `None` on success.

#### Approach

Remove the `.filter(|_| result.is_err())` so `fee_payer` is always set to the validated payer key.

#### Architecture Decisions (MADR Format)

- **AD-01:** Always populate `fee_payer` — callers can distinguish "no fee payer" from "fee payer present" by checking `fee_payer.is_none()` only when the fee payer was genuinely unknown.

### Finding 2: N+1 pre-rent-state lookups from external account source

- **Category:** performance
- **Impact:** HIGH
- **Effort:** S

#### Requirements (EARS Notation)

- **[REQ-01]:** The `AccountSource` trait SHALL provide a batch fetch method that accepts a slice of pubkeys.
- **[REQ-02]:** The rent check loop SHALL call the batch fetch method once before iterating, not per-account.

#### Current state

- `crates/hpsvm/src/lib.rs:1696`: `self.accounts.try_get_account(pubkey)` called inside the per-account loop at line 1677.
- `crates/hpsvm/src/accounts_db.rs:262`: falls through to `self.source.get_account(pubkey)` for each cache miss.

#### Approach

Add `get_accounts(&self, pubkeys: &[Address]) -> Result<Vec<Option<AccountSharedData>>, AccountSourceError>` to `AccountSource` with a default implementation calling `get_account` per-key. Pre-fetch all writable account pubkeys before the rent check loop.

### Finding 3: Justfile setup missing tools

- **Category:** DX
- **Impact:** MEDIUM
- **Effort:** S

#### Current state

- `Justfile:112-116`: `setup` installs `cargo-machete`, `cargo-sort`, `typos-cli` but not `rumdl` or `cargo-tarpaulin`.

#### Approach

Add `cargo install rumdl` and `cargo install cargo-tarpaulin` to the `setup` recipe.

### Finding 4: CI duplicate test steps

- **Category:** DX
- **Impact:** MEDIUM
- **Effort:** S

#### Current state

- `.github/workflows/ci.yml:44-55`: Runs `cargo test --features precompiles` and SPL token tests separately, then `just ci` which runs `cargo test --all-features` (superset).

#### Approach

Remove the standalone `cargo test --features precompiles` and `cd crates/token && cargo test --features token-2022` steps. Let `just ci` handle all testing. Keep the Solana CLI install and test program build steps.

### Finding 12: Unsafe as_bytes lacks safety docs

- **Category:** correctness
- **Impact:** LOW
- **Effort:** S

#### Current state

- `crates/hpsvm/src/register_tracing.rs:329-331`: `unsafe { std::slice::from_raw_parts(...) }` with no `// SAFETY:` comment.

#### Approach

Add `// SAFETY: T is Copy with no padding bytes; the resulting byte slice faithfully represents the original data.` Consider replacing with `bytemuck::cast_slice` if the type implements `bytemuck::Pod`.

### Finding 13: register_tracing path traversal

- **Category:** security
- **Impact:** LOW
- **Effort:** S

#### Current state

- `crates/hpsvm/src/register_tracing.rs:174`: `std::env::var("SBF_TRACE_DIR").unwrap_or(DEFAULT_PATH.to_string())` — no path validation.
- `crates/hpsvm/src/register_tracing.rs:214-216`: `current_dir.join(&self.sbf_trace_dir)` — joins without checking.

#### Approach

Validate that the resolved path is either relative (stays under cwd) or explicitly allowed. Add a check: if the path is absolute and not under an expected base directory, return an error.

### Finding 14: ansi_term replaced with anstyle

- **Category:** dependency
- **Impact:** LOW
- **Effort:** S

#### Current state

- `crates/hpsvm/src/format_logs.rs:3`: `use ansi_term::Colour;`
- 4 color variants used: `Fixed(9).bold()`, `Green`, `Fixed(243).bold()`, `Fixed(239)`.

#### Approach

Replace `ansi_term::Colour` with `anstyle::Style` and `anstyle::Color::Ansi(AnsiColor::...)`. The `colourise` function maps trivially. Remove `ansi_term` from workspace deps, add `anstyle`.

### Finding 15: serde_yaml replaced with serde_yml

- **Category:** dependency
- **Impact:** LOW
- **Effort:** S

#### Current state

- `bin/hpsvm-cli/src/config.rs:20`: `serde_yaml::from_str::<CompareConfigFile>(&file)` — single call site.

#### Approach

Replace `serde_yaml` with `serde_yml` (drop-in replacement, identical API). Update workspace dep and the single call site.

### Finding 16: ed25519-dalek updated to v2

- **Category:** dependency
- **Impact:** LOW
- **Effort:** S

#### Current state

- Root `Cargo.toml:27`: `ed25519-dalek = "=1.0.1"` (exact pin, dev-dependency only).
- `tests/precompiles.rs` uses `ed25519-dalek` v1 API.

#### Approach

Update to `ed25519-dalek = "2"` in workspace deps and `crates/hpsvm/Cargo.toml` dev-deps. Adjust 4 call sites in `tests/precompiles.rs` for v2 API changes (`SecretKey::parse_slice` → `SecretKey::from_bytes`, etc.).

### Finding 18: u32 overflow in loader chunk offset

- **Category:** correctness
- **Impact:** LOW
- **Effort:** S

#### Current state

- `crates/loader/src/lib.rs:82-92`: `let mut offset = 0u32; ... offset += chunk_size as u32;`

#### Approach

Change `offset` to `u64` or add a guard: `if program_bytes.len() > u32::MAX as usize { return Err(...) }`. The BPF loader `write` instruction accepts `u64` offset, so `u64` is safe.

## Verification

| Purpose   | Command                                          | Expected on success |
|-----------|--------------------------------------------------|---------------------|
| Check     | `cargo check --all-targets --all-features`       | exit 0              |
| Tests     | `cargo test --all-features`                      | all pass            |
| BDD       | `cargo test -p hpsvm --test bdd`                 | all pass            |
| Clippy    | `cargo +nightly clippy --all -- -D warnings`     | exit 0              |
