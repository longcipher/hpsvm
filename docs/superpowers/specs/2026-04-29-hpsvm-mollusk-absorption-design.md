# HPSVM Mollusk Capability Absorption Design

**Status:** Superseded by product-layer consolidation
**Date:** 2026-04-29
**Scope:** Crate-level design only. No implementation is proposed in this document.

**Implementation update, 2026-05-02:** The product layer now consolidates the originally separate `hpsvm-result`, `hpsvm-fixture`, and `hpsvm-bencher` proposals into one `crates/hpsvm-fixture` crate. The Firedancer adapter remains separate as `crates/hpsvm-fixture-fd`, and the CLI lives under `bin/hpsvm-cli`.

## Goal

Absorb the parts of Mollusk that improve the product layer around `hpsvm` without replacing `hpsvm`'s transaction-first execution core.

This design keeps `hpsvm` as the execution engine for transaction-faithful tests and adds product-layer surfaces for:

- result assertion and comparison inside `hpsvm-fixture`
- fixture capture, storage, and replay inside `hpsvm-fixture`
- command-line replay and A/B comparison
- compute-unit regression reporting inside `hpsvm-fixture`
- external fixture format compatibility through `hpsvm-fixture-fd`

## Problem Statement

`hpsvm` already has stronger runtime-facing capabilities than Mollusk in several areas:

- explicit `transact -> commit_transaction` semantics
- conflict-aware batch planning and execution
- read-through account sources
- top-level inspectors
- register tracing and trace metrics

The missing layer is not the VM. The missing layer is the testing product surface:

- no reusable assertion DSL over execution results
- no stable fixture schema for replay and regression testing
- no CLI for fixture execution or candidate-vs-baseline comparison
- no developer-facing CU regression crate separate from Criterion and hotpath analysis
- no dedicated crate boundaries for these concerns

## Non-Goals

- rewriting `hpsvm` into a Mollusk-style minified instruction harness
- replacing existing Criterion or hotpath benchmarks
- feature-gating core public fields or methods in ways that fragment the API
- baking Firedancer compatibility into the first release
- introducing a proto-first schema as the only supported fixture format in v1

## Design Principles

1. Keep `hpsvm` transaction-first.
2. Add product-layer crates around `hpsvm`; do not fold everything into the core crate.
3. Prefer additive feature flags and separate crates over complex feature matrices.
4. Keep fixture and assertion data models serializable and stable.
5. Treat the CLI as a thin integration layer, not as the location of core logic.
6. Make instruction-level helpers convenience APIs, not the primary semantic model.

## Approaches Considered

### Approach A: Expand `hpsvm` Monolithically

Put result assertions, fixtures, CLI-facing helpers, and CU reporting directly into `crates/hpsvm` as modules and features.

**Pros**

- lowest package count
- easiest initial discovery for users
- no cross-crate conversion layer

**Cons**

- mixes runtime core with product-layer concerns
- grows `hpsvm`'s public API and dependency surface too quickly
- forces optional serialization, config parsing, and report generation concerns into the core crate
- makes long-term semver harder because everything becomes part of the main crate surface

### Approach B: Add Companion Crates Around `hpsvm` (Recommended)

Keep `hpsvm` as the execution engine and add small companion crates for result checking, fixtures, CLI integration, and CU reporting.

**Pros**

- clean boundaries with one reason to change per crate
- optional dependencies stay out of `hpsvm`
- package surfaces map directly to user jobs
- easier phased rollout and crate-by-crate publishing

**Cons**

- more workspace members
- requires a stable conversion boundary from `hpsvm` runtime outputs into product-layer snapshots

### Approach C: Recreate Mollusk's Split Exactly

Mirror Mollusk's `harness`, `result`, `fuzz/*`, `cli`, and `bencher` layout and shift `hpsvm` toward an instruction-harness-first architecture.

**Pros**

- easy mental mapping for users migrating from Mollusk
- direct parity with Mollusk documentation and examples

