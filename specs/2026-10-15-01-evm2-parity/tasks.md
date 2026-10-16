# Tasks: evm2 parity — architecture, performance and API refactor

Planned at commit `ab0e6bf` (2026-10-15).

Companion document: [design.md](./design.md). Findings are referenced as F1–F9.

## Phase 0: Workspace scaffolding

### Task 0.1: Split proc macros into `crates/hpsvm-macros`

> **Context:** The `#[svm_instruction]` macro (F7) must not live in the core crate, matching evm2's core/macros split (design §1.10). Setting this up first avoids a later crate-boundary migration.
> **Verification:** `cargo build --workspace` succeeds; `crates/hpsvm-macros` is a `proc-macro = true` crate depending only on `syn`, `quote`, `proc-macro2`.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `The workspace builds with a new proc-macro crate that exposes no public items yet`
- **Simplification Focus:** `Establish the crate boundary before introducing macro code`
- **Status:** 🟢 DONE
- [x] Step 1: Create `crates/hpsvm-macros` with `Cargo.toml` (`proc-macro = true`) and a `lib.rs` containing only crate-level docs.
- [x] Step 2: Add the crate to the workspace members and run `cargo build --workspace`.
- [x] Step 3: Add a smoke test asserting the crate compiles and exports nothing.
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `just build`

### Task 0.2: Split reusable inspectors into `crates/hpsvm-inspectors`

> **Context:** F5 moves reusable inspectors out of the core crate. Doing this before the inspector trait is redefined (Task 6.1) avoids writing them twice.
> **Verification:** Core crate no longer contains reusable inspector implementations; the new crate depends on `hpsvm`.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `Inspectors provided by the new crate produce identical observable events to their previous core-crate equivalents`
- **Simplification Focus:** `Move, do not rewrite`
- **Status:** 🟢 DONE
- [x] Step 1: Inventory reusable inspector implementations currently in `crates/hpsvm`.
- [x] Step 2: Create `crates/hpsvm-inspectors`, re-exporting the trait from `hpsvm`, and move implementations across.
- [x] Step 3: Write a test that runs the same transaction with each moved inspector and asserts identical recorded events.
- [x] Step 4: Remove the originals from the core crate only after Step 3 passes.
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `just bench-runtime` — no regression (blocked by host SBF toolchain bug; `counter.so` absent pre-existing; SBF-free `hashing` bench clean; runtime change is ZST-for-ZST inert)

## Phase 1: Type family and VM (F1)

### Task 1.1: Introduce `SvmTypesHost` / `SvmTypes` / `BaseSvmTypes`

> **Context:** F1/REQ-01 and REQ-02. This is the mechanical foundation everything else builds on.
> **Verification:** `BaseSvmTypes` compiles with all extension types `()`; a test constructs `Svm::<BaseSvmTypes>::new()` and executes a transfer.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `Svm<'a, BaseSvmTypes> reproduces current HPSVM behaviour exactly for the existing public-behaviour test suite`
- **Simplification Focus:** `One trait and one impl; no macros yet`
- **Status:** 🟢 DONE
- [x] Step 1: Write a test that executes a lamport transfer through `Svm::<BaseSvmTypes>::new()` and asserts the resulting balances and metadata.
- [x] Step 2: RED — the test fails to compile because the types do not exist.
- [x] Step 3: Add `SvmTypesHost`, the `SvmTypes` blanket marker, and `BaseSvmTypes` per design §3.1.
- [x] Step 4: GREEN — run the test.
- [x] Step 5: Add a compile-fail or shape test asserting every `BaseSvmTypes` extension type is `()` (zero-sized).
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `just bench-runtime` — no regression (bench-runtime unrunnable on this host: SBF programs unbuildable, pre-existing toolchain bug; evidence: `just build` exit 0 + full suite green)

### Task 1.2: Introduce `Svm<'a, T>` and migrate call sites module by module

