# Design: evm2 parity — architecture, performance and API refactor

| Metadata | Details |
| :--- | :--- |
| **Status** | Draft |
| **Created** | 2026-10-15 |
| **Mode** | Full |
| **Priority** | P1 |
| **Planned at** | commit `ab0e6bf`, 2026-10-15 |
| **Reference project** | `/Volumes/akext/tmp/evm2` (crate `evm2`, `crates/macros`, `crates/inspectors`, `crates/jit`, `crates/eest`) |
| **Scope** | Ground-up refactor of `crates/hpsvm`; no interface-compatibility constraints |

## Summary

> Rebuild `crates/hpsvm` around the architectural spine of the `evm2` project: a single associated-type family (`SvmTypesHost`) that makes spec, transaction envelope, host, environment extensions and extension state swappable without reshaping the core; a `Version` object that carries feature/gas/limit configuration as runtime data instead of scattering booleans through the code; a `Host` trait that decouples execution from the concrete VM; a typed-inside/type-erased-outside registry; a journal-based state model with checkpoint rollback; a borrow-scoped `ExecutedTx` with `commit()`/`discard()`; a zero-cost inspector with mutable hooks; and a layered, classification-preserving error taxonomy. The refactor is explicitly bounded by two decisions: agave's `solana-program-runtime` remains the semantic execution core, and configuration stays runtime-`FeatureSet`-based rather than compile-time const-generic.

## Why this matters

