# Tasks: evm2 JIT parity — compiled-program tiering, resident entry store, and observability

Planned at commit `a884b47` (2026-10-16).

Companion document: [design.md](./design.md). Findings are referenced as F1–F6.

## Phase 0: Module boundary and policy surface (F6, F1/REQ-01)

### Task 0.1: Create the `programs` module family and move the default-program memo

> **Context:** F6/REQ-01–03. One boundary for program identity before any tiering code exists, so later phases add behavior to a settled location instead of scattering it.
> **Verification:** `cargo build --workspace` succeeds; `crates/hpsvm/src/programs/{jit_policy,entry_store,tiering,tier_stats}.rs` exist with the memo moved out of `runtime_env.rs`; no call site outside `programs::*` and `accounts_db` names `ProgramCacheEntry::load`.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `Default-program executables are memoized exactly as before the move (same cache-hit behavior for repeated default-VM construction)`
- **Simplification Focus:** `Move code; do not change the key, the eligibility, or the load path`
- **Status:** 🟢 TODO
- [ ] Step 1: Write a test asserting two VMs constructed with the default configuration share one default-program executable (Arc pointer identity), matching the pre-move behavior.
- [ ] Step 2: RED — assert fails or does not compile against the old location.
- [ ] Step 3: Create `programs/jit_policy.rs`, `programs/entry_store.rs`, `programs/tiering.rs`, `programs/tier_stats.rs`; move `DEFAULT_PROGRAM_CACHE` / `resolve_program_entry` eligibility into `entry_store.rs`.
- [ ] Step 4: GREEN — run the test and the full default-VM suite.
- [ ] Step 5: Grep-audit `ProgramCacheEntry::load` call sites; none outside `programs::*` and `accounts_db::load_program`.
- [ ] Advanced Test Verification: `cargo test --all-features` — all pass
- [ ] Runtime Verification: `just bench-runtime` — no regression

### Task 0.2: Add `JitPolicy` to `Version` with builder plumbing

> **Context:** F1/REQ-01, AD-02. Policy as runtime data on `Version`, defaulting to `Eager` so behavior is unchanged.
> **Verification:** `JitPolicy::default() == Eager`; `Version::with_jit_policy` overrides; `JitPolicy::effective()` maps every variant to `Off` on non-x86_64; a size/shape test asserts the default policy is carried on the version constructed by every existing entry point.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `Every existing VM constructor produces a Version whose policy is Eager, and with_jit_policy round-trips through the builder`
- **Simplification Focus:** `One enum, one field, one override; no load-path change in this task`
- **Status:** 🟢 TODO
- [ ] Step 1: Write a unit test for the `JitPolicy` truth table (`effective()` per variant; `requires_uncompiled_load()` per variant) and a builder round-trip test.
- [ ] Step 2: RED.
- [ ] Step 3: Add `JitPolicy` per design §3.1; add the field to `Version`; add `with_jit_policy` and builder plumbing.
- [ ] Step 4: GREEN.
- [ ] Step 5: Assert every public constructor (`Hpsvm::new`, builder paths) yields `Eager`.
- [ ] Advanced Test Verification: `cargo test --all-features` — all pass
- [ ] Runtime Verification: `just bench-runtime` — no regression (policy is inert in this task)

## Phase 1: Uncompiled load path and forced-interpreted mode (F1/REQ-02, F5 entry)

### Task 1.1: Implement `load_program_uncompiled` and prove structural equality