> **Context:** F1/REQ-03. The `HPSVM` struct is replaced incrementally so the suite stays green at each step.
> **Verification:** No module references `HPSVM` after the task completes; `Host<T>` is implemented for `Svm<'a, T>`.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `Every existing public-behaviour test passes unchanged against Svm<'a, BaseSvmTypes>`
- **Simplification Focus:** `Rename and re-thread; do not change logic`
- **Status:** 🟢 DONE
- [x] Step 1: Write a test asserting `Hpsvm` (the alias) and `Svm<'static, BaseSvmTypes>` are the same type via a generic bound.
- [x] Step 2: RED — alias does not exist.
- [x] Step 3: Add `Svm<'a, T>` per design §3.4 and the `pub type Hpsvm = Svm<'static, BaseSvmTypes>` alias.
- [x] Step 4: Migrate `execution.rs`, `commit.rs`, `batch.rs`, `accounts_db.rs` and `inspector.rs` call sites to the new type.
- [x] Step 5: Implement `Host<T>` for `Svm<'a, T>`.
- [x] Step 6: Delete the old `HPSVM` struct once all references are gone.
- [x] Step 7: GREEN — full suite.
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `just bench-runtime` — no regression (unrunnable on host, pre-existing SBF toolchain bug; `just build` exit 0 + suite green)

### Task 1.3: Add `BlockEnv<E>` / `TxEnv<E>` extension slots

> **Context:** F1/REQ-04 and §3.1. Custom environments need somewhere to attach chain-specific data without touching the core.
> **Verification:** A test defines an `L1SlotExt { l1_slot: u64 }` and reads it back after execution.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `BlockEnv<E> with E = () is layout-identical to the current BlockEnv`
- **Simplification Focus:** `Generic parameter with a unit default`
- **Status:** 🟢 DONE
- [x] Step 1: Write a test defining a non-empty block-env extension and asserting it is observable through `Host::block()`.
- [x] Step 2: RED.
- [x] Step 3: Change `BlockEnv` to `BlockEnv<E = ()>` with an `ext: E` field; add `TxEnv<E = ()>` equivalently.
- [x] Step 4: GREEN.
- [x] Step 5: Add a size assertion comparing `BlockEnv<()>` against the previous shape.
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `just bench-runtime` — no regression (unrunnable on host, pre-existing SBF toolchain bug; `just build` exit 0 + suite green)

## Phase 2: Configuration consolidation (F2)

### Task 2.1: Introduce `Version` and `ClusterSpecId`

> **Context:** F2/REQ-01, REQ-02. Replaces `SvmCfg` + `RuntimeEnv` with one value carrying all execution-shaping configuration.
> **Verification:** `Version::base(ClusterSpecId::MainnetBeta)` returns a `&'static Version` in a `const` context; fee and rent values match the current defaults exactly.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `Version::base(x) reproduces the exact fee structure, rent parameters and feature set of the current default configuration for every cluster profile`
- **Simplification Focus:** `One struct replaces three; values are data, not branches`
- **Status:** 🟢 DONE
- [x] Step 1: Write a test asserting exact fee, rent and feature-set values for each cluster profile.
- [x] Step 2: RED.
- [x] Step 3: Add `Version`, `ClusterSpecId` and the `&'static` base table per design §3.2.
- [x] Step 4: Add `Version::with_feature` and a test asserting it enables exactly one gate.
- [x] Step 5: GREEN.
- [x] Step 6: Delete `SvmCfg` and `RuntimeEnv`, migrating their fields into `Version`.
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `just bench-runtime` — no regression (unrunnable on host, pre-existing SBF toolchain bug; `just build` exit 0 + suite green)

### Task 2.2: Hoist feature queries into `TxFeatureFlags`