**Cons**

- fights `hpsvm`'s current semantics
- over-optimizes for feature parity instead of leverage
- risks duplicating runtime abstractions already solved differently in `hpsvm`

## Recommendation

Adopt Approach B.

The core idea is:

- keep `crates/hpsvm` as the execution engine
- add `crates/hpsvm-fixture` as the stable assertion, comparison, fixture, replay, and compute-unit reporting layer
- add `bin/hpsvm-cli` as the operator-facing replay and comparison tool
- keep `crates/hpsvm-fixture-fd` as the external-format compatibility adapter

## Proposed Workspace Shape

### Existing crates retained

- `hpsvm`
- `hpsvm-fork-rpc`
- `hpsvm-loader`
- `hpsvm-token`

### New crates

| Package | Kind | Purpose | Depends on |
| --- | --- | --- | --- |
| `hpsvm-fixture` | library | snapshot model, assertion DSL, comparison DSL, fixture schema, codecs, capture, replay, and CU reporting | `hpsvm` |
| `hpsvm-cli` | binary + internal lib modules | fixture execution, A/B compare, CU report generation | `hpsvm`, `hpsvm-fixture`, optionally `hpsvm-fixture-fd` |
| `hpsvm-fixture-fd` | library | import and export Firedancer-compatible fixtures | `hpsvm-fixture` |

### Dependency graph

```text
hpsvm-fixture -> hpsvm
hpsvm-cli -> hpsvm-fixture, hpsvm
hpsvm-fixture-fd -> hpsvm-fixture
```

Rules:

- `hpsvm` must not depend on any new companion crate.
- `hpsvm-fixture` owns the product-layer model so snapshot, comparison, fixture replay, and CU reporting evolve together.
- `hpsvm-cli` owns config-file parsing and command ergonomics, not validation logic.

## Core Crate Changes in `hpsvm`

Phase 1 should keep changes in `hpsvm` narrow.

### Required changes

- no new core crate split is required
- keep current `TransactionMetadata`, `ExecutionOutcome`, and `SimulatedTransactionInfo` as the source runtime outputs
- add any missing public accessors only when a companion crate cannot derive its snapshot without reaching into internals

### Recommended additions

- add a small `instruction` convenience module in `hpsvm` later, not in phase 1
- keep it always-on rather than feature-gated if it lands, because it is a user-facing convenience API with low dependency cost

### Explicit non-change

`hpsvm` should not embed fixture codecs, YAML parsing, markdown reporting, or CLI concerns.

## Crate Design: `hpsvm-result`

### Purpose

Provide a stable, serializable, product-facing view over `hpsvm` execution outputs plus a reusable DSL for checks and comparisons.

This crate absorbs the parts of Mollusk's `result` crate that are clearly missing today.

### Package name

- package: `hpsvm-result`
- crate: `hpsvm_result`

### Public API

```rust
pub use crate::{
    check::{AccountExpectation, AccountExpectationBuilder, Check},
    compare::{AccountCompareScope, Compare},
    config::ResultConfig,
    snapshot::{
        AccountSnapshot,
        ExecutionSnapshot,
        ExecutionStatus,
        InnerInstructionSnapshot,
        ReturnDataSnapshot,
    },
};

impl ExecutionSnapshot {
    pub fn from_outcome(outcome: &hpsvm::ExecutionOutcome) -> Self;
    pub fn from_simulation(result: &hpsvm::SimulatedTransactionInfo) -> Self;
    pub fn from_failed_simulation(error: &hpsvm::FailedTransactionMetadata) -> Self;
    pub fn run_checks(&self, checks: &[Check], config: &ResultConfig) -> bool;
    pub fn compare_with(
        &self,
        other: &ExecutionSnapshot,
        compares: &[Compare],
        config: &ResultConfig,
    ) -> bool;
}
```

### Feature flags

