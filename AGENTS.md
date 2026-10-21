# hpsvm Agent Instructions

## Non-negotiables

- Surgical edits only: every changed line must trace to the request. No drive-by refactors, no reformatting untouched code, no deleting pre-existing dead code (mention it instead).
- Fix root causes. Never silence a warning, weaken a check, or patch a symptom to make a gate pass.
- Never report completion while any `just` gate below fails. Report the failure verbatim and stop.
- Stop and ask when a request is ambiguous, conflicts with existing conventions, or requires credentials, publishing, or destructive operations.
- When a correction reveals a missing rule, add one specific rule here ("always X for Y", never "be careful with Y"); delete rules that never catch real errors.

## Scope

- `crates/hpsvm` is the in-process Solana VM simulator; `crates/hpsvm-fixture` captures and replays transaction fixtures; `crates/hpsvm-inspectors` exposes reusable execution observers; `crates/hpsvm-macros` holds the proc macros; `crates/hpsvm-test-support` resolves the SBF test-program binaries.
- `bin/hpsvm-cli` produces the `hpsvm` binary for fixture authoring and inspection.
- `crates/hpsvm/test_programs/` holds the SBF test programs built with the Solana CLI; `crates/hpsvm/elf/` holds the vendored program binaries that ship inside the published `hpsvm` crate.
- `vendor/hpsvm-fd-fixture` is a byte-identical third-party fork with three widened version requirements (see `vendor/PATCHES.md`). It is excluded from the workspace: never reformat it, never lint it, never "fix" it in place.
- `crates/hpsvm-fixture/fuzz/` is its own cargo-fuzz workspace root.
- English only in code, comments, docs, and commit messages.

## Execution

- Parallelize aggressively: dispatch subagents for independent research, exploration, and implementation.
- NEVER run two `cargo` commands concurrently, and never overlap file edits with a running `cargo` command (`target/` file lock): finish edits, wait, then run.
- NEVER use `git worktree`. Edit the current branch in place.
- TDD for behavior changes: failing test first, smallest change to pass, then run `just mutants-quick` and fix surviving mutants.

## Verification loop

After each feature or bug fix, in order; any failure means the task is not done:

```bash
just format
just lint
just test-all
just mutants-quick
```

`just test-all` runs `just sbf-test-programs` first, because integration tests, benches, and the `lib.rs` doctests silently skip when the SBF programs are missing.

## Workspace rules

- Never hand-type dependency versions. Add per-crate deps with `cargo add <crate> -p <crate>`. Shared deps go into root `[workspace.dependencies]` — numeric versions only, no features — and are referenced with `dep = { workspace = true }`.
- Sub-crates inherit `version`/`edition`/`license`/`repository` via `workspace = true`. Inheritance failures are fixed by `just fix` (cargo-workspace-inheritance-check).
- The toolchain is pinned to nightly in `rust-toolchain.toml`. Use `cargo +nightly fmt` / `cargo +nightly clippy` as the Justfile does.
- `.cargo/config.toml` enables `-Zpolonius=next` for every crate. Unset the compiler wrapper when invoking clippy manually (`RUSTC_WRAPPER=`), exactly as the Justfile does.
- The agave and solana version lines are load-bearing. `crates/hpsvm` depends on the older `-agave` copies of `solana-message`, `solana-transaction`, and `solana-svm-transaction`; `crates/hpsvm/src/agave_compat.rs` bridges them to the newer workspace pins. Do not collapse the two families into one version, and move `solana-svm-transaction` and agave together.
- The Solana CLI release used to build the test programs must track the agave dependency line. Bumping either one alone breaks the link step.
- SBF test programs are built with `cargo build-sbf` inside `crates/hpsvm/test_programs`; the four required binaries are `counter.so`, `failure.so`, `hpsvm_clock_example.so`, and `test_program_custom_syscall.so`. On macOS the platform-tools rustc can emit proc-macro dylibs that dyld rejects with an "misaligned LINKEDIT string pool" error; build on Linux and copy `target/deploy/` back.
- Mutation testing is configured in `.cargo/mutants.toml`: `just mutants` runs the whole workspace, `just mutants-quick` mutates only the branch diff. A surviving mutant means a missing assertion, not a reason to exclude the code.
- **BDD is not used in this project.** Do not add Gherkin `.feature` files, `cucumber-rs`, step definitions, or BDD runners.
- Workspace lints deny `unwrap_used`, `allow_attributes`, `todo`, `unimplemented`, `dbg_macro`, `print_stdout`, and `print_stderr`. The only escape hatch is `#[expect(lint, reason = "...")]`.

## Dependency choices

- Caches: `lru` behind a `parking_lot::Mutex` for bounded caches; never a hand-rolled LRU.
- Locks: `parking_lot` for sync locking; release guards before any `.await`-free hot path boundary.
- CPU-bound work: `rayon`; the VM is single-instance and in-process, so never emulate Sealevel-style concurrent scheduling.
- Logging: `tracing` only. Program `log()` calls are forwarded to `tracing::info!`.
- Errors: `thiserror` in the crates. There is no HTTP client and no database in this workspace.
- Forbidden by default: `anyhow`, `log`, `reqwest`, `dashmap`.

## Code conventions

- Feature flags on `crates/hpsvm` are the public surface: `precompiles`, `register-tracing`, `hotpath`, `hotpath-alloc`, `fork`, `loader`, `token`, `token-2022`, `serde`, `hashbrown`, `internal-test`, `invocation-inspect-callback`. Add a new capability as a feature, not as an always-on dependency.
- `HPSVM` stays the stable facade. New capability goes into the sharper seams: `transact`/`commit`/`discard`, `transact_detached`, `with_account_source`, `block_env`, `with_inspector`.
- Assertions must be specific: compare exact expected values and error variants, never only `is_err()` or a stringified error.
- Benchmarks live in `crates/hpsvm/benches` and are compared against the committed baselines in `docs/benchmarks/baselines/` and `docs/benchmarks/runtime-baselines/`. Refresh a baseline only on the machine that owns it, via `just bench-baseline-refresh` or `just bench-runtime-baseline-refresh`.
- Hotpath runs are driven by the `HPSVM_HOTPATH=1` and `HPSVM_TRACE_METRICS=1` environment variables; the Justfile recipes already set them.
- Publishing order is dependency order, dev-dependencies included: `hpsvm-macros`, `hpsvm-test-support`, `hpsvm`, `hpsvm-fixture`, `hpsvm-inspectors`, `hpsvm-cli`, with `hpsvm-fd-fixture` published first from `vendor/`. Use `just publish`; it mirrors the release workflow and waits for index propagation.