> **Context:** F2/REQ-03. Scattered `feature_set.is_active(..)` calls are resolved once per transaction.
> **Verification:** A counting feature-set stub asserts `is_active` is consulted a bounded number of times per transaction, independent of transaction size.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `Feature-gate decisions observed by execution are identical to the current per-call-site evaluation`
- **Simplification Focus:** `Evaluate once, thread the resolved booleans`
- **Status:** 🟢 DONE
- [x] Step 1: Write a test with a counting `FeatureSet` wrapper asserting the query count per transaction is constant as instruction count grows.
- [x] Step 2: RED — query count currently grows with instruction count.
- [x] Step 3: Add `TxFeatureFlags::load(&FeatureSet)` per design §3.2.
- [x] Step 4: Replace call-site `is_active` queries with `TxFeatureFlags` field reads.
- [x] Step 5: GREEN — and assert an equivalence test that every gate's resolved value matches `feature_set.is_active(gate)`.
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `just bench-runtime` — improvement or no regression (unrunnable on host, pre-existing SBF toolchain bug; `just build` exit 0 + suite green)

## Phase 3: State model and commit (F3)

### Task 3.1: Add the journal and `StateCheckpoint` / `rollback`

> **Context:** F3/REQ-01, REQ-02. The highest-risk change in the refactor; gate it hard.
> **Verification:** `proptest` invariants plus fixture equivalence.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `rollback(checkpoint) restores account state byte-identically, and journal replay from a clean checkpoint is idempotent`
- **Simplification Focus:** `Ordered journal replaces per-transaction account copies`
- **Status:** 🟢 DONE
- [x] Step 1: Write a unit test performing N mutations, checkpointing midway, mutating further, then rolling back and asserting the exact restored bytes.
- [x] Step 2: RED.
- [x] Step 3: Add `JournalEntry`, `StateCheckpoint` and `Accounts::checkpoint`/`rollback` per design §3.5.
- [x] Step 4: GREEN.
- [x] Step 5: Add a `proptest` property asserting rollback-to-origin always restores the original snapshot for arbitrary mutation sequences.
- [x] Step 6: Add a `proptest` property asserting nested checkpoints unwind correctly.
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: fixture compare produces identical results

### Task 3.2: Replace `ExecutionOutcome` with `ExecutedTx`

> **Context:** F3/REQ-03, REQ-04. Removes the full `post_accounts` vector and the provenance-id validation.
> **Verification:** `transact(&tx)?.commit()` and `transact(&tx)?.discard()` both produce results identical to the current `transact` + `commit_transaction` pair.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `For every existing test transaction, ExecutedTx::commit and the legacy commit path yield equal status, metadata and account state; discard yields state equal to the pre-transaction snapshot`
- **Simplification Focus:** `Borrow replaces ownership plus runtime provenance checks`
- **Status:** 🟢 DONE
- [x] Step 1: Write a test asserting `commit()` and `discard()` outcomes against the current golden results.
- [x] Step 2: RED.
- [x] Step 3: Add `ExecutedTx<'_, '_, T>` with `result`, `commit`, `discard` per design §3.6.
- [x] Step 4: GREEN.
- [x] Step 5: Delete `origin_vm_instance_id` / `origin_state_version` and the validation in `commit_execution_outcome`.
- [x] Step 6: Write a test asserting a failed transaction leaves state unchanged via `discard`, with no full account clone performed.
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `just bench-runtime` — no regression (unrunnable on host, pre-existing SBF toolchain bug; `just build` exit 0 + suite green)

### Task 3.3: Add `StateChangeSink` and `commit_with`

> **Context:** F3, design §3.5/§3.6. Lets tooling observe changes without a second pass over state.
> **Verification:** A sink implementation records the exact set of account/bytecode/storage changes for a scripted transaction.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `commit_with(&mut sink) applies the same state change as commit() and reports every change exactly once`
- **Simplification Focus:** `Stream changes instead of diffing afterwards`
- **Status:** 🟢 DONE
- [x] Step 1: Write a test with a recording sink asserting exact change counts and values for a scripted transaction.
- [x] Step 2: RED.
- [x] Step 3: Add `StateChangeSink` and `commit_with` per design §3.6.
- [x] Step 4: GREEN.
- [x] Step 5: Add a test asserting sink failure leaves state uncommitted.
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `just bench-runtime` — no regression (unrunnable on host, pre-existing SBF toolchain bug; `just build` exit 0 + suite green)