| Feature | Default | Purpose |
| --- | --- | --- |
| `serde` | off | derive `Serialize` and `Deserialize` for snapshots and DSL enums |
| `schema` | off | optional JSON Schema export for fixture and CLI config tooling |

Notes:

- no `yaml` or `json` feature here; file-format handling belongs in `hpsvm-cli` or `hpsvm-fixture`
- no feature-gated public fields

### Data model

```rust
#[non_exhaustive]
pub struct ExecutionSnapshot {
    pub status: ExecutionStatus,
    pub included: bool,
    pub compute_units_consumed: u64,
    pub fee: u64,
    pub logs: Vec<String>,
    pub return_data: Option<ReturnDataSnapshot>,
    pub inner_instructions: Vec<InnerInstructionSnapshot>,
    pub post_accounts: Vec<AccountSnapshot>,
}

#[non_exhaustive]
pub enum ExecutionStatus {
    Success,
    Failure { kind: String, message: String },
}

#[non_exhaustive]
pub struct AccountSnapshot {
    pub address: solana_address::Address,
    pub lamports: u64,
    pub owner: solana_address::Address,
    pub executable: bool,
    pub rent_epoch: u64,
    pub data: Vec<u8>,
}

#[non_exhaustive]
pub struct ReturnDataSnapshot {
    pub program_id: solana_address::Address,
    pub data: Vec<u8>,
}

#[non_exhaustive]
pub struct InnerInstructionSnapshot {
    pub program_id: solana_address::Address,
    pub stack_height: u32,
    pub data: Vec<u8>,
    pub accounts: Vec<solana_address::Address>,
}
```

### Check DSL

```rust
#[non_exhaustive]
pub enum Check {
    Success,
    Failure,
    Included(bool),
    ComputeUnits(u64),
    Fee(u64),
    ReturnData(Vec<u8>),
    LogContains(String),
    InnerInstructionCount(usize),
    Account(AccountExpectation),
    AllRentExempt,
}

#[non_exhaustive]
pub struct AccountExpectation {
    pub address: solana_address::Address,
    pub lamports: Option<u64>,
    pub owner: Option<solana_address::Address>,
    pub executable: Option<bool>,
    pub data: Option<Vec<u8>>,
    pub data_slice: Option<(usize, Vec<u8>)>,
    pub closed: Option<bool>,
    pub rent_exempt: Option<bool>,
}
```

### Compare DSL

```rust
#[non_exhaustive]
pub enum Compare {
    Status,
    Included,
    ComputeUnits,
    Fee,
    ReturnData,
    Logs,
    InnerInstructionCount,
    Accounts(AccountCompareScope),
}

#[non_exhaustive]
pub enum AccountCompareScope {
    All,
    Only(Vec<solana_address::Address>),
    AllExcept(Vec<solana_address::Address>),
}
```

### Why snapshot instead of comparing core types directly

- serializable by design
- stable contract independent of internal runtime structs
- supports fixture files and CLI output without leaking internal representation details
- failed simulations can still be represented consistently even when no post-state snapshot exists

## Crate Design: `hpsvm-fixture`

### Purpose

Define the canonical `hpsvm` fixture format and provide capture, load, save, and replay helpers.

Unlike Mollusk, v1 should not be proto-first. `hpsvm` does not currently need proto layouts to get value from fixtures.

### Package name

- package: `hpsvm-fixture`
- crate: `hpsvm_fixture`

### Public API