> **Context:** F1/REQ-02. agave's `load` unconditionally JIT-compiles on x86_64; the uncompiled path is the only way to obtain the interpreter-first tier, and it must be structurally identical.
> **Verification:** A unit test constructs the entry via both paths and asserts equality (the `dev-context-only-utils` `PartialEq` compares owner, deployment slot, and entry-type discriminant); the uncompiled entry reports `get_compiled_program() == None` on x86_64.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `The uncompiled entry equals agave's loaded entry modulo the compiled program, and an uncompiled entry executes a program successfully (interpreted) on x86_64`
- **Simplification Focus:** `Mirror agave's load body verbatim, minus the jit_compile call`
- **Status:** 🟢 TODO
- [ ] Step 1: Write the structural-equality test and an execution test (load uncompiled → run a transaction → assert success and state effect) gated to x86_64, skipping elsewhere.
- [ ] Step 2: RED.
- [ ] Step 3: Implement `load_program_uncompiled` in `programs/tiering.rs` per design §3.2 using `Executable::load` + `verify::<RequisiteVerifier>()` + manual `ProgramCacheEntry` construction.
- [ ] Step 4: Route `Accounts::load_program` through the policy: uncompiled path iff `target_arch = "x86_64" && policy.requires_uncompiled_load()`.
- [ ] Step 5: GREEN.
- [ ] Advanced Test Verification: `cargo test --all-features` — all pass (non-x86_64 skips recorded)
- [ ] Runtime Verification: fixture harness — identical results with policy `Off` vs `Eager` on x86_64

### Task 1.2: Wire `JitPolicy::Off` through the fixture harness as the forced-interpreted mode