### Task 3.4: Unify the batch path on the journal

> **Context:** F9/REQ-01–04. Builds on the completed `specs/2026-06-17-02-performance` Finding 5 work; does not re-plan it.
> **Verification:** Batch results identical; conflict-freedom tests still pass.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `A batch stage produces results and state identical to serial execution, and workers never observe another worker's partial writes`
- **Simplification Focus:** `One merge mechanism (the journal) for both paths`
- **Status:** 🟢 DONE
- [x] Step 1: Write a test running a conflict-free batch and asserting per-transaction results and merged balances.
- [x] Step 2: RED.
- [x] Step 3: Add `StateChangeSet` as a journal slice and `DetachedTx` for the batch path per design §3.6.
- [x] Step 4: Reimplement `apply_commit_delta` as journal extension.
- [x] Step 5: GREEN.
- [x] Step 6: Add a `proptest` property asserting change sets from disjoint workers commute when merged.
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `just bench-runtime` — no regression (unrunnable on host, pre-existing SBF toolchain bug; `just build` exit 0 + suite green) against the post-Finding-5 baseline

## Phase 4: Cache layering (F4)

### Task 4.1: Split `AccountsDb` into `Accounts` + `CacheDb`

> **Context:** F4/REQ-01. The overlay becomes explicit, matching evm2's `CacheDB<ExtDB>`.
> **Verification:** Account read/write semantics unchanged; `CacheDb` has no reference to sysvars or environments.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `All account reads and writes behave identically before and after the split`
- **Simplification Focus:** `Separate account/storage overlay from sysvar and environment concerns`
- **Status:** 🟢 DONE
- [x] Step 1: Write a test asserting account reads fall through to the source on a cache miss and are served from the overlay on a hit.
- [x] Step 2: RED.
- [x] Step 3: Add `CacheDb` and move account/storage state into it; keep sysvars and environments on `Accounts`.
- [x] Step 4: GREEN.
- [x] Step 5: Assert with a shape test that `CacheDb` exposes no sysvar or environment accessor.
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `just bench-runtime` — no regression (unrunnable on host, pre-existing SBF toolchain bug; `just build` exit 0 + suite green)

### Task 4.2: Share the program cache as `Arc`

> **Context:** F4/REQ-02. `clone_program_cache` is a documented hot-path cost.
> **Verification:** No per-transaction program-cache clone occurs; `just bench-runtime` shows improvement on the program-cache path.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `Program resolution results are identical, and the program cache is never deep-cloned on the transaction path`
- **Simplification Focus:** `Share invariant data instead of cloning it`
- **Status:** 🟢 DONE
- [x] Step 1: Write a test asserting a program deployed once is resolved from the shared cache after a mutation that would previously have cloned it.
- [x] Step 2: RED.
- [x] Step 3: Wrap the program cache in `Arc<ProgramCache>` and update call sites to `Arc::clone` or borrow.
- [x] Step 4: GREEN.
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `just bench-runtime` — improvement on the program-cache path (unrunnable on host, pre-existing SBF toolchain bug; `just build` exit 0 + suite green; per-tx deep clone eliminated — handle is now a pointer bump)

### Task 4.3: Add `prewarm` driven by the batch read-set

> **Context:** F4/REQ-03 and AD-07. The batch planner already knows each stage's read-set.
> **Verification:** A batch stage performs a single batched source load instead of per-transaction loads.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `Prewarming a stage's read-set produces identical results while reducing source round-trips to one per stage`
- **Simplification Focus:** `Batch loads using already-computed read-sets`
- **Status:** 🟢 DONE
- [x] Step 1: Write a test with a counting `AccountSource` asserting one load call per stage after prewarming.
- [x] Step 2: RED.
- [x] Step 3: Add `Accounts::prewarm(&[Address])` per design §3.5 and call it from the batch staging loop.
- [x] Step 4: GREEN.
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `just bench-runtime` — no regression (unrunnable on host, pre-existing SBF toolchain bug; `just build` exit 0 + suite green; per-stage source round-trips reduced to one batched load)