```rust
pub use crate::{
    capture::{CaptureBuilder, CaptureMode},
    codec::FixtureFormat,
    model::{
        Fixture,
        FixtureExpectations,
        FixtureHeader,
        FixtureInput,
        FixtureKind,
        InstructionFixture,
        ProgramBinding,
        RuntimeFixtureConfig,
        TransactionFixture,
    },
    runner::{FixtureExecution, FixtureRunner},
};

impl Fixture {
    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self, FixtureError>;
    pub fn save(
        &self,
        path: impl AsRef<std::path::Path>,
        format: FixtureFormat,
    ) -> Result<(), FixtureError>;
}

impl FixtureRunner {
    pub fn new(vm: hpsvm::HPSVM) -> Self;
    pub fn with_program_elf(
        self,
        program_id: solana_address::Address,
        loader_id: solana_address::Address,
        elf: Vec<u8>,
    ) -> Self;
    pub fn run(&mut self, fixture: &Fixture) -> Result<FixtureExecution, FixtureError>;
    pub fn run_and_validate(
        &mut self,
        fixture: &Fixture,
        config: &hpsvm_result::ResultConfig,
    ) -> Result<bool, FixtureError>;
}
```

### Feature flags

| Feature | Default | Purpose |
| --- | --- | --- |
| `serde` | off | serialize and deserialize fixture models |
| `json-codec` | on | load and save `.json` fixtures |
| `bin-codec` | off | load and save compact binary fixtures |
| `capture` | on | enable capture helpers that turn live execution into fixtures |

Notes:

- `json-codec` is the default because it is easy to review in git
- `bin-codec` exists for scale, not for the initial developer loop
- external format compatibility does not belong here; it belongs in a future adapter crate

### Data model

```rust
#[non_exhaustive]
pub struct Fixture {
    pub header: FixtureHeader,
    pub input: FixtureInput,
    pub expectations: FixtureExpectations,
}

#[non_exhaustive]
pub struct FixtureHeader {
    pub schema_version: u16,
    pub name: String,
    pub kind: FixtureKind,
    pub hpsvm_version: String,
    pub solana_runtime_version: String,
    pub source: Option<String>,
    pub tags: Vec<String>,
}

#[non_exhaustive]
pub enum FixtureKind {
    Transaction,
    Instruction,
}

#[non_exhaustive]
pub enum FixtureInput {
    Transaction(TransactionFixture),
    Instruction(InstructionFixture),
}

#[non_exhaustive]
pub struct TransactionFixture {
    pub runtime: RuntimeFixtureConfig,
    pub programs: Vec<ProgramBinding>,
    pub pre_accounts: Vec<hpsvm_result::AccountSnapshot>,
    pub transaction_bytes: Vec<u8>,
}

#[non_exhaustive]
pub struct InstructionFixture {
    pub runtime: RuntimeFixtureConfig,
    pub programs: Vec<ProgramBinding>,
    pub program_id: solana_address::Address,
    pub instruction_accounts: Vec<solana_instruction::AccountMeta>,
    pub instruction_data: Vec<u8>,
    pub pre_accounts: Vec<hpsvm_result::AccountSnapshot>,
}

#[non_exhaustive]
pub struct RuntimeFixtureConfig {
    pub blockhash: solana_hash::Hash,
    pub slot: u64,
    pub feature_set: Vec<String>,
    pub log_bytes_limit: Option<usize>,
    pub sigverify: bool,
}

#[non_exhaustive]
pub struct ProgramBinding {
    pub program_id: solana_address::Address,
    pub loader_id: solana_address::Address,
    pub role: String,
}

#[non_exhaustive]
pub struct FixtureExpectations {
    pub baseline: hpsvm_result::ExecutionSnapshot,
    pub default_compares: Vec<hpsvm_result::Compare>,
}
```

### Why the fixture stores program bindings but not ELF bytes by default

- keeps fixtures stable across candidate builds
- makes A/B testing two program binaries against the same fixture natural
- avoids huge fixture files in git

Embedding ELF bytes can be added later as an opt-in `EmbeddedElf` variant if real users need self-contained fixtures.

## Crate Design: `hpsvm-cli`

### Purpose

Provide a thin operator-facing binary for fixture replay, candidate-vs-baseline comparison, and CU report generation.

### Package name

- package: `hpsvm-cli`
- binary: `hpsvm`

### Command surface

