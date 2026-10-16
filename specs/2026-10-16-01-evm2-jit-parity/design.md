# Design: evm2 JIT parity — compiled-program tiering, resident entry store, and observability

| Metadata | Details |
| :--- | :--- |
| **Status** | Draft |
| **Created** | 2026-10-16 |
| **Mode** | Full |
| **Priority** | P1 |
| **Planned at** | commit `a884b47` |
| **Reference project** | `/Volumes/akext/tmp/evm2` (crate family `crates/jit/*`, conformance harness `crates/eest`) |
| **Companion spec** | `specs/2026-10-15-01-evm2-parity` (landed in `d5137f7`); this spec does not supersede it |
| **Scope** | The tiering, caching, prewarming, and observability machinery that surrounds the SBF compiler. The compiler itself is out of scope. |

## Summary

> The parity spec classified `crates/jit/*` as **blocked** because the SBF→native compiler belongs to agave (`solana-sbpf`), not to `hpsvm`. That verdict was correct about the *compiler*, and this document re-examines only the machinery *around* it — all of which lives in `hpsvm`'s own code and none of which exists today. The findings: agave 4.5 already contains a tiered-execution seam (`ExecutionMode::PreferJit` + `Executable::get_compiled_program` / `jit_compile`) that `hpsvm` never exercises, because every program load goes through `ProgramCacheEntry::load`, which unconditionally JIT-compiles on x86_64 at load time; program entries are re-verified and re-JIT-compiled on every cache rebuild and on every non-memoized `add_program`; there is no background compilation stage, no promotion policy, no resident-entry store keyed by program content, no visibility of compiled-vs-interpreted execution, and no interpreted/compiled equivalence gate. The plan ports five transferable `evm2-jit` mechanisms — the tiering policy (`JitPolicy` + hot-threshold promotion mirroring `RuntimeTuning`/`JitBackend::lookup`), the resident entry store (mirroring `ResidentMap` + `ArtifactStore`), the background worker pool (mirroring the rayon compile pool), the stats surface (mirroring `RuntimeStats`/`CompilationEvent`), and the conformance gate (mirroring the EEST JIT run) — into `hpsvm`'s `Accounts`/`Version`/fixture layers. Two bounding decisions: agave remains the semantic core (parity AD-01 restated), and the default behavior stays byte-identical to today (`Eager`, no promotion, agave's own load path).

## Why this matters

The user operates a MEV searcher that builds and simulates transactions in bulk against a forked-state VM. Two costs dominate the harness around agave:

1. **Program compilation is paid over and over.** `ProgramCacheEntry::load` (ELF parse + verify + JIT compile) runs on every `add_program` for user programs — only `include_bytes!` statics under the default environment are memoized ([runtime_env.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/runtime_env.rs#L173-L210)) — and on **every** program in the overlay whenever `rebuild_program_cache` runs ([accounts_db.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/accounts_db.rs#L640-L675)): after `ExecutedTx::discard` on a cache-dirty transaction ([executed.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/executed.rs#L236-L245)), after a failed feature-set reconfiguration, and after any account mutation that touches a program account. A simulation that discards (the common case in a search loop) can therefore pay a full re-JIT of the program set.
2. **There is no policy, no tiering, and no visibility.** On x86_64 every program is compiled eagerly whether it runs zero times or a million; on arm64 nothing is ever compiled; nothing reports which mode actually ran. `evm2-jit` solved exactly this class of problem for EVM bytecode with a small, regular runtime, and every piece of it has a corresponding seam in agave 4.5 that `hpsvm` can drive without forking anything.

## 1. Reference analysis: what makes evm2-jit good

### 1.1 The compiler is one pipeline with an abstract backend

`EvmCompiler<B: Backend>` ([compiler/mod.rs](file:///Volumes/akext/tmp/evm2/crates/jit/codegen/src/compiler/mod.rs#L87-L113)) drives analysis → translation → optimization → codegen behind a backend trait ([traits.rs](file:///Volumes/akext/tmp/evm2/crates/jit/backend/src/traits.rs#L211-L277): `build_function`/`optimize_module`/`jit_function`/`write_object`). Bytecode is parsed into a CFG with blocks, gas sections and stack info ([bytecode/mod.rs](file:///Volumes/akext/tmp/evm2/crates/jit/codegen/src/bytecode/mod.rs#L1-L46)), with dedicated passes (dedup, peephole). For `hpsvm` the entire pipeline is agave's; what transfers is the *shape*: a policy object that decides **whether, when, and on which thread** a program becomes native code.

### 1.2 Tiering is a runtime, not a flag

`JitBackend` ([runtime/mod.rs](file:///Volumes/akext/tmp/evm2/crates/jit/runtime/src/runtime/mod.rs#L139-L249)) keeps a lock-free `ResidentMap` and an event queue. `lookup` never blocks: resident hit → run compiled; miss → push a fire-and-forget observation event and run interpreted. A backend thread drains events, and once a key crosses `jit_hot_threshold` (default 8) a worker compiles it. Eligibility is gated by `RuntimeTuning::should_compile` ([config.rs](file:///Volumes/akext/tmp/evm2/crates/jit/runtime/src/runtime/config.rs#L347-L359), max 24 KiB), blocking mode exists for tests, and every failure path degrades to the interpreter (`LookupMissReason::JitFailed`). agave 4.5 exposes the identical receiving end: `execute_program` runs `ExecutionMode::PreferJit` — if `executable.get_compiled_program()` is `Some`, native code runs, otherwise the interpreter runs and the mode is recorded as `Interpreted` ([sbpf vm.rs](file:///Users/akagi201/.cargo/registry/src/rsproxy.cn-e3de039b2554c837/solana-sbpf-0.25.0/src/vm.rs#L438-L474)). agave's `process_instruction` hardcodes `PreferJit` ([program-runtime vm.rs](file:///Users/akagi201/.cargo/registry/src/rsproxy.cn-e3de039b2554c837/solana-program-runtime-4.5.0-alpha.2/src/vm.rs#L281)). The promotion hook `Executable::jit_compile(&self)` publishes the compiled program under interior mutability ([sbpf elf.rs](file:///Users/akagi201/.cargo/registry/src/rsproxy.cn-e3de039b2554c837/solana-sbpf-0.25.0/src/elf.rs#L438-L447)) — so a *later* invocation picks it up with zero coordination. That is evm2's tiering loop, already waiting to be driven.

### 1.3 The resident map and the artifact store are the same idea

Runtime lookup is O(1) against `ResidentMap`; persisted artifacts are keyed by a full `ArtifactKey { code_hash, spec_id, backend, opt_level, abi_version }` ([storage.rs](file:///Volumes/akext/tmp/evm2/crates/jit/runtime/src/runtime/storage.rs#L9-L31)) and preloaded at startup ([runtime/mod.rs](file:///Volumes/akext/tmp/evm2/crates/jit/runtime/src/runtime/mod.rs#L581-L629)). `hpsvm` already has a primitive of exactly this kind — the process-wide `DEFAULT_PROGRAM_CACHE` memo keyed by environment identity + ELF slice identity — but it is restricted to `include_bytes!` statics under the default environment ([runtime_env.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/runtime_env.rs#L49-L63)). Generalizing the key from slice identity to program content is the whole port.

### 1.4 Compilation runs off the critical path, on a bounded pool

`worker.rs` ([runtime/worker.rs](file:///Volumes/akext/tmp/evm2/crates/jit/runtime/src/runtime/worker.rs#L1-L50)) owns a rayon pool sized `clamp(cpus/2, 1, 4)` with bounded queues, a sync-notifier for blocking callers, and per-job timings. Dropped events are counted, never fatal. `hpsvm` already depends on rayon for the batch path; the same pool shape applies to program promotion and prewarming.

### 1.5 The stats surface makes the tier visible

`RuntimeStats` (lookup hits/misses, resident entries, compilations, dropped events/commands) and `CompilationEvent { duration, timings }` ([config.rs](file:///Volumes/akext/tmp/evm2/crates/jit/runtime/src/runtime/config.rs#L135-L150)) are the only way to reason about whether tiering pays off. agave already records per-entry `ProgramStatistics` (compilations, compile-time EMA, interpreter/jit invocation counts and execution EMAs — [program_metrics.rs](file:///Users/akagi201/.cargo/registry/src/rsproxy.cn-e3de039b2554c837/solana-program-runtime-4.5.0-alpha.2/src/program_metrics.rs#L20-L90)); `hpsvm` reads none of it today.

### 1.6 The conformance gate runs the corpus under both modes

`evm2-eest` constructs a blocking, non-optimized JIT backend and runs the state/blockchain test suites through it (`cargo st "statetests::devnet::jit"` — [eest/src/jit.rs](file:///Volumes/akext/tmp/evm2/crates/eest/src/jit.rs#L7-L30)), i.e. JIT vs interpreter equivalence is a first-class test mode, not an afterthought. `hpsvm`'s `crates/hpsvm-fixture` compare harness is the analogous mechanism and already compares recorded outcomes; it simply has no execution-mode axis.

### 1.7 The bench harness measures the right things per benchmark

`scripts/jit/bench.py` compares revisions on codegen size, compile instructions (via `perf stat`), jump-resolution statistics and runtime instructions, per benchmark, with verbatim results tables ([jit/AGENTS.md](file:///Volumes/akext/tmp/evm2/crates/jit/AGENTS.md#L94-L165)). `hpsvm` already has the criterion + baseline-compare infrastructure (`just bench-runtime`, `scripts/criterion_export.py`, `scripts/criterion_compare.py`, `docs/benchmarks/runtime-baselines`); this spec extends it to the program-cache path instead of inventing a new harness.

## 2. Transferability analysis

### 2.1 Blocked — do not port

| evm2-jit mechanism | Why it does not transfer | Evidence |
| :--- | :--- | :--- |
| The compiler itself (analysis, translation, IR, LLVM backend) | SBF→native compilation is agave's `solana-sbpf` JIT, compiled into the dependency graph (`solana-program-runtime` enables `solana-sbpf` with `features = ["jit"]`). Forking it breaks mainnet fidelity (parity AD-01). | [program-runtime Cargo.toml](file:///Users/akagi201/.cargo/registry/src/rsproxy.cn-e3de039b2554c837/solana-program-runtime-4.5.0-alpha.2/Cargo.toml#L131-L134) |
| Backend abstraction trait (`Backend`, `Builder`, `TypeMethods`) | There is no second backend to abstract over; `Backend` exists in evm2 because codegen was swappable. | [backend/traits.rs](file:///Volumes/akext/tmp/evm2/crates/jit/backend/src/traits.rs#L211-L277) |
| Peephole / dedup / DSE / LLVM pass pipeline, size-gated LICM | Inside sbpf; `hpsvm` cannot influence codegen quality. | [codegen/src/compiler/translate/peephole.rs](file:///Volumes/akext/tmp/evm2/crates/jit/codegen/src/compiler/translate/peephole.rs) |
| Persistent on-disk compiled artifacts (`ArtifactStore` of dylibs) | sbpf's JIT output has no stable artifact format and no object writer; a disk store would be an unsupported ABI. The in-process resident store covers the searcher's VM lifecycle (VMs are created per block within one process). | [storage.rs](file:///Volumes/akext/tmp/evm2/crates/jit/runtime/src/runtime/storage.rs#L73-L94) |
| Out-of-process JIT helper (`JitMode::OutOfProcess`) | Validator-scale isolation concern; a simulator compiles a handful of programs. | [runtime/config.rs](file:///Volumes/akext/tmp/evm2/crates/jit/runtime/src/runtime/config.rs#L160-L172) |
| Gas kept in a register against `ecx.gas` | SBF CU metering is sbpf's `ContextObject`; unreachable from `hpsvm`. | sbpf `vm.rs` `ContextObject::consume` |

### 2.2 Adapted — port the pattern, change the substance

| evm2-jit mechanism | hpsvm adaptation |
| :--- | :--- |
| `RuntimeTuning` + `JitBackend::lookup` tiering | `JitPolicy` on `Version` + `TierPromoter` sweeping agave's per-entry `ProgramStatistics`. agave's `ExecutionMode::PreferJit` supplies the interpreter/compiled switch for free. |
| `ResidentMap` (in-memory, O(1)) | Process-wide content-keyed `ProgramEntryStore`: `(env identity, program content hash, loader, slot bucket) → Arc<ProgramCacheEntry>`, generalizing `DEFAULT_PROGRAM_CACHE`. |
| Rayon compile worker pool + bounded queues | A `TierPromoter` pool (same sizing rule) plus `Accounts::prewarm_programs` for batch stages, reusing the existing rayon dependency. |
| `RuntimeStats` / `CompilationEvent` | `ProgramTierStats` snapshot assembled from agave's `ProgramStatistics` plus `hpsvm`-side promotion counters. |
| EEST JIT test mode | A policy axis in the fixture compare harness: same corpus under `Off` (forced interpreted) and `Eager`. |
| `blocking` mode for tests | `JitPolicy::Lazy { hot_threshold: 0 }` acting as "compile synchronously at first use" for deterministic tests. |

### 2.3 Direct — portable with no semantic change

- Compile-time elimination of the whole surface on non-x86_64: sbpf gates JIT on `feature = "jit" && target_arch = "x86_64"`, and `ProgramCacheEntry::load` only calls `jit_compile` under the same cfg — so every policy collapses to interpreted off x86_64 automatically.
- `ExecutionMode::PreferJit` fallback semantics: an entry that fails `jit_compile` simply keeps interpreting (evm2's `LookupMissReason::JitFailed` fallback, already built in).
- Promotion via interior mutability needs no lock on the cache: `jit_compile(&self)` publishes atomically; the next invocation observes it.
- Non-blocking event submission with drop-and-count semantics when the pool is saturated (evm2's `events_dropped`).
- The `#[cfg]` + skip-don't-fail test convention already used for the SBF toolchain gap extends naturally to JIT-specific tests.

### 2.4 The honest ceiling

`hpsvm` cannot make SBF programs run faster than sbpf runs them; it can only stop paying to compile programs that do not need compiling, stop recompiling programs it has already compiled, move the remaining compilation off the critical path, and make all of it observable. Everything above is hpsvm-side code. The parity spec's framing still holds: **runtime dispatch parity is bounded by agave; harness-side compiled-program costs are entirely within reach.**

## 3. Target architecture

### 3.1 Policy as runtime data

Following parity AD-02 (`Version` carries configuration as data), compilation policy is one enum on `Version`, selected per VM:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JitPolicy {
    /// Compile every loaded program to native code (x86_64 only).
    /// Today's behavior via agave's `ProgramCacheEntry::load`.
    Eager,
    /// Interpret until a program crosses `hot_threshold` invocations,
    /// then promote it on a background pool.
    Lazy { hot_threshold: usize },
    /// Never compile; force the interpreter even on x86_64. Diagnostics.
    Off,
}

impl JitPolicy {
    pub const DEFAULT: Self = Self::Eager;
    /// Whether the load path must bypass `jit_compile` (only meaningful on x86_64).
    pub const fn requires_uncompiled_load(self) -> bool { !matches!(self, Self::Eager) }
    /// The policy as observed on this target: everything collapses to `Off` off x86_64.
    pub const fn effective(self) -> Self { if cfg!(target_arch = "x86_64") { self } else { Self::Off } }
}
```

### 3.2 The uncompiled load path

Under `Lazy`/`Off` on x86_64, `hpsvm` builds entries itself instead of calling `ProgramCacheEntry::load` (which unconditionally compiles):

```rust
pub(crate) fn load_program_uncompiled(
    loader_key: &Address,
    env: ProgramRuntimeEnvironment,
    elf_bytes: &[u8],
) -> Result<ProgramCacheEntry, InstructionError> {
    let executable = Executable::load(elf_bytes, Arc::clone(&*env))?;
    executable.verify::<RequisiteVerifier>()?;
    Ok(ProgramCacheEntry {
        program: ProgramCacheEntryType::Loaded(executable),
        account_owner: ProgramCacheEntryOwner::try_from(loader_key).map_err(|_| InstructionError::IncorrectProgramId)?,
        deployment_slot: IMMEDIATE_DEPLOYMENT_SLOT,
        stats: Arc::new(ProgramStatistics::default()),
        latest_access_slot: AtomicU64::new(0),
    })
}
```

All items are public in agave 4.5 (`ProgramCacheEntry` fields, `ProgramCacheEntryType::Loaded`, `RequisiteVerifier`). A unit test asserts this entry `==` the one `ProgramCacheEntry::load` produces (the `dev-context-only-utils` `PartialEq` compares owner, deployment slot and entry-type discriminant) — proving the uncompiled path is structurally identical modulo the compiled program.

### 3.3 Resident entry store (evm2 `ResidentMap`)

```rust
pub(crate) struct ProgramEntryStore {
    inner: RwLock<HashMap<ProgramEntryKey, Arc<ProgramCacheEntry>>>,
}
/// `(env_ptr, content_hash, loader_id, slot_bucket)`
pub(crate) struct ProgramEntryKey { env: usize, hash: [u8; 32], loader: Address, slot_bucket: u64 }
```

- Content hash over the ELF bytes (sha2, already a dependency) computed once per load attempt; a hit re-links the `Arc<ProgramCacheEntry>` — no ELF parse, no verify, no JIT.
- Eligibility widens the existing memo: `add_program`, `add_program_with_loader`, `add_program_from_file`, fixture loads, and `rebuild_program_cache` (which becomes "re-link resident entries for unchanged program accounts" instead of "reload all").
- The `include_bytes!` slice-pointer fast path remains for default programs (hashing is unnecessary when pointer identity is available and stable).
- Invalidation: a program account whose bytes change hashes to a different key; the old entry is unreachable through the store (no eviction scan needed, mirroring content addressing). Stats accumulated on a shared memoized entry are process-global, exactly like evm2's resident map — documented, accepted.
- The env-identity component is the same safety argument the existing memo already documents: only VMs sharing the exact same `Arc<ProgramRuntimeEnvironments>` may reuse entries.

### 3.4 Tier promoter (evm2 backend thread + worker pool)

```rust
pub(crate) struct TierPromoter {
    enabled: bool,                     // target x86_64 && policy == Lazy
    threshold: usize,
    pool: rayon::ThreadPool,           // clamp(cpus/2, 1, 4)
    stats: Arc<PromotionStats>,        // atomics: attempted/succeeded/failed/compile_us
}

impl TierPromoter {
    /// Post-transaction sweep: promote entries whose interpreted-invocation count
    /// crossed `threshold`. Reads only atomics under the cache read lock.
    pub(crate) fn observe(&self, cache: &SharedProgramCache);
    /// Prewarm path: promote an explicit set (fixture / batch planner).
    pub(crate) fn promote(&self, entry: Arc<ProgramCacheEntry>);
}
```

- Hotness signal: agave's own per-entry counters. After an interpreted run, agave records `stats.interpreter_executed(us)` for the entry ([program-runtime vm.rs](file:///Users/akagi201/.cargo/registry/src/rsproxy.cn-e3de039b2554c837/solana-program-runtime-4.5.0-alpha.2/src/vm.rs#L342-L346)) — so `entry.stats.interpreter_invocations` is a free, exact invocation counter. The sweep is O(entries) atomic loads per transaction.
- A job is `Arc<ProgramCacheEntry>` → `Loaded(exe) => exe.jit_compile()`. Promotion is safe concurrently with interpreted execution of the same entry (immutable text segments; publication under the internal mutex), and safe across batch workers (entries are shared by `Arc`).
- Double promotion is possible under a race; the second compile overwrites the first. Guard with `get_compiled_program().is_some()` pre-check; accept the rare wasted compile (evm2's resident-map presence check has the same tolerance).
- Saturation policy mirrors evm2: bounded queue, drop-and-count on overflow.
- `policy.effective()` makes the promoter a zero-sized no-op off x86_64 and under `Eager`/`Off`.

### 3.5 Program prewarm (evm2 AOT preload / `prepare_aot_batch`)

`Accounts::prewarm_programs(&[Address])` resolves (and, under `Lazy`, promotes) the program set for a batch stage or fixture before execution, on the rayon pool — the program-side twin of the account `prewarm` from parity AD-07. The batch staging loop feeds it the stage's program set, computed alongside the existing read-set. Error shape matches the account prewarm: attributed to the stage head transaction, fatal classification preserved.

### 3.6 Stats (evm2 `RuntimeStats`)

`ProgramTierStats` snapshot on `Svm`: resident entries, compilations, total compile time, interpreted vs compiled invocations, promotions attempted/succeeded/failed, dropped jobs, plus the active policy and target-arch reality. All from atomics; no locks on the hot path. Feeds the existing hotpath/tracing surface (`HPSVM_HOTPATH=1`, `register-tracing`).

### 3.7 Module layout

New module family under `crates/hpsvm/src/programs/` (the directory already exists):

```text
programs/
├── mod.rs            # existing default/SPL program loading
├── jit_policy.rs     # JitPolicy + effective() + builder plumbing
├── entry_store.rs    # ProgramEntryStore / ProgramEntryKey (generalizes runtime_env memo)
├── tiering.rs        # TierPromoter + PromotionStats + uncompiled load path
└── tier_stats.rs     # ProgramTierStats snapshot
```

Not a separate crate: the tiering logic is small and coupled to `Accounts`; the parity spec's crate split (macros/inspectors/fixture) is unaffected.

### 3.8 DX surface

```rust
Version::with_jit_policy(JitPolicy)                       // runtime override, like with_feature
Hpsvm::program_tier_stats() -> ProgramTierStats           // observability
Accounts::prewarm_programs(&[Address])                    // batch/fixture warm
```

## 4. Findings

### Finding 1: No compiled-program tiering policy — compilation is unconditional and uninstrumented

- **Category:** performance
- **Impact:** HIGH
- **Effort:** M
- **Risk:** MED — the uncompiled load path must match agave's structurally; promotion must never affect results.

#### Requirements (EARS Notation)

- **[REQ-01]:** `Version` SHALL carry a `JitPolicy` defaulting to `Eager`, overridable per VM via the builder or `with_jit_policy`.
- **[REQ-02]:** Under `Lazy`/`Off` on x86_64, program loading SHALL use the uncompiled load path; the resulting entry SHALL be structurally equal to agave's `ProgramCacheEntry::load` output modulo the compiled program.
- **[REQ-03]:** Under `Lazy`, a program whose per-entry interpreted-invocation count reaches `hot_threshold` SHALL be promoted on a background pool; invocations before publication SHALL run interpreted.
- **[REQ-04]:** Promotion failures SHALL leave the entry interpreted and SHALL NOT fail or alter any transaction.
- **[REQ-05]:** On non-x86_64 targets every policy SHALL behave identically (interpreted) and the promoter SHALL compile to nothing.
- **[REQ-06]:** With the default policy, observable behavior — results, CU accounting, program-cache contents — SHALL be byte-identical to the pre-spec implementation.

#### Current state

- [program_cache_entry.rs](file:///Users/akagi201/.cargo/registry/src/rsproxy.cn-e3de039b2554c837/solana-program-runtime-4.5.0-alpha.2/src/program_cache_entry.rs#L222-L232): `jit_compile()` unconditional under `target_arch = "x86_64"`.
- [accounts_db.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/accounts_db.rs#L713-L793) `load_program` → `ProgramCacheEntry::load`; no policy knob exists anywhere in `Version`.
- evm2 reference: [RuntimeTuning](file:///Volumes/akext/tmp/evm2/crates/jit/runtime/src/runtime/config.rs#L232-L288), [`JitBackend::lookup`](file:///Volumes/akext/tmp/evm2/crates/jit/runtime/src/runtime/mod.rs#L217-L249).

#### Approach

Add `JitPolicy` (§3.1), the uncompiled load path (§3.2), and the `TierPromoter` (§3.4). The promotion observation point is a post-transaction sweep in `Svm` — O(entries) atomic reads, off the agave path entirely. `Off` = `Lazy { hot_threshold: usize::MAX }` semantically; kept as a separate variant for an explicit, self-documenting API.

#### Architecture Decisions (MADR Format)

- **AD-01:** agave/sbpf remains the only SBF compiler. Rationale: mainnet fidelity (parity AD-01). Consequence: this spec drives agave's existing tiering seam; it never emits code.
- **AD-02:** The policy lives on `Version` as data, not as a cargo feature. Rationale: the searcher toggles it per workload (simulate-heavy vs replay-heavy) without a rebuild; compile-time selection cannot express "interpret this batch, compile that one".
- **AD-03:** Promotion mutates the `Executable` (interior mutability) rather than replacing cache entries. Rationale: no cache-map write lock, no worker-visible entry swap, batch-stage snapshots keep working; agave's `PreferJit` picks up the compiled program on the next invocation with zero coordination.
- **AD-04:** The hotness signal is agave's `ProgramStatistics.interpreter_invocations`, not an hpsvm-side counter. Rationale: it counts *program invocations* (including CPI) for free; an hpsvm-side counter would only see top-level instructions and would drift under CPI.
- **AD-05:** `Eager` remains the default. Rationale: on x86_64 that is today's behavior and mainnet parity; `Lazy` is opt-in for simulation-heavy workloads and must earn its default through the benchmarks.

### Finding 2: Program reloads and cache rebuilds recompile everything; no content-addressed resident store

- **Category:** performance
- **Impact:** HIGH
- **Effort:** M
- **Risk:** MED — memoization must never serve an entry across differing environments or program bytes.

#### Requirements (EARS Notation)

- **[REQ-01]:** A process-wide `ProgramEntryStore` SHALL key entries by `(environment identity, program content hash, loader, slot bucket)`.
- **[REQ-02]:** Loading a program whose entry is resident SHALL skip ELF parsing, verification and JIT compilation entirely.
- **[REQ-03]:** `rebuild_program_cache` SHALL reuse resident entries for program accounts whose bytes are unchanged instead of reloading them.
- **[REQ-04]:** A program account whose bytes differ from the resident entry's hash SHALL resolve to a fresh load (content addressing; no explicit eviction).
- **[REQ-05]:** The existing default-program memo (slice-pointer identity) SHALL keep working; the content-keyed store is an extension, not a replacement.

#### Current state

- [runtime_env.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/runtime_env.rs#L173-L210): memo restricted to `CACHED` callers with `include_bytes!` statics under the default environment.
- [accounts_db.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/accounts_db.rs#L640-L675): rebuild reloads every program account.
- [executed.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/executed.rs#L236-L245): discard on a cache-dirty transaction triggers a full rebuild.
- evm2 reference: `ResidentMap` + [`ArtifactStore` keyed by full identity](file:///Volumes/akext/tmp/evm2/crates/jit/runtime/src/runtime/storage.rs#L9-L31).

#### Approach

Generalize the memo (§3.3). Hash cost is paid once per load attempt and is orders of magnitude below ELF verification. Slot bucket distinguishes entries by the deployment-slot constant family (all current loads use `IMMEDIATE_DEPLOYMENT_SLOT`; the field exists to keep the key honest if that ever changes).

#### Architecture Decisions (MADR Format)

- **AD-06:** The store is in-process only; no disk artifacts. Rationale: sbpf exposes no stable artifact format (blocked §2.1); within a searcher process the per-block VM lifecycle is exactly the resident-map lifecycle.
- **AD-07:** Entries are keyed by environment `Arc` pointer identity plus content hash, not by environment contents. Rationale: matching the existing memo's documented safety argument; contents-equivalent environments are distinct objects by construction (custom syscalls / feature sets build fresh environments).

### Finding 3: No background compilation or program-prewarm stage

- **Category:** performance
- **Impact:** MEDIUM
- **Effort:** M
- **Risk:** LOW — prewarming only front-loads work already done serially.

#### Requirements (EARS Notation)

- **[REQ-01]:** `Accounts` SHALL expose `prewarm_programs(&[Address])` resolving program entries off the transaction path.
- **[REQ-02]:** The batch staging loop SHALL prewarm each stage's program set before spawning stage workers.
- **[REQ-03]:** Under `Lazy`, prewarm SHALL additionally schedule promotion for the prewarmed set.
- **[REQ-04]:** Prewarm source failures SHALL be attributed to the stage head transaction with the fatal classification preserved, mirroring account prewarm.

#### Current state

- [batch.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/batch.rs#L170-L196): account prewarm exists per stage; nothing program-side.
- evm2 reference: [worker pool](file:///Volumes/akext/tmp/evm2/crates/jit/runtime/src/runtime/worker.rs#L1-L50), [`prepare_aot_batch`](file:///Volumes/akext/tmp/evm2/crates/jit/runtime/src/runtime/mod.rs#L364-L407).

#### Approach

§3.5. Prewarm runs on the existing rayon pool (promoter pool when active, otherwise a short-lived scoped pool) so a stage's first transaction never pays the program-set load serially.

### Finding 4: Compiled-tier execution is invisible

- **Category:** DX / operability
- **Impact:** MEDIUM
- **Effort:** S
- **Risk:** LOW.

#### Requirements (EARS Notation)

- **[REQ-01]:** `Svm` SHALL expose a `program_tier_stats()` snapshot reporting policy, resident entries, compilations, compile time, interpreted/compiled invocations, promotion outcomes, and dropped jobs.
- **[REQ-02]:** Stats SHALL be assembled from atomics only; no locks on the transaction path.
- **[REQ-03]:** Off x86_64 the snapshot SHALL report the effective policy as `Off` with zero compile/promotion counters.

#### Current state

- agave's per-entry `ProgramStatistics` exists ([program_metrics.rs](file:///Users/akagi201/.cargo/registry/src/rsproxy.cn-e3de039b2554c837/solana-program-runtime-4.5.0-alpha.2/src/program_metrics.rs#L20-L90)) but `hpsvm` reads none of it; the existing hotpath blocks cover only `process_transaction` phases ([execution.rs](file:///Volumes/akext/src/github.com/longcipher/hpsvm/crates/hpsvm/src/execution.rs#L227-L256)).
- evm2 reference: [`RuntimeStats`/`CompilationEvent`](file:///Volumes/akext/tmp/evm2/crates/jit/runtime/src/runtime/config.rs#L135-L150).

### Finding 5: No interpreted/compiled equivalence gate

- **Category:** correctness
- **Impact:** HIGH
- **Effort:** M
- **Risk:** LOW (the gate itself is read-only).

#### Requirements (EARS Notation)

- **[REQ-01]:** The fixture compare harness SHALL accept an execution-policy axis, including a forced-interpreted mode on x86_64.
- **[REQ-02]:** Running the corpus under forced-interpreted and under `Eager` SHALL produce identical post-state, logs, return data, and status for every fixture.
- **[REQ-03]:** CU-count differences attributable to sbpf's JIT meter-checkpoint granularity SHALL be surfaced as a report line, not silently ignored; state/result identity is the hard gate.
- **[REQ-04]:** The gate SHALL skip (recording why) on non-x86_64, following the existing SBF-toolchain skip convention.

#### Current state

- `crates/hpsvm-fixture` compare/capture/check pipeline exists; no mode axis.
- evm2 reference: [eest/src/jit.rs](file:///Volumes/akext/tmp/evm2/crates/eest/src/jit.rs#L7-L30) — the corpus runs under JIT as a first-class mode.

#### Approach

`JitPolicy::Off` (Finding 1) doubles as the forced-interpreted switch, so the gate costs one harness parameter. REQ-03 acknowledges a real sbpf property: the JIT meters in basic-block chunks (`instruction_meter_checkpoint_distance`), so pathological programs can differ by a bounded amount; since mainnet validators run the JIT, `Eager` is the fidelity reference and `Off` is the diagnostic arm.

### Finding 6: Program-identity and caching logic has no single boundary

- **Category:** architecture
- **Impact:** MEDIUM
- **Effort:** S
- **Risk:** LOW.

#### Requirements (EARS Notation)

- **[REQ-01]:** All program-identity concerns (environment sharing, memoization, policy selection, promotion, stats) SHALL live in the `programs` module family.
- **[REQ-02]:** `runtime_env.rs` SHALL retain environment construction only; the default-program memo SHALL move into the entry store.
- **[REQ-03]:** No module outside `programs::*` and `accounts_db` SHALL call `ProgramCacheEntry::load` directly.

#### Current state

- The memo lives in `runtime_env.rs`, loads in `accounts_db.rs::load_program`, and resolution in `vm.rs::add_program_internal` — three call sites, one concern.

## 5. Non-goals

- No SBF compiler, IR, or backend work (AD-01; §2.1).
- No persistent/on-disk compiled artifacts; no out-of-process helper (AD-06; §2.1).
- No change to default behavior: `Eager` on x86_64 reproduces today's load path exactly (F1/REQ-06).
- No JIT on non-x86_64 targets — sbpf does not support them (§2.3).
- No changes to execution semantics, transaction staging, or the public `Hpsvm` surface beyond the additive policy/stats/prewarm APIs.

## 6. Test Strategy

- **Primary Language:** Rust 2024.
- **Unit tests:** colocated (`#[cfg(test)]`); `JitPolicy::effective` truth table across targets; uncompiled-load structural equality against `ProgramCacheEntry::load` (via the `dev-context-only-utils` `PartialEq`); entry-store key determinism (same bytes → same key; one differing byte → different key); eligibility rules (env identity mismatch must not serve an entry).
- **Promotion tests:** threshold crossing (no promotion below, promotion at/after); promotion failure leaves the entry interpreted and the transaction successful; a counting/forcing harness asserts the entry is interpreted before publication and observed compiled after (on x86_64; skip elsewhere).
- **Property tests:** `proptest` invariants — store lookup is deterministic for fixed (env, bytes, loader, slot bucket); repeated rebuild after N mutations re-links rather than reloads (counting source with a load counter); promotion is idempotent under repeated observation.
- **Behavioral equivalence:** the fixture compare harness under `Off` vs `Eager` (F5) is the acceptance gate for Findings 1–3; `just bench-runtime-baseline-compare` guards the default path.
- **Mutation testing:** threshold comparison, policy `effective()` mapping, store-key construction and the eligibility check must kill mutants; tighten tests rather than excluding.
- **Platform convention:** JIT-specific assertions skip on non-x86_64 with a recorded reason, matching the existing SBF-toolchain skip pattern.

## 7. Benchmark and performance gates

- `just bench-runtime` and `just bench-hotpath` MUST NOT regress under the default `Eager` policy (no behavior change by construction).
- New criterion bench `program_cache` in `crates/hpsvm/benches`: cold vs warm `add_program` for a program set, `rebuild_program_cache` after an account mutation, and `prewarm_programs` amortization for a multi-stage batch. Baselines recorded via the existing `bench-runtime-baseline-refresh` flow.
- `JitPolicy::Lazy` workloads MUST show: (a) compile cost moved off the transaction path (hotpath block on `resolve_program_entry` drops), (b) no result divergence in the fixture gate. Throughput win on simulation-heavy loops is expected but measured, not assumed.
- Stats snapshots are part of the acceptance evidence: every benchmark run records the tier stats next to the criterion numbers.

## 8. Landing order and risk

Phase 0 (boundary + policy surface, no behavior change) → Phase 1 (uncompiled load + `Off` + initial equivalence gate) → Phase 2 (resident entry store) → Phase 3 (promoter + `Lazy`) → Phase 4 (prewarm + batch integration) → Phase 5 (stats surface) → Phase 6 (bench + docs + full gate sweep). Each phase keeps the suite green and is independently revertible; the gate from Finding 5 lands before the first semantics-affecting phase.

Residual risks and mitigations:

- **Memoization serving a mismatched entry** — mitigated by env-pointer identity + content hash + unit tests asserting cross-env misses and one-byte key changes; property test on determinism.
- **Manual entry construction drifting from agave's `load`** — mitigated by the structural-equality unit test and by pinning the agave version in the workspace; the path is cfg'd to x86_64 + non-Eager only, so the default never touches it.
- **Promotion racing interpreted execution** — mitigated by interior-mutability design (`jit_compile(&self)`, immutable text segments) and by the double-promotion tolerance check; proptest idempotence.
- **CU-count divergence between modes** — surfaced by REQ-03 reporting; `Eager` is the fidelity reference.
- **Pool starvation under saturation** — drop-and-count semantics inherited from evm2; saturation is observable in stats.

## 9. Evidence index

| Ref | Location |
| :--- | :--- |
| E1 | `crates/jit/runtime/src/runtime/mod.rs` — `JitBackend`, `lookup`, `preload_aot` |
| E2 | `crates/jit/runtime/src/runtime/config.rs` — `RuntimeConfig`, `RuntimeTuning`, `should_compile` |
| E3 | `crates/jit/runtime/src/runtime/storage.rs` — `ArtifactKey`, `ArtifactStore`, `RuntimeArtifactStore` |
| E4 | `crates/jit/runtime/src/runtime/worker.rs` — compile worker pool |
| E5 | `crates/jit/backend/src/traits.rs` — `Backend`, `BackendConfig`, `OptimizationLevel` |
| E6 | `crates/jit/codegen/src/compiler/mod.rs`, `bytecode/mod.rs` — compiler driver, bytecode analysis |
| E7 | `crates/eest/src/jit.rs` — EEST JIT backend (conformance mode) |
| E8 | `crates/jit/AGENTS.md`, `scripts/jit/bench.py` — bench/diff harness conventions |
| E9 | solana-sbpf 0.25.0 `src/vm.rs` — `ExecutionMode`, `execute_program` PreferJit arm |
| E10 | solana-sbpf 0.25.0 `src/elf.rs` — `get_compiled_program`, `jit_compile` |
| E11 | solana-program-runtime 4.5.0-alpha.2 `src/program_cache_entry.rs` — `ProgramCacheEntry`, unconditional `jit_compile` on x86_64 |
| E12 | solana-program-runtime 4.5.0-alpha.2 `src/vm.rs` — hardcoded `ExecutionMode::PreferJit` |
| E13 | solana-program-runtime 4.5.0-alpha.2 `Cargo.toml` — `solana-sbpf` with `features = ["jit"]` |
| E14 | `crates/hpsvm/src/runtime_env.rs` — `DEFAULT_PROGRAM_CACHE`, `resolve_program_entry` |
| E15 | `crates/hpsvm/src/accounts_db.rs` — `load_program`, `rebuild_program_cache`, `SharedProgramCache` |
| E16 | `crates/hpsvm/src/vm.rs` — `add_program_internal`, `add_program_preverified` |
| E17 | `crates/hpsvm/src/executed.rs`, `batch.rs`, `execution.rs` — discard/rebuild triggers, worker cache snapshots, per-phase hotpath blocks |
| E18 | `specs/2026-10-15-01-evm2-parity/design.md` — blocked set (§2.1), AD-01/AD-02, AD-07 prewarm precedent |