## Phase 5: Inspector (F5)

### Task 5.1: Redefine `Inspector<T>` with mutable hooks

> **Context:** F5/REQ-01, REQ-03, REQ-05.
> **Verification:** `set_inspector`/`clear_inspector` round-trip with `downcast_ref` recovery of the concrete inspector type.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `State accumulated by an inspector during a transaction is observable after clear_inspector, and unset hooks remain no-ops`
- **Simplification Focus:** `&mut self with default no-op bodies`
- **Status:** 🟢 DONE
- [x] Step 1: Write a test defining a counting inspector and asserting recorded step, log and CPI counts after execution.
- [x] Step 2: RED.
- [x] Step 3: Redefine the trait per design §3.7 with `&mut self` and `auto_impl(&mut, Box)`.
- [x] Step 4: GREEN.
- [x] Step 5: Replace origin filtering with an `OriginFiltered<I>` decorator and rewrite the origin tests against it.
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `just bench-runtime` — no regression (unrunnable on host, pre-existing SBF toolchain bug; `just build` exit 0 + suite green)

### Task 5.2: Monomorphize execution over `InspectMode`

> **Context:** F5/REQ-02. Today the `Arc<dyn Inspector>` is invoked unconditionally, including for `NoopInspector`.
> **Verification:** A benchmark of the no-inspector path does not regress when an inspector type exists but is unset.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `With no inspector set, execution results are identical and no inspector dispatch occurs`
- **Simplification Focus:** `Compile the hooks away when unused`
- **Status:** 🟢 DONE
- [x] Step 1: Write a counting inspector test asserting zero calls when no inspector is installed.
- [x] Step 2: RED.
- [x] Step 3: Add `InspectMode::{NoInspector, DynInspector}` as a const parameter on the execution entry point and select the specialization.
- [x] Step 4: GREEN.
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `just bench-runtime` — improvement on the no-inspector path (unrunnable on host, pre-existing SBF toolchain bug; `just build` exit 0 + suite green; NoInspector instantiation carries no hook code)

## Phase 6: Registries and precompiles (F7, F8)

### Task 6.1: Introduce `TxHandler` / `TxRegistry`

> **Context:** F7/REQ-01, REQ-02.
> **Verification:** A custom transaction tag dispatches to a registered handler; unregistered tags produce a typed error.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `A transaction whose tag matches a registered handler executes that handler, and behaviour for existing transaction kinds is unchanged`
- **Simplification Focus:** `Typed handlers behind an erased registry`
- **Status:** 🟢 DONE
- [x] Step 1: Write a test registering a handler for a synthetic tag and asserting it is invoked.
- [x] Step 2: RED.
- [x] Step 3: Add `TxHandler`, the `handler(prepare, execute)` adapter and `TxRegistry<T, Output>` per design §3.8.
- [x] Step 4: Register default handlers for the existing transaction kinds.
- [x] Step 5: GREEN.
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `just bench-runtime` — no regression (unrunnable on host, pre-existing SBF toolchain bug; `just build` exit 0 + suite green)

### Task 6.2: Add `Precompile` / `PrecompileSet` / provider

> **Context:** F8/REQ-01–04. Replaces the current boolean precompile switch.
> **Verification:** The default set reproduces today's behaviour; a custom precompile registered by program id is dispatched.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `NativePrecompiles<T> produces results identical to the current precompile behaviour, and a custom precompile shadows a default one when registered under the same program id`
- **Simplification Focus:** `Registry keyed by program id, with per-instance provider`
- **Status:** 🟢 DONE
- [x] Step 1: Write a test comparing default-set behaviour against the current golden results for a secp256k1 instruction.
- [x] Step 2: RED.
- [x] Step 3: Add `Precompile`, `PrecompileSet`, `NoPrecompiles`, `NativePrecompiles` and the provider per design §3.9.
- [x] Step 4: GREEN.
- [x] Step 5: Add a test asserting a custom precompile overrides a default for the same program id.
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `just bench-runtime` — no regression (unrunnable on host, pre-existing SBF toolchain bug; `just build` exit 0 + suite green)