`hpsvm` is functionally rich but architecturally monolithic. `HPSVM` is a single struct with 20+ fields ([lib.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/lib.rs#L369-L397)), execution is hardwired to agave's concrete `InvokeContext`/`process_message` path ([execution.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/execution.rs#L349-L389)), `BlockEnv` has no extension slot ([env.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/env.rs#L8-L11)), configuration lives in three overlapping structs (`SvmCfg`, `RuntimeEnv`, `BlockEnv`), the inspector is an immutable `Arc<dyn Inspector>` with three hooks ([inspector.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/inspector.rs#L30-L71)), errors are a flat enum carrying `String` reasons ([error.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/error.rs#L43-L79)), and transaction commit materializes a full `Vec<(Address, AccountSharedData)>` per transaction guarded by instance/version provenance ids ([types.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/types.rs#L154-L165)).

`evm2` solves the same class of problems — a VM that must be extended by downstream forks, tuned for speed, and pleasant to use — with a much smaller, more regular core. Every one of those five problems has a proven, transferable solution in `evm2`. Adopting them gives `hpsvm` three concrete outcomes: a core small enough to audit, extension points that do not require editing the core, and measurable hot-path wins in the harness that surrounds agave.

## 1. Reference analysis: what makes evm2 good

### 1.1 One type family for the whole configuration surface

`EvmTypesHost` ([config.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/src/evm/config.rs#L15-L59)) is a single trait whose associated types describe every swappable axis: `ConfigSelector`, `SpecId`, `Tx`, `EvmExt`, `MessageExt`, `MessageResultExt`, `TxEnvExt`, `TxResultExt`, `BlockEnvExt`, and `Host<'a>`. A blanket `EvmTypes` marker pins `Host<'a>` to `Evm<'a, Self>`, so "the host is this VM" is a type-system fact rather than a runtime convention. The base family (`BaseEvmTypes`) sets every extension type to `()`. The payoff: a fork changes one trait impl and inherits the whole machinery; unused extension points cost nothing because they are `()` (zero-sized) and monomorphized away.

### 1.2 Configuration as runtime data, not scattered branches

`Version` ([version/mod.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/src/version/mod.rs#L27-L49)) bundles the gas parameter table, feature set, chain id, and all limits into one value stored on the active execution path. The source comment states the reason directly: gas parameters are data on the active version "so changes automatically affect every instruction that reads them. Tracking instruction dependencies on opcode config is not sustainable for custom forks." `Version::base(spec_id)` is a `const fn` returning a `&'static` entry, so lookup is an array index, not a branch chain. Custom forks override with `with_version` ([custom_evm/main.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/examples/custom_evm/main.rs#L232-L252)).

### 1.3 Compile-time dispatch tables and the `#[instruction]` macro

`make_table` ([dispatch/mod.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/src/interpreter/dispatch/mod.rs#L57-L123)) builds a 256-entry instruction table in a `const fn`, copying the previous spec's table and overwriting only changed entries. Two parallel tables are generated — one for the no-inspector path, one for the dynamic-inspector path ([dispatch/mod.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/src/interpreter/dispatch/mod.rs#L125-L205)) — so inspector support is compiled away when unused. Opcodes are declared as ordinary functions via `#[instruction(...)]` ([macros/lib.rs](file:///Volumes/akext/tmp/evm2/crates/macros/src/lib.rs#L17-L47), expansion at [L196-L323](file:///Volumes/akext/tmp/evm2/crates/macros/src/lib.rs#L196-L323)), which generates a zero-sized type plus an `#[inline]` `execute` implementation with stack binding, dynamic-gas handling and `EvmTypes` concrete/bound specialization. The macro is the reason opcode plumbing is cheap in `evm2`.

### 1.4 The Host trait as the single state boundary

`Host<T>` ([host.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/src/interpreter/host.rs#L112-L184)) is the only surface the interpreter uses to reach spec/config, block environment, account/storage loads, message execution and self-destruct. The interpreter never knows which state backend it runs against. This is what allows `InMemoryDB`, `CacheDB` overlays, `AnyDatabase` and fork databases to be interchangeable.

### 1.5 Typed-inside, type-erased-outside registries

`TxHandler<T, Tx, Output>` with `TxRequest` ([registry.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/src/evm/registry.rs#L170-L240)) and `AnyTxHandler`/`TxRegistry` ([registry.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/src/evm/registry.rs#L240-L402)) let handlers be written against concrete transaction types while the registry stores them type-erased, keyed by envelope tag. Adding a transaction kind is a registry insertion, not a core change.

### 1.6 Precompiles as a first-class registry

`Precompiles`/`Precompile` with `NoPrecompiles` as the empty default and `install_crypto` as a batteries-included set. `Evm` stores `Box<dyn PrecompileProvider<T>>` ([evm/mod.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/src/evm/mod.rs#L207-L230)), so a chain can replace the precompile set per instance.

### 1.7 Inspectors with mutable hooks and downcasting

`Inspector<T>` ([inspector.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/src/evm/inspector.rs#L12-L117)) exposes `initialize_interp`, `step`, `step_end`, `call`, `create`, `log` and more, all defaulting to no-ops, all taking `&mut self`. `Evm` holds `Option<Box<dyn Inspector<T>>>` and exposes `set_inspector`/`clear_inspector` with `downcast_ref` recovery — demonstrated end-to-end in [custom_evm/main.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/examples/custom_evm/main.rs#L157-L196).

### 1.8 Journal-based state and borrow-scoped commit

`State<'a>` ([state/mod.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/src/evm/state/mod.rs#L53-L223)) layers a `CacheDB` overlay over a boxed backend, records every mutation as a `JournalEntry` ([journal.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/src/evm/state/journal.rs#L40)), supports `StateCheckpoint` + `rollback` ([state/mod.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/src/evm/state/mod.rs#L723)), and exposes `prewarm`/`prewarm_storage` ([state/mod.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/src/evm/state/mod.rs#L396-L410)) for batch account preloading. `CacheDB<ExtDB>` ([db/cache.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/src/evm/db/cache.rs#L109)) is the concrete overlay. Commitment is borrow-scoped: `ExecutedTx` ([tx.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/src/evm/tx.rs#L142-L155)) borrows the EVM and offers `.commit()`, `.discard()` and `.commit_with(&mut sink)`. No provenance identifiers, no full post-account vectors — the pending journal *is* the delta.

### 1.9 A layered, classification-preserving error taxonomy

`AnyError` (an `Arc<dyn Error + Send + Sync>` with `downcast_ref`) sits underneath `DatabaseError { error, fatal }`, `LoadError`, `ExecutionError`, `HostError` and `HandlerError` ([error.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/src/error.rs#L71-L153)). The `fatal` flag survives type erasure, so a fatal database fault is still distinguishable from an input-level load failure after crossing an erased boundary. Concrete error types remain recoverable by downcast.

### 1.10 Crate layout and developer experience

The core (`crates/evm2`) is separate from the proc macros (`crates/macros`), from optional inspectors (`crates/inspectors`), from the JIT family (`crates/jit/*`) and from the conformance suite (`crates/eest`, the Ethereum Execution Spec Tests analogue). The DX target is [custom_evm](file:///Volumes/akext/tmp/evm2/crates/evm2/examples/custom_evm): roughly twenty lines to stand up a custom environment with a custom spec id, custom block-env extension, custom transaction envelope and custom registry.

## 2. Transferability analysis

This is the spine of the plan. Each `evm2` mechanism is classified as **direct**, **adapted** or **blocked**, with the structural reason.

### 2.1 Blocked — do not port

| evm2 mechanism | Why it does not transfer | Evidence |
| :--- | :--- | :--- |
| 256-entry const-fn opcode dispatch table | `hpsvm` does not own the SBF interpreter; it delegates to agave. There is no opcode table to build. | [execution.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/execution.rs#L349-L389) calls `process_message` |
| `#[instruction]` macro as an opcode-definition mechanism | Opcodes are defined inside agave's interpreter, not in `hpsvm`. | [macros/lib.rs](file:///Volumes/akext/tmp/evm2/crates/macros/src/lib.rs#L17-L47) |
| Tail-call interpreter (`tco`) / `asm-keccak` / `dispatch_packed` backends | All live inside the interpreter `hpsvm` does not own. | `crates/evm2/src/interpreter/backend/*` |
| JIT pipeline (`crates/jit/*`) | Solana's SBF→native path is agave's `solana-sbpf` JIT. Re-implementing it would fork the semantic core and break mainnet fidelity. | `crates/jit/README.md` |
| Compile-time const-generic `SpecId`/`EvmConfig<BASE_SPEC_ID>` machinery | Solana protocol changes are runtime **feature gates**, not a linear compile-time spec enum. Compile-time specs cannot express feature-gate activation order or dependency. | [config.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/src/evm/config.rs#L61-L89) |
| `no_std + alloc` core | agave's runtime requires `std`. | `crates/hpsvm/Cargo.toml` |

### 2.2 Adapted — port the pattern, change the substance

| evm2 mechanism | hpsvm adaptation |
| :--- | :--- |
| `EvmTypesHost` associated-type family | `SvmTypesHost`: associated types for `SpecId`, `Tx`, `SvmExt`, `MessageExt`, `MessageResultExt`, `TxEnvExt`, `TxResultExt`, `BlockEnvExt`, `Host<'a>`. Direct structure, `()` defaults, same monomorphization story. |
| `Version` as consolidated runtime data | `Version` bundling `Arc<FeatureSet>`, fee structure, compute budget, rent, limits and cluster metadata. Runtime-derived, not const-generic. |
| `Host<T>` trait | `Host<T>` with account/sysvar/program-cache/block-env/log/record operations. Currently `&HPSVM` is passed implicitly; this becomes an explicit, swappable trait. |
| `TxRegistry` typed-in/erased-out | `TxRegistry<T, Output>` keyed by an envelope tag, plus an instruction-handler registry for builtins. |
| `Precompiles` registry + provider | `PrecompileSet<T>` with `NoPrecompiles` default and an agave-backed default set; per-instance `Box<dyn PrecompileProvider<T>>`. |
| `Inspector<T>` mutable hooks, all no-op | Extended inspector trait with `&mut self`, per-CPI call hooks, log and account-change hooks; `InspectMode` const parameter so the no-inspector path specializes away. |
| Journal + `StateCheckpoint` + `rollback` | `Accounts` overlay with `JournalEntry`, `StateCheckpoint`, `rollback`, `prewarm`. Replaces per-transaction post-account vectors and provenance-id validation. |
| `ExecutedTx` borrow-scoped commit | `ExecutedTx<'_, T>` with `.commit()`, `.discard()`, `.commit_with(&mut sink)`. Keeps a detached owned outcome for the parallel batch path. |
| Layered errors with `fatal` preserved through erasure | `AnyError` + `DbError{fatal}` + `LoadError`/`TxError`/`ExecutionError`/`HostError`/`HandlerError`. Replaces `HPSVMError`'s `String` reasons. |
| `StateChangeSink` for commit consumers | `StateChangeSink` trait; `commit_with` lets tooling observe bytecode/account/storage changes without a second pass. |
| `for_each_spec!` spec enumeration | `for_each_cluster_spec!` enumerating cluster profiles, generating selector tables. |
| Crate split | `hpsvm-macros`, `hpsvm-inspectors` as separate crates; `hpsvm-fixture` remains the EEST analogue. |

### 2.3 Direct — portable with no semantic change

- Extension types defaulting to `()` and monomorphizing to nothing.
- `Env` generics: `BlockEnv<E = ()>` and `TxEnv<E = ()>` carrying `*Ext` ([custom_evm/config.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/examples/custom_evm/config.rs#L88-L91) uses `l1_block_number`).
- `set_inspector`/`clear_inspector` with `downcast_ref` recovery.
- `Arc`-shared invariant data (feature set, program cache) instead of cloning.
- `cold_path()` on error branches, `#[inline]` on hot helpers, `#[inline(always)]` on cross-crate accessors.

### 2.4 The honest ceiling

`hpsvm`'s single largest cost is inside agave: the SBF program body executes in `process_message`. No amount of harness refactoring reduces that. The documented hot-path hotspots are, however, all harness-side — `clone_program_cache`, `load_accounts`, `calculate_fee`, `create_transaction_context`, `build_invoke_context` — and those are entirely within this refactor's reach. The correct framing is therefore: **architecture and DX parity with evm2 is fully achievable; raw interpreter parity is bounded by agave and is out of scope by design (AD-01).**

## 3. Target architecture

### 3.1 Type family

```rust
pub trait SvmTypesHost: Sized + 'static {
    type SpecId: Copy + Into<ClusterSpecId>;
    type Tx;
    type SvmExt;
    type MessageExt;
    type MessageResultExt;
    type TxEnvExt;
    type TxResultExt;
    type BlockEnvExt;
    type Host<'a>: Host<Self> + ?Sized;
}

pub trait SvmTypes: for<'a> SvmTypesHost<Host<'a> = Svm<'a, Self>> {}

pub struct BaseSvmTypes;

impl SvmTypesHost for BaseSvmTypes {
    type SpecId = ClusterSpecId;
    type Tx = VersionedTransaction;
    type SvmExt = ();
    type MessageExt = ();
    type MessageResultExt = ();
    type TxEnvExt = ();
    type TxResultExt = ();
    type BlockEnvExt = ();
    type Host<'a> = Svm<'a, Self>;
}

impl SvmTypes for BaseSvmTypes {}
```

### 3.2 Version and cluster profiles

`ClusterSpecId` is a small `#[repr(u32)]` enum of built-in cluster profiles (`MainnetBeta`, `Testnet`, `Devnet`, `Next`), each indexing a `&'static Version` table. Custom environments build on a base profile and override:

```rust
pub struct Version {
    pub feature_set: Arc<FeatureSet>,
    pub fee_structure: FeeStructure,
    pub compute_budget: ComputeBudget,
    pub rent: Rent,
    pub limits: RuntimeLimits,
    pub cluster: ClusterInfo,
    pub _non_exhaustive: (),
}

impl Version {
    pub const fn base(spec_id: ClusterSpecId) -> &'static Self;
    pub fn with_feature(self, id: FeatureId, enabled: bool) -> Self;
}
```

Then, per transaction, feature-gate queries are hoisted **once** into a `TxFeatureFlags` value instead of being re-evaluated at every call site:

```rust
pub struct TxFeatureFlags { /* one bool per gate used on the hot path */ }
impl TxFeatureFlags {
    pub fn load(feature_set: &FeatureSet) -> Self;
}
```

This directly mirrors the `evm2` rationale for putting gas parameters on the version: configuration changes then automatically affect every consumer without dependency tracking.

### 3.3 Host boundary

```rust
pub trait Host<T: SvmTypesHost> {
    fn version(&self) -> &Version;
    fn block(&self) -> &BlockEnv<T>;
    fn account(&mut self, addr: &Address) -> DbResult<Option<AccountHandle<'_, '_>>>;
    fn set_account(&mut self, addr: Address, account: AccountSharedData) -> DbResult<()>;
    fn sysvar<V: Sysvar>(&mut self) -> Result<V, LoadError>;
    fn program_cache(&self) -> &Arc<ProgramCache>;
    fn log(&mut self, message: &str);
}
```

### 3.4 The VM

```rust
pub struct Svm<'a, T: SvmTypesHost> {
    spec_id: T::SpecId,
    version: Version,
    block: BlockEnv<T>,
    registry: TxRegistry<T, TxResult<T>>,
    accounts: Accounts<'a>,
    ext: T::SvmExt,
    precompiles: Box<dyn PrecompileProvider<T> + 'a>,
    inspector: Option<Box<dyn Inspector<T> + 'a>>,
}
```

DX alias for the common case: `pub type Hpsvm = Svm<'static, BaseSvmTypes>;` — an ergonomic name, not a compatibility shim.

### 3.5 State: overlay + journal

```rust
pub struct Accounts<'a> {
    source: Box<dyn AccountSource + 'a>,
    cache: CacheDb,                    // account + storage overlay (evm2 CacheDB analogue)
    journal: Vec<JournalEntry>,        // every mutation, in order
    program_cache: Arc<ProgramCache>,  // Arc, never cloned per transaction
}

pub enum JournalEntry { Account(Address), Lamports(Address, u64), Data(Address), Wipe(Address), ... }
pub struct StateCheckpoint(usize);

impl Accounts<'_> {
    pub fn checkpoint(&self) -> StateCheckpoint;
    pub fn rollback(&mut self, checkpoint: StateCheckpoint);
    pub fn prewarm(&mut self, addrs: &[Address]);
    pub fn commit_source<S: StateChangeSink>(&self, sink: &mut S);
}
```

### 3.6 Borrow-scoped commit

```rust
pub struct ExecutedTx<'svm, 'a, T: SvmTypesHost> {
    svm: &'svm mut Svm<'a, T>,
    result: Option<TxResult<T>>,
}

impl<T: SvmTypesHost> ExecutedTx<'_, '_, T> {
    pub fn result(&self) -> &TxResult<T>;
    pub fn commit(self) -> TxResult<T>;
    pub fn discard(self) -> TxResult<T>;
    pub fn commit_with<S: StateChangeSink>(self, sink: &mut S) -> Result<TxResult<T>, S::Error>;
}

impl<T: SvmTypesHost> Svm<'_, T> {
    pub fn transact(&mut self, tx: impl Into<T::Tx>) -> Result<ExecutedTx<'_, '_, T>, HandlerError>;
    pub fn transact_detached(&self, tx: impl Into<T::Tx>) -> DetachedTx<T>; // owned; batch path
}
```

`origin_vm_instance_id` / `origin_state_version` provenance validation is deleted: the borrow checker makes stale-outcome misuse unrepresentable. The parallel batch path keeps `DetachedTx` plus a merged `StateChangeSet` applied through the journal, preserving the existing conflict-freedom invariant.

### 3.7 Inspector

```rust
pub trait Inspector<T: SvmTypesHost> {
    fn initialize(&mut self, _svm: &mut Svm<'_, T>) {}
    fn transaction_start(&mut self, _svm: &mut Svm<'_, T>, _tx: &SanitizedTransaction) {}
    fn instruction_start(&mut self, _cx: &mut InstructionCx<'_, T>) {}
    fn instruction_end(&mut self, _cx: &mut InstructionCx<'_, T>) {}
    fn cpi_start(&mut self, _depth: u8, _program_id: &Address) {}
    fn cpi_end(&mut self, _depth: u8, _program_id: &Address) {}
    fn log(&mut self, _message: &str) {}
    fn account_change(&mut self, _change: AccountChangeRef<'_>) {}
    fn transaction_end(&mut self, _result: &TxResult<T>) {}
}
```

All methods default to no-ops; the trait is `auto_impl(&mut, Box)`. Two additions that `evm2` lacks and `hpsvm` should keep, restructured: origin-aware routing becomes a `OriginFiltered<I>` decorator rather than 4 extra trait methods ([inspector.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/inspector.rs#L35-L71)), and `InvocationInspectCallback` (agave-level introspection) stays a separate capability. `InspectMode::{NoInspector, DynInspector}` is a const parameter on the execution entry point so the no-inspector path compiles the hooks away. Batteries move to `crates/hpsvm-inspectors`.

### 3.8 Registry

```rust
pub trait TxHandler<T: SvmTypesHost, Tx, Output> {
    fn validate(&self, tx: &Tx, host: &mut T::Host<'_>) -> Result<(), TxError>;
    fn execute(&self, tx: Tx, host: &mut T::Host<'_>) -> Result<Output, TxError>;
}

pub struct TxRegistry<T: SvmTypesHost, Output> { /* tag -> Arc<dyn AnyTxHandler<T, Output>> */ }
pub fn handler<F, G>(prepare: F, execute: G) -> impl TxHandler<...>;
```

Default handlers: legacy/versioned transaction, sanitize-and-execute, simulate. Instruction handlers cover builtins and precompiles.

### 3.9 Precompiles

```rust
pub trait Precompile<T: SvmTypesHost> {
    fn call(&self, cx: &mut PrecompileCx<'_, T>) -> Result<(), PrecompileError>;
}
pub struct PrecompileSet<T> { /* program id -> handler */ }
pub struct NoPrecompiles;
pub struct NativePrecompiles<T>;   // agave-backed default set
```

### 3.10 Errors

```rust
pub struct AnyError(Arc<dyn Error + Send + Sync>);
pub struct DbError { error: AnyError, fatal: bool }
pub struct HandlerError(AnyError);

pub enum LoadError { Account(DbError), Program(DbError), Sysvar(DbError) }
pub enum TxError { Validation(TxValidationError), Execution(ExecutionError), Database(DbError) }
pub enum ExecutionError { Database(DbError), Fatal(Box<AnyError>), Halt(HaltReason) }
pub enum HostError { Halt(HaltReason), Execution(ExecutionError) }

pub type DbResult<X> = Result<X, DbError>;
```

### 3.11 Crate layout

```text
crates/
├── hpsvm/             # core: types, version, host, registry, state, execution harness, precompiles
├── hpsvm-macros/      # #[svm_instruction] / #[svm_tx_handler] proc macros
├── hpsvm-inspectors/  # CU accounting, coverage, tracing, counting
├── hpsvm-fixture/     # fixtures + CU benching (EEST analogue, retained)
├── hpsvm-test-support/# retained
└── hpsvm-fork/        # optional: RPC-backed AccountSource (split out of `hpsvm`'s `fork` feature)
bin/hpsvm-cli/
```

### 3.12 Developer experience target

A `custom_svm` example mirroring [custom_evm](file:///Volumes/akext/tmp/evm2/crates/evm2/examples/custom_evm/main.rs), demonstrating: a custom `SvmTypesHost` with a non-empty `BlockEnvExt` (e.g. `l1_slot`), a custom transaction handler registered in `TxRegistry`, a custom builtin instruction declared with `#[svm_instruction]`, a custom `Precompile`, and both a `commit()` and a `commit_with(&mut sink)` flow.

## 4. Findings

### Finding 1: Monolithic `HPSVM` with no type-level configuration surface

- **Category:** architecture
- **Impact:** HIGH
- **Effort:** L
- **Risk:** MED — touches every module, but is mechanical when done in the order given in `tasks.md`.

#### Requirements (EARS Notation)

- **[REQ-01]:** The crate SHALL expose an `SvmTypesHost` trait with associated types `SpecId`, `Tx`, `SvmExt`, `MessageExt`, `MessageResultExt`, `TxEnvExt`, `TxResultExt`, `BlockEnvExt`, `Host<'a>`.
- **[REQ-02]:** A `BaseSvmTypes` family SHALL exist whose extension types are all `()`, and `Svm<'a, BaseSvmTypes>` SHALL be functionally equivalent to the current `HPSVM`.
- **[REQ-03]:** `Svm<'a, T>` SHALL implement `Host<T>` for every `T: SvmTypesHost`.
- **[REQ-04]:** Adding an extension type SHALL NOT require modifying any module in the execution path.

#### Current state

- [lib.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/lib.rs#L369-L397): `HPSVM` with 20+ fields, concrete types only.
- [env.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/env.rs#L8-L28): `BlockEnv` and `RuntimeEnv` have no extension slot.
- [builder.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/builder.rs#L125-L187): configuration is a runtime builder, not a type surface.

#### Approach

Introduce the type family and `Svm<'a, T>` alongside the existing struct, migrate call sites module by module, then delete `HPSVM`. `BaseSvmTypes` keeps public-behaviour tests green throughout.

#### Architecture Decisions (MADR Format)

- **AD-01:** Keep agave `solana-program-runtime` as the semantic execution core. No bespoke SBF interpreter and no JIT work in this refactor. Rationale: mainnet fidelity is `hpsvm`'s primary value; a forked interpreter would diverge silently. Consequence: the `#[instruction]` macro and dispatch-table machinery are deliberately not ported (§2.1).
- **AD-02:** Configuration is a runtime `Version` object built on `FeatureSet`, not compile-time const-generic spec machinery. Rationale: Solana protocol changes are runtime feature gates with dependency ordering, which a linear compile-time spec enum cannot express. Consequence: no `SvmConfig<const BASE_SPEC_ID>`; custom environments override `Version` values.
- **AD-03:** Extension points are associated types defaulting to `()`. Rationale: zero cost when unused, and it keeps `BaseSvmTypes` free of dead fields.

### Finding 2: Configuration scattered across three structs and re-evaluated per call site

- **Category:** architecture / performance
- **Impact:** HIGH
- **Effort:** M
- **Risk:** MED — consolidation must preserve exact fee and rent semantics.

#### Requirements (EARS Notation)

- **[REQ-01]:** A single `Version` value SHALL carry feature set, fee structure, compute budget, rent, limits and cluster metadata.
- **[REQ-02]:** `Version::base(spec_id)` SHALL return a `&'static Version` and SHALL be evaluable in a `const` context.
- **[REQ-03]:** Feature-gate queries on the hot path SHALL be resolved once per transaction into a `TxFeatureFlags` value.
- **[REQ-04]:** Custom environments SHALL override `Version` fields without editing core modules.

#### Current state

- [env.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/env.rs#L13-L28): `SvmCfg`, `RuntimeEnv`, `BlockEnv` overlap.
- `feature_set.is_active(...)` is consulted at multiple scattered sites rather than once per transaction.

#### Approach

Collapse the three structs into `Version` + `BlockEnv<T>`. Compute `TxFeatureFlags` immediately after the version is selected and thread the value (not the `FeatureSet`) through the execution path.

#### Architecture Decisions (MADR Format)

- **AD-04:** Feature queries are hoisted into `TxFeatureFlags` once per transaction rather than cached globally, because feature sets can be reconfigured between transactions and a global cache would go stale.

### Finding 3: Full post-account vector per transaction and provenance-id validation

- **Category:** performance / correctness
- **Impact:** HIGH
- **Effort:** M
- **Risk:** MED — rollback correctness is the critical invariant.

#### Requirements (EARS Notation)

- **[REQ-01]:** State mutation SHALL be recorded as `JournalEntry` values in an ordered journal.
- **[REQ-02]:** `Accounts` SHALL support `checkpoint()` and `rollback(checkpoint)`.
- **[REQ-03]:** Committing a transaction SHALL apply the journal, not materialize a full post-account vector.
- **[REQ-04]:** Stale-outcome misuse SHALL be prevented by borrowing rather than by runtime provenance-id checks.

#### Current state

- [types.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/types.rs#L154-L165): `ExecutionOutcome` owns `post_accounts: Vec<(Address, AccountSharedData)>` plus `origin_vm_instance_id`/`origin_state_version`.
- [commit.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/commit.rs#L90-L124): `commit_execution_outcome` validates those ids, then applies the delta.
- Reference model: [journal.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/src/evm/state/journal.rs#L40), [state/mod.rs rollback](file:///Volumes/akext/tmp/evm2/crates/evm2/src/evm/state/mod.rs#L723).

#### Approach

Replace `ExecutionOutcome` with `ExecutedTx` (§3.6) for the single-transaction path and `DetachedTx` + `StateChangeSet` for the batch path. Rollback replaces the current "discard the clone" strategy, so failed transactions stop paying for a full account copy.

#### Architecture Decisions (MADR Format)

- **AD-05:** The journal is `Vec<JournalEntry>` rather than a per-address dirty map, because ordered replay is required for correct nested-CPI rollback.
- **AD-06:** The batch path keeps an owned, `Send` detached outcome rather than forcing the borrowed `ExecutedTx` through `thread::scope`; parallelism needs owned data.

### Finding 4: `AccountsDb` is a single flat map; program cache is cloned per transaction

- **Category:** performance
- **Impact:** HIGH
- **Effort:** M
- **Risk:** MED — cache layering must not change observable account semantics.

#### Requirements (EARS Notation)

- **[REQ-01]:** Accounts SHALL be layered as `CacheDb` overlay over an `AccountSource` backend.
- **[REQ-02]:** The program cache SHALL be shared as `Arc<ProgramCache>` and SHALL NOT be cloned per transaction.
- **[REQ-03]:** `Accounts` SHALL expose `prewarm` to batch-load accounts before a stage.
- **[REQ-04]:** `commit_with(&mut sink)` SHALL stream account/bytecode/storage changes to a consumer.

#### Current state

- [accounts_db.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/accounts_db.rs#L117-L124): `AccountsDb` holds `source`, `inner`, `removed`, `programs_cache`, `sysvar_cache`, `environments` in one struct.
- Documented hot path includes `clone_program_cache` and `load_accounts`.
- Reference model: [db/cache.rs `CacheDB`](file:///Volumes/akext/tmp/evm2/crates/evm2/src/evm/db/cache.rs#L109), [state `prewarm`](file:///Volumes/akext/tmp/evm2/crates/evm2/src/evm/state/mod.rs#L396-L410), [`StateChangeSink`](file:///Volumes/akext/tmp/evm2/crates/evm2/src/evm/state/mod.rs#L229-L237).

#### Approach

Split `AccountsDb` into `Accounts` (overlay + journal + source) and `CacheDb` (account/storage overlay), wrap the program cache in `Arc`, and add `prewarm` driven by the batch planner's read-set.

#### Architecture Decisions (MADR Format)

- **AD-07:** Prewarming is driven by the batch planner's already-computed read-set rather than by a speculative scan, because the planner already knows the exact account set per stage.

### Finding 5: Inspector is immutable, always invoked, and lacks CPI-level hooks

- **Category:** DX / performance
- **Impact:** MEDIUM
- **Effort:** M
- **Risk:** LOW — the inspector is observational.

#### Requirements (EARS Notation)

- **[REQ-01]:** `Inspector<T>` hooks SHALL take `&mut self` and default to no-ops.
- **[REQ-02]:** The execution path SHALL be monomorphized over `InspectMode` so the no-inspector path pays nothing.
- **[REQ-03]:** `set_inspector`/`clear_inspector` SHALL use `Box<dyn Inspector<T>>` with `downcast_ref` recovery.
- **[REQ-04]:** Origin filtering SHALL be a decorator and SHALL NOT be four extra trait methods.
- **[REQ-05]:** Per-CPI `cpi_start`/`cpi_end` and an `account_change` hook SHALL exist.

#### Current state

- [inspector.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/inspector.rs#L30-L71): `&self` methods, four origin variants, three hooks.
- `Arc<dyn Inspector>` is invoked unconditionally even when it is `NoopInspector`.

#### Approach

Redefine the trait per §3.7, introduce `OriginFiltered<I>`, add `InspectMode` to the execution entry point, and move reusable inspectors into `crates/hpsvm-inspectors`.

#### Architecture Decisions (MADR Format)

- **AD-08:** Origin filtering is a decorator rather than duplicated trait methods, because combinator-style composition keeps the trait surface small and composes with future filters.

### Finding 6: Flat error enum with `String` reasons loses classification

- **Category:** DX / correctness
- **Impact:** MEDIUM
- **Effort:** M
- **Risk:** LOW.

#### Requirements (EARS Notation)

- **[REQ-01]:** Errors SHALL be layered as `AnyError` → `DbError`/`LoadError`/`TxError`/`ExecutionError`/`HostError`/`HandlerError`.
- **[REQ-02]:** `DbError` SHALL carry a `fatal` classification that survives type erasure.
- **[REQ-03]:** Concrete error types SHALL be recoverable from an erased error via `downcast_ref`.
- **[REQ-04]:** I/O and instruction errors SHALL convert into the appropriate layer via `From`.

#### Current state

- [error.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/error.rs#L43-L79): `HPSVMError` with `String` payloads in `SysvarSerialization`, `RuntimeEnvironment`, `CustomSyscallRegistration`, `AccountSource`, `ProgramLoad`.
- Reference model: [error.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/src/error.rs#L71-L153).

#### Approach

Introduce the layered taxonomy, keep `#[from]` conversions, and preserve the existing assertions about rendered messages where the messages are part of tested behaviour.

#### Architecture Decisions (MADR Format)

- **AD-09:** `AnyError` wraps `Arc<dyn Error + Send + Sync>` rather than `Box<dyn Error>` because errors cross thread boundaries in the batch path.

### Finding 7: Transaction and instruction handling is hardcoded

- **Category:** architecture / DX
- **Impact:** MEDIUM
- **Effort:** M
- **Risk:** MED.

#### Requirements (EARS Notation)

- **[REQ-01]:** A `TxRegistry<T, Output>` SHALL dispatch transaction handling by envelope tag.
- **[REQ-02]:** Handlers SHALL be authored against concrete `Tx` types and stored type-erased.
- **[REQ-03]:** Builtin instruction handlers SHALL be registrable.
- **[REQ-04]:** A `#[svm_instruction]` macro SHALL generate the handler type, argument binding and table registration from a plain function.

#### Current state

- [lib.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/lib.rs#L765-L873): `add_builtin`, `add_program`, `add_program_with_loader` — registration is manual.
- [execution.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/execution.rs#L349-L389): a single hardcoded execution path.
- Reference model: [registry.rs](file:///Volumes/akext/tmp/evm2/crates/evm2/src/evm/registry.rs#L170-L402), [macros/lib.rs](file:///Volumes/akext/tmp/evm2/crates/macros/src/lib.rs#L17-L47).

#### Approach

Introduce `TxRegistry` + `TxHandler` with default handlers for legacy/versioned/simulate, and a reduced `#[svm_instruction]` macro scoped to `hpsvm`'s own builtins and precompiles. The macro is explicitly a **partial** analogue of `#[instruction]`: it serves `hpsvm`-owned instructions only, never SBF program dispatch (§2.1).

#### Architecture Decisions (MADR Format)

- **AD-10:** The instruction macro targets builtins and precompiles only. Extending it to SBF program dispatch is impossible without forking agave (AD-01), so the macro's contract is documented as hpsvm-owned instructions exclusively.

### Finding 8: Precompiles are a feature-gated bulk switch

- **Category:** architecture / DX
- **Impact:** MEDIUM
- **Effort:** M
- **Risk:** LOW.

#### Requirements (EARS Notation)

- **[REQ-01]:** A `Precompile<T>` trait SHALL exist with a CU-aware context.
- **[REQ-02]:** A `PrecompileSet<T>` registry SHALL exist, keyed by program id, defaulting to `NoPrecompiles`.
- **[REQ-03]:** A `NativePrecompiles<T>` set SHALL wrap agave's default precompiles.
- **[REQ-04]:** A `Svm` instance SHALL accept a per-instance precompile provider.

#### Current state

- `builder.rs` / `lib.rs` expose `with_precompiles()` and a `precompiles` feature (`extract_secp256k1_instruction`), a single on/off switch.
- Reference model: `crates/evm2/src/evm/precompile/*`, `PrecompileProvider` on `Evm`.

#### Approach

Replace the boolean switch with a registry and provider, defaulting to `NativePrecompiles<T>`, matching today's behaviour out of the box.

#### Architecture Decisions (MADR Format)

- **AD-11:** `NoPrecompiles` is the type-level default and `NativePrecompiles<T>` the behavioural default, mirroring evm2's split between the empty type and the batteries-included set.

### Finding 9: Parallelism model has no journal-based merge path

- **Category:** performance / architecture
- **Impact:** MEDIUM
- **Effort:** M
- **Risk:** MED — must preserve the conflict-freedom invariant.

#### Requirements (EARS Notation)

- **[REQ-01]:** Batch workers SHALL share a read-only account snapshot via `Arc`.
- **[REQ-02]:** Each worker SHALL produce a `StateChangeSet`.
- **[REQ-03]:** The staging loop SHALL merge change sets through the same journal used by the single path.
- **[REQ-04]:** The conflict-freedom invariant SHALL hold: workers never observe another worker's partial writes.

#### Current state

- [batch.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/batch.rs#L15-L39): `TransactionBatchStage`/`TransactionBatchPlan`/`TransactionBatchExecutionResult`; `send_transaction_batch` applies deltas via `apply_commit_delta`.
- `specs/2026-06-17-02-performance` Finding 5 already moved workers to `Arc<AccountsDb>`; this finding builds on that completed work rather than re-planning it.

#### Approach

Reuse the journal as the single merge mechanism for both paths: `StateChangeSet` is a serializable journal slice, and `apply_commit_delta` becomes `journal::extend_from_set`.

#### Architecture Decisions (MADR Format)

- **AD-12:** `StateChangeSet` is a journal slice rather than a `HashMap`, because ordered replay is needed if a merged stage is later reverted.

## 5. Non-goals

- No bespoke SBF interpreter, no SBF JIT, no interpreter backend selection (AD-01).
- No compile-time const-generic spec machinery (AD-02).
- No `no_std` port while agave requires `std`.
- No port of evm2's 256-entry dispatch table, `tco`, `asm-keccak` or `dispatch_packed` (§2.1).
- No change to execution semantics: observable results must remain byte-identical to the current implementation, verified by the fixture compare harness.

## 6. Test Strategy

- **Primary Language:** Rust 2024.
- **Unit Test Command:** `cargo test --all-features`.
- **Workspace gates:** `just format`, `just lint`, `just test-all`, `just mutants-quick` (per `AGENTS.md`).
- **Behavioural equivalence:** the fixture compare harness in `crates/hpsvm-fixture` must produce identical results pre- and post-refactor on the existing corpus.
- **Property tests:** `proptest` for journal invariants — `rollback(checkpoint)` restores byte-identical account state; journal replay is idempotent; merged `StateChangeSet`s from disjoint workers commute.
- **Mutation testing:** `.cargo/mutants.toml`; the journal and `Version` code paths must be covered by assertions specific enough to kill boundary mutants.
- **Error taxonomy:** table-driven tests asserting exact `Downcast`/`From` behaviour per layer, not `is_err()`.

## 7. Benchmark and performance gates

Baselines live in `docs/benchmarks/runtime-baselines` and `docs/benchmarks/baselines`.

- `just bench-runtime` MUST NOT regress against `runner`, and MUST show improvement on the harness hotspots `clone_program_cache`, `load_accounts` and the transaction-context construction path.
- `just bench-hotpath` MUST NOT regress against the recorded baseline.
- `just bench-runtime-baseline-compare` is the acceptance gate for every performance finding.
- Diagnostics remain opt-in (`SvmCfg`/`Version` flag), building on `specs/2026-06-17-02-performance` Finding 6.

## 8. Landing order and risk

The order is chosen so the public behaviour stays green at every step: type surface first (mechanical, no semantics), then configuration consolidation, then state/commit, then cache layering, then inspector, then registries, then DX and hardening. Each phase is independently revertible and gated by the fixture compare harness.

Residual risks and mitigations:

- **Rollback correctness** — the highest-risk change. Mitigation: `proptest` journal invariants plus fixture equivalence gates before the phase is marked done.
- **Batch invariant regression** — mitigation: the existing conflict-freedom tests from `specs/2026-06-17-02-performance` remain the gate.
- **Fee/rent drift during `Version` consolidation** — mitigation: exact-value assertions on fee and rent calculations.

## 9. Evidence index

| Ref | Location |
| :--- | :--- |
| E1 | `crates/evm2/src/evm/config.rs` — `EvmTypesHost`, `EvmConfig`, `ExecutionConfig`, `BaseEvmConfig` |
| E2 | `crates/evm2/src/version/mod.rs` — `Version` |
| E3 | `crates/evm2/src/interpreter/dispatch/mod.rs`, `dispatch/table/mod.rs` — const-fn dispatch tables |
| E4 | `crates/macros/src/lib.rs` — `#[instruction]` macro |
| E5 | `crates/evm2/src/interpreter/host.rs` — `Host<T>` |
| E6 | `crates/evm2/src/evm/registry.rs` — `TxRequest`, `TxHandler`, `AnyTxHandler`, `TxRegistry` |
| E7 | `crates/evm2/src/evm/inspector.rs` — `Inspector<T>` |
| E8 | `crates/evm2/src/evm/state/mod.rs`, `state/journal.rs`, `state/pending.rs` — journal state model |
| E9 | `crates/evm2/src/evm/db/cache.rs` — `CacheDB` |
| E10 | `crates/evm2/src/evm/tx.rs` — `ExecutedTx` |
| E11 | `crates/evm2/src/error.rs` — layered errors |
| E12 | `crates/evm2/src/evm/mod.rs` — `Evm<'a, T>` |
| E13 | `crates/evm2/examples/custom_evm/*` — DX reference |
| E14 | `crates/hpsvm/src/lib.rs`, `vm.rs`, `env.rs`, `types.rs`, `commit.rs`, `accounts_db.rs`, `batch.rs`, `inspector.rs`, `error.rs`, `execution.rs`, `builder.rs` — current state |
| E15 | `specs/2026-06-17-02-performance/{design,tasks}.md` — completed predecessor work |