> **Context:** F5/REQ-01, REQ-04. The equivalence gate must exist before the store and promoter land, so semantics-affecting phases always have a reference oracle.
> **Verification:** `cargo run -p hpsvm-fixture -- compare --policy off` (or the harness's equivalent flag) runs the corpus and writes results; a scripted run under `off` and `eager` diffs clean.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `Every fixture in the corpus produces identical status, post-state, logs, and return data under Off and Eager`
- **Simplification Focus:** `One parameter on the existing harness; reuse the existing compare pipeline`
- **Status:** 🟢 TODO
- [ ] Step 1: Extend the fixture harness config/CLI with a policy axis; write a test running one fixture under both policies and asserting equality.
- [ ] Step 2: RED.
- [ ] Step 3: Thread the policy into VM construction in the harness runner.
- [ ] Step 4: GREEN; run the full corpus under both policies on x86_64; record the CU-difference report line required by REQ-03.
- [ ] Advanced Test Verification: `cargo test --all-features` — all pass
- [ ] Runtime Verification: fixture compare — identical results across the corpus (x86_64 host)

## Phase 2: Resident entry store (F2)

### Task 2.1: Implement the content-keyed `ProgramEntryStore`

> **Context:** F2/REQ-01–05. Content addressing generalizes the existing memo so `add_program`, `add_program_from_file`, fixture loads and rebuilds all share resident entries.
> **Verification:** A counting source shows zero ELF loads for a repeated `add_program` of identical bytes under the same environment; a one-byte-different ELF misses; an entry is not served across different environment identities.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `Resident entries skip ELF parse, verification and JIT compilation; content-addressed keys are deterministic and environment-scoped`
- **Simplification Focus:** `Generalize the existing memo key; keep the slice-pointer fast path`
- **Status:** 🟢 TODO
- [ ] Step 1: Write tests: repeated `add_program` hits (counting `LoadProgramMetrics`/entry constructions via a test hook), one-byte mutation misses, cross-environment miss, program-account mutation (upgrade) resolves a fresh entry.
- [ ] Step 2: RED.
- [ ] Step 3: Implement `ProgramEntryStore` / `ProgramEntryKey` per design §3.3 (sha256 content hash, env pointer identity, loader, slot bucket) with the `include_bytes!` fast path retained.
- [ ] Step 4: Route `resolve_program_entry` and `add_program_internal` through the store.
- [ ] Step 5: GREEN.
- [ ] Step 6: Add a `proptest` property: for arbitrary (env, bytes, loader, bucket) tuples the key is deterministic and differing bytes never collide.
- [ ] Advanced Test Verification: `cargo test --all-features` — all pass
- [ ] Runtime Verification: `just bench-runtime` — no regression; the new `program_cache` bench records cold-vs-warm `add_program`

### Task 2.2: Make `rebuild_program_cache` re-link resident entries

> **Context:** F2/REQ-03. The discard/reconfigure path must stop recompiling the untouched program set.
> **Verification:** After a cache-dirty discard, a counting hook shows reloads only for programs whose bytes changed; results identical to the pre-change rebuild.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `rebuild_program_cache reuses resident entries for unchanged program accounts and produces a functionally identical cache`
- **Simplification Focus:** `Per-account store lookup inside the existing loop`
- **Status:** 🟢 TODO
- [ ] Step 1: Write a test that deploys/upgrades one program among several, discards, and asserts only the changed program was (re)loaded.
- [ ] Step 2: RED.
- [ ] Step 3: Replace the unconditional `load_program` call in the rebuild loop with a store-resolve that falls back to load on miss.
- [ ] Step 4: GREEN; assert fixture equivalence on the discard-heavy fixtures.
- [ ] Advanced Test Verification: `cargo test --all-features` — all pass
- [ ] Runtime Verification: fixture compare — identical results; `rebuild_program_cache` hotpath block shows the drop

## Phase 3: Tier promoter (F1/REQ-03–05)

### Task 3.1: Implement `TierPromoter` with hot-threshold observation

> **Context:** F1/REQ-03–05, AD-03/AD-04/AD-05. The interpreter-first tier, driven by agave's own per-entry invocation counters and promoted via `Executable::jit_compile` interior mutability.
> **Verification:** Below threshold: never compiled. At threshold: exactly one promotion enqueued; after publication, the next invocation runs compiled (asserted via entry stats / hotpath evidence on x86_64). A forced `jit_compile` failure leaves the entry interpreted and the transaction successful. Off x86_64 the promoter compiles to nothing and every policy behaves identically.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `A program crosses from interpreted to compiled exactly when its interpreted-invocation count reaches hot_threshold, and promotion never affects transaction results`
- **Simplification Focus:** `One sweep, one job type, one bounded pool; drop-and-count on saturation`
- **Status:** 🟢 TODO
- [ ] Step 1: Write tests: threshold boundary (threshold-1, threshold), promotion-failure injection (test hook forcing `jit_compile` error), idempotent repeated sweeps, non-x86_64 skip.
- [ ] Step 2: RED.
- [ ] Step 3: Implement `TierPromoter` per design §3.4: bounded rayon pool (`clamp(cpus/2, 1, 4)`), post-transaction `observe` sweep reading `entry.stats.interpreter_invocations`, `get_compiled_program()` pre-check, atomic `PromotionStats`.
- [ ] Step 4: Wire the sweep into the transaction completion path (after commit/discard staging resolves) and enable it only under `Lazy` on x86_64.
- [ ] Step 5: GREEN.
- [ ] Step 6: Add a `proptest` property: promotion observation is idempotent and never promotes under `Eager`/`Off`.
- [ ] Advanced Test Verification: `cargo test --all-features` — all pass (non-x86_64 skips recorded)
- [ ] Runtime Verification: fixture compare under `Lazy { hot_threshold: 0 }` and a mid threshold — identical results to `Eager`/`Off`

## Phase 4: Program prewarm (F3)

### Task 4.1: Add `Accounts::prewarm_programs` and wire the batch staging loop

> **Context:** F3/REQ-01–04. The program-side twin of the account prewarm (parity AD-07); the batch planner already computes each stage's read-set and can name its program set.
> **Verification:** A batch stage performs one batched program resolve instead of per-transaction loads; under `Lazy`, prewarmed programs are promoted; source failures attribute to the stage head with the fatal classification preserved.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `A stage's programs are resolved (and promoted under Lazy) before its first transaction executes, with identical batch results`
- **Simplification Focus:** `Reuse the account prewarm's error shape and the promoter's pool`
- **Status:** 🟢 TODO
- [ ] Step 1: Write a counting test: one batched resolve per stage; a `Lazy` test asserting promotion of the stage's program set.
- [ ] Step 2: RED.
- [ ] Step 3: Implement `prewarm_programs(&[Address])` per design §3.5; compute each stage's program set in the staging loop and call it before spawning workers.
- [ ] Step 4: GREEN; assert batch results identical to serial execution and conflict-freedom tests still pass.
- [ ] Advanced Test Verification: `cargo test --all-features` — all pass
- [ ] Runtime Verification: `just bench-runtime` — no regression; multi-stage batch bench records the amortization

## Phase 5: Stats surface (F4)

### Task 5.1: Expose `ProgramTierStats`

> **Context:** F4/REQ-01–03. Without the snapshot, none of the above can be evaluated or operated.
> **Verification:** A test drives a `Lazy` VM through a threshold crossing and asserts the snapshot reports attempts/successes and interpreted/compiled invocations; on non-x86_64 the snapshot reports `Off` with zero counters.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `program_tier_stats() reports policy, resident entries, compilations, compile time, invocation split, promotion outcomes, and dropped jobs from atomics only`
- **Simplification Focus:** `Read agave's ProgramStatistics and hpsvm's PromotionStats; add no new hot-path work`
- **Status:** 🟢 TODO
- [ ] Step 1: Write the threshold-crossing stats test and the non-x86_64 zero-counter test.
- [ ] Step 2: RED.
- [ ] Step 3: Implement `ProgramTierStats` and `Svm::program_tier_stats()` per design §3.6; add hotpath blocks around resolve/rebuild/promotion.
- [ ] Step 4: GREEN; document the snapshot in the crate docs.
- [ ] Advanced Test Verification: `cargo test --all-features` — all pass
- [ ] Runtime Verification: `just bench-hotpath` — no regression; bench outputs record the tier stats

## Phase 6: Bench, docs, and gate sweep (design §7)

### Task 6.1: Add the `program_cache` criterion bench and record baselines

> **Context:** Design §7 — the hot path is explicit (program load/rebuild/prewarm), so Criterion is justified per AGENTS.md.
> **Verification:** `cargo bench -p hpsvm --bench program_cache` runs on x86_64 (skips elsewhere); baselines recorded into `docs/benchmarks/runtime-baselines` via the existing refresh flow.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `The bench measures cold vs warm add_program, rebuild after mutation, and prewarm amortization, and the baseline compare detects regressions`
- **Simplification Focus:** `One bench file, three measurements, existing export/compare scripts`
- **Status:** 🟢 TODO
- [ ] Step 1: Write the bench (cold/warm `add_program`, `rebuild_program_cache` post-mutation, multi-stage prewarm) with SBF-program availability skip guard.
- [ ] Step 2: RED (bench unrunnable without the harness).
- [ ] Step 3: Implement; run and record baselines on the x86_64 host.
- [ ] Step 4: `just bench-runtime-baseline-compare` clean for the default path.
- [ ] Advanced Test Verification: `cargo test --all-features` — all pass
- [ ] Runtime Verification: `just bench-runtime`, `just bench-hotpath`, `just bench-runtime-baseline-compare` — no regressions

### Task 6.2: Documentation and full gate sweep

> **Context:** The policy surface is public API; docs and the full gate set close the spec.
> **Verification:** `cargo doc --workspace` warning-free; every new public item documented; all gates exit 0.

- **Loop Type:** `TDD`
- **Behavioral Contract:** `The crate docs demonstrate the policy surface end-to-end and the workspace passes format, lint, test, mutants-quick, and the fixture equivalence gate`
- **Simplification Focus:** `Replace stale docs; do not extend scope`
- **Status:** 🟢 TODO
- [ ] Step 1: Add a doctest exercising `with_jit_policy` + `program_tier_stats()` (x86_64-agnostic assertions).
- [ ] Step 2: RED.
- [ ] Step 3: Rewrite the program-caching docs section around policy, store, promoter, prewarm, stats.
- [ ] Step 4: GREEN.
- [ ] Step 5: Run `just format`, `just lint`, `just test-all`, `just mutants-quick` (tighten tests for surviving mutants in threshold, `effective()`, key construction, eligibility), and the fixture compare gate under `off`/`eager`/`lazy`.
- [ ] Advanced Test Verification: `cargo test --doc --all-features` — all pass
- [ ] Runtime Verification: `just ci` — exit 0

## Dependency order

Phase 0 → 1 → 2 → 3 → 4 → 5 → 6.

Phase 2 and Phase 3 are independent of each other after Phase 1 and may land in either order. Phase 1 (the equivalence gate) deliberately precedes the first semantics-affecting phase so every later phase can be validated against a reference oracle. Phase 6 lands last.