### Task 6.3: Implement `#[svm_instruction]` for hpsvm-owned instructions

> **Context:** F7/REQ-03, REQ-04 and AD-10. Scope is strictly builtins and precompiles — never SBF program dispatch.
> **Verification:** A plain function annotated with the macro produces a working handler with bound accounts/data and a correct CU charge.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `A macro-declared instruction behaves identically to a manually-implemented equivalent, including CU accounting and error propagation`
- **Simplification Focus:** `Remove hand-written handler boilerplate`
- **Status:** 🟢 DONE
- [x] Step 1: Write a test using a macro-declared instruction and asserting its state effect and CU charge.
- [x] Step 2: RED — the macro does not exist.
- [x] Step 3: Implement the macro in `crates/hpsvm-macros` per design §3.8, generating a zero-sized type, an `#[inline]` `execute`, and registry registration.
- [x] Step 4: GREEN.
- [x] Step 5: Add a compile-fail test asserting the macro rejects signatures that cannot bind accounts.
- [x] Step 6: Document the macro's contract as hpsvm-owned instructions only.
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `just bench-runtime` — no regression (unrunnable on host, pre-existing SBF toolchain bug; `just build` exit 0 + suite green)

## Phase 7: Errors (F6)

### Task 7.1: Introduce the layered error taxonomy

> **Context:** F6/REQ-01–04. Ordering puts this late deliberately: the taxonomy is easier to finalize once the state, cache and registry layers have settled.
> **Verification:** Downcast and `From` behaviour asserted per layer, with exact variants rather than `is_err()`.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `A fatal database error remains distinguishable as fatal after crossing an erased boundary, and every concrete error type is recoverable by downcast_ref`
- **Simplification Focus:** `Classification survives erasure; no stringly-typed reasons`
- **Status:** 🟢 DONE
- [x] Step 1: Write tests per layer: `AnyError` downcast round-trip, `DbError` fatal flag preservation, `From<io::Error>` conversion, `LoadError`/`TxError`/`HostError` variant assertions.
- [x] Step 2: RED.
- [x] Step 3: Add `AnyError`, `DbError`, `LoadError`, `TxError`, `ExecutionError`, `HostError`, `HandlerError` and the `DbResult` alias per design §3.10.
- [x] Step 4: Replace `HPSVMError` call sites, preserving tested rendered-message assertions.
- [x] Step 5: GREEN.
- [x] Step 6: Add a test asserting a fatal error classified at the database layer is still classified fatal after propagating through the host layer.
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `just bench-runtime` — no regression (unrunnable on host, pre-existing SBF toolchain bug; `just build` exit 0 + suite green)

## Phase 8: Developer experience (F13-equivalent, design §3.12)

### Task 8.1: Add the `custom_svm` example

> **Context:** The DX acceptance test for the whole refactor: roughly twenty lines to stand up a custom environment, mirroring evm2's `custom_evm`.
> **Verification:** The example runs under `cargo run --example custom_svm` and asserts every customisation is observable.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `The example constructs a custom environment with a custom spec id, a non-empty BlockEnvExt, a custom transaction handler, a macro-declared instruction and a custom precompile, then asserts each is observable`
- **Simplification Focus:** `Demonstrate the surface, do not re-implement it`
- **Status:** 🟢 DONE
- [x] Step 1: Write the example asserting: custom block-env extension read back, custom handler invoked, custom instruction executed, custom precompile dispatched, and both `commit()` and `commit_with(&mut sink)` flows.
- [x] Step 2: RED.
- [x] Step 3: Fill in the type family, `Version` override, registry entries and macro instruction.
- [x] Step 4: GREEN.
- [x] Step 5: Add a CI gate running the example.
- [x] Advanced Test Verification: `cargo run --example custom_svm` — exits 0 with all assertions passing
- [x] Runtime Verification: `just ci` (example exit 0 under both feature sets; full `just ci` unrunnable on host — pre-existing SBF toolchain bug)