```text
hpsvm fixture run
hpsvm fixture compare
hpsvm fixture inspect
hpsvm cu report
```

### Public interface

The CLI is primarily a binary. Its stable surface is the command-line contract rather than a library API.

Recommended commands:

#### `hpsvm fixture run`

Execute one fixture or a directory of fixtures against one candidate binary set.

Example:

```text
hpsvm fixture run \
  --fixture fixtures/system-transfer \
  --program 11111111111111111111111111111111=./target/deploy/my_program.so \
  --config checks.yaml
```

#### `hpsvm fixture compare`

Execute the same fixture set against a baseline program set and a candidate program set, then compare snapshots.

Example:

```text
hpsvm fixture compare \
  --fixture fixtures/token \
  --baseline Tokenkeg...=./artifacts/v1.so \
  --candidate Tokenkeg...=./artifacts/v2.so \
  --ignore compute-units
```

#### `hpsvm fixture inspect`

Print decoded fixture contents for debugging and review.

#### `hpsvm cu report`

Run fixture inputs and produce markdown CU summaries and deltas.

### Feature flags

| Feature | Default | Purpose |
| --- | --- | --- |
| `yaml-config` | on | load `Compare` configuration from YAML |
| `json-config` | on | load `Compare` configuration from JSON |
| `fd-compat` | off | enable external fixture adapters once `hpsvm-fixture-fd` exists |
| `cu-report` | on | enable markdown CU report generation |

### Config model

```rust
#[non_exhaustive]
pub struct CompareConfigFile {
    pub compares: Vec<hpsvm_result::Compare>,
}
```

This stays intentionally small. The CLI should not invent a second assertion DSL.

## Crate Design: `hpsvm-bencher`

### Purpose

Provide a developer-facing CU regression product distinct from the existing Criterion and hotpath benchmark suite.

This crate is for application and program authors who want:

- stable markdown CU tables
- delta-to-baseline reporting
- matrix comparison across multiple program binaries

It is not a replacement for the current performance regression infrastructure in `Justfile`.

### Package name

- package: `hpsvm-bencher`
- crate: `hpsvm_bencher`

### Public API

```rust
pub use crate::{
    matrix::{ComputeUnitMatrixBencher, MatrixReport},
    report::{CuDelta, CuReport, CuReportRow},
    single::ComputeUnitBencher,
};

pub type FixtureBenchCase<'a> = (&'a str, &'a hpsvm_fixture::Fixture);

impl ComputeUnitBencher {
    pub fn new(vm: hpsvm::HPSVM) -> Self;
    pub fn case(self, case: FixtureBenchCase<'_>) -> Self;
    pub fn must_pass(self, must_pass: bool) -> Self;
    pub fn baseline_dir(self, path: impl Into<std::path::PathBuf>) -> Self;
    pub fn output_dir(self, path: impl Into<std::path::PathBuf>) -> Self;
    pub fn execute(self) -> Result<CuReport, BenchError>;
}

impl ComputeUnitMatrixBencher {
    pub fn new() -> Self;
    pub fn program(
        self,
        name: impl Into<String>,
        loader_id: solana_address::Address,
        program_id: solana_address::Address,
        elf: Vec<u8>,
    ) -> Self;
    pub fn case(self, case: FixtureBenchCase<'_>) -> Self;
    pub fn execute(self) -> Result<MatrixReport, BenchError>;
}
```

### Feature flags

| Feature | Default | Purpose |
| --- | --- | --- |
| `markdown` | on | emit markdown reports |
| `serde` | off | serialize reports for downstream tooling |

### Data model

```rust
#[non_exhaustive]
pub struct CuReport {
    pub generated_at: String,
    pub solana_runtime_version: String,
    pub rows: Vec<CuReportRow>,
}

#[non_exhaustive]
pub struct CuReportRow {
    pub name: String,
    pub compute_units: u64,
    pub delta: Option<CuDelta>,
    pub pass: bool,
}

#[non_exhaustive]
pub struct CuDelta {
    pub absolute: i64,
    pub percent: f64,
}
```