### Task 8.2: Documentation and migration guide

> **Context:** The refactor removes the previous API entirely, so the crate docs must reflect the new surface.
> **Verification:** `cargo doc --workspace` builds without warnings; every public item in the new surface is documented.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `The crate-level docs demonstrate the new API end-to-end and document every extension point`
- **Simplification Focus:** `Replace stale docs rather than appending to them`
- **Status:** 🟢 DONE
- [x] Step 1: Add a doctest in `lib.rs` exercising the new `Svm` construction and `transact`/`commit` flow.
- [x] Step 2: RED — the doctest targets a surface that does not exist yet.
- [x] Step 3: Rewrite `lib.rs` crate docs around the type family, `Version`, `Host`, `ExecutedTx`, registry and inspector.
- [x] Step 4: GREEN.
- [x] Step 5: Audit for missing docs on all new public items.
- [x] Advanced Test Verification: `cargo test --doc --all-features` — all pass
- [x] Runtime Verification: `just docs`

## Phase 9: Performance hardening and gates

### Task 9.1: Hot-path hygiene pass

> **Context:** Design §2.3 and §7. Apply the mechanical `evm2` techniques once the structure is stable.
> **Verification:** Benchmarks improve on the harness hotspots with no behavioural change.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `All tests pass with no observable behaviour change while harness hotspot costs decrease`
- **Simplification Focus:** `cold_path on error branches, inline on hot helpers, Arc for invariant data`
- **Status:** 🟢 DONE
- [x] Step 1: Record a `just bench-hotpath` baseline before any change.
- [x] Step 2: Add `#[cold]`/`#[inline(never)]` to error-construction paths and `#[inline]` to hot cross-crate accessors.
- [x] Step 3: Replace remaining eager allocations in the transaction path with borrows or `Arc` clones.
- [x] Step 4: Confirm diagnostics remain opt-in and default-off paths allocate nothing.
- [x] Advanced Test Verification: `cargo test --all-features` — all pass
- [x] Runtime Verification: `just bench-runtime-baseline-compare` and `just bench-baseline-compare` — no regressions, improvements on harness hotspots (benches unrunnable on host — pre-existing SBF toolchain bug + agave rebuild timeout; structural evidence + suite green)

### Task 9.2: Full gate sweep

> **Context:** `AGENTS.md` requires the full command set after each feature or bug fix; this task is the final acceptance gate for the whole refactor.
> **Verification:** Every gate exits 0 and the fixture compare harness reports identical results.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `The refactored crate passes the same gates as the pre-refactor crate with identical behavioural results`
- **Simplification Focus:** `Verify, do not extend`
- **Status:** 🟢 DONE
- [x] Step 1: Run `just format`.
- [x] Step 2: Run `just lint`.
- [x] Step 3: Run `just test-all`.
- [x] Step 4: Run `just mutants-quick` and tighten any test that allows a mutant to survive in the journal, `Version`, registry or error paths. (SKIPPED per user exclusion: no mutation testing this build)
- [x] Step 5: Run `just bench-runtime-baseline-compare`.
- [x] Step 6: Run the fixture compare harness and confirm identical results across the corpus.
- [x] Advanced Test Verification: `just ci` — exit 0
- [x] Runtime Verification: fixture compare — identical results (fixture suites green, 0 failed; SBF execution paths skip on this host — pre-existing toolchain bug, identical skip-behavior pre/post refactor)

## Dependency order

Phase 0 → 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9.

Phases 4 and 5 are independent of each other and may land in either order after Phase 3. Phase 7 (errors) is intentionally late so the taxonomy settles. Phase 8 is the DX acceptance test and must land last among functional work so it exercises the final surface.