## Future Crate: `hpsvm-fixture-fd`

### Purpose

Provide import and export adapters for Firedancer-compatible fixture layouts without polluting the canonical `hpsvm` fixture model.

### Package name

- package: `hpsvm-fixture-fd`
- crate: `hpsvm_fixture_fd`

### Public API

```rust
pub struct FiredancerFixture { /* opaque external-format model */ }

impl FiredancerFixture {
    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self, AdapterError>;
    pub fn save(&self, path: impl AsRef<std::path::Path>) -> Result<(), AdapterError>;
}

impl TryFrom<FiredancerFixture> for hpsvm_fixture::Fixture { /* ... */ }
impl TryFrom<hpsvm_fixture::Fixture> for FiredancerFixture { /* ... */ }
```

### Feature flags

| Feature | Default | Purpose |
| --- | --- | --- |
| `prost-codec` | on | decode and encode external protobuf layouts |
| `serde` | off | optional JSON inspection of adapter models |

This crate is explicitly deferred until the native `hpsvm` fixture workflow is solid.

## Instruction-Level Convenience API

This is a core-crate follow-up, not a new crate.

Recommended later additions inside `hpsvm`:

```rust
pub mod instruction {
    pub struct InstructionCase {
        pub program_id: solana_address::Address,
        pub accounts: Vec<solana_instruction::account_meta::AccountMeta>,
        pub data: Vec<u8>,
        pub pre_accounts: Vec<(solana_address::Address, solana_account::Account)>,
    }
}

impl hpsvm::HPSVM {
    pub fn process_instruction_case(
        &self,
        case: &instruction::InstructionCase,
    ) -> Result<hpsvm_result::ExecutionSnapshot, hpsvm::HPSVMError>;
}
```

Rationale:

- absorbs the ergonomic win from Mollusk
- does not redefine `hpsvm` as an instruction-only harness
- remains optional from a workflow perspective, not from a compilation perspective

## Rollout Plan

### Phase 1

- ship `hpsvm-result`
- ship `hpsvm-fixture` with JSON fixtures only
- ship `hpsvm-cli` with `fixture run`, `fixture compare`, and `fixture inspect`

### Phase 2

- ship `hpsvm-bencher`
- add `hpsvm` instruction convenience API
- add binary fixture codec if fixture volume justifies it

### Phase 3

- ship `hpsvm-fixture-fd`
- add CLI compatibility flags for external fixture layouts

## Testing and Validation Expectations

Each new library crate should support at minimum:

- `cargo test`
- `cargo test --all-features`
- `cargo test --no-default-features` when meaningful

Recommended focused test types:

- `hpsvm-result`: snapshot conversion tests, DSL comparison tests, serialization round-trips
- `hpsvm-fixture`: fixture save/load round-trips, replay against `hpsvm`, partial validation paths
- `hpsvm-cli`: golden CLI output tests and end-to-end fixture replay tests
- `hpsvm-bencher`: report delta calculations, matrix output shape, baseline loading behavior

Repo-wide release gate remains the existing sequence from [Justfile](../../../Justfile):

- `just format`
- `just lint`
- `just test`
- `just bdd`
- `just test-all`

## Open Decisions

These are design choices that should be resolved before implementation starts, but they do not block the crate boundaries proposed here.

1. Whether `hpsvm-result::ExecutionStatus` should carry structured error enums or a normalized string representation in v1.
2. Whether `hpsvm-fixture` should add a compact binary codec in its first release or wait until fixture volume makes JSON too costly.
3. Whether `hpsvm-bencher` should accept only fixtures in v1 or support both fixtures and ad hoc execution cases.

## Final Recommendation

Build a companion product layer, not a replacement core.

If this design is accepted, the next implementation plan should start with `hpsvm-result`, because every other new package depends on having a stable snapshot and comparison contract.
