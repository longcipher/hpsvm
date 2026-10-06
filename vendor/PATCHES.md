# Vendored patches

Third-party crates copied into this directory so the workspace can build against
dependency versions that upstream has not published yet.

## `mollusk-svm-fuzz-fixture-firedancer` 0.16.0

**Why.** Upstream pins `agave-feature-set`, `solana-account`, and
`solana-transaction-context` to `4.3.0` — the newest *stable* agave release. This
workspace tracks agave `4.4.0-beta.0`, the first line built against the solana 5.x
type generation (`solana-account` 5.1.0, `solana-transaction` 5.1.0,
`solana-sysvar` 5.0.0, `solana-svm-transaction` 6.0.0).

Those two cannot coexist in one build. `4.3.0` is semver-compatible with
`4.4.0-beta.0`, but a semver range excludes prereleases, and Cargo will not keep
two semver-compatible copies of the same crate. So the requirement fails to
resolve outright rather than merely duplicating.

**What was changed.** Only the three version requirements in `Cargo.toml`,
widened from `4.3.0` to `>=4.3.0`. No source file was modified — every symbol the
crate uses still exists upstream:

| Symbol | Present in |
|---|---|
| `agave_feature_set::FeatureSet` | `agave-feature-set` 4.4.0-beta.0 |
| `FeatureSet::all_enabled()` | `agave-feature-set` 4.4.0-beta.0 |
| `agave_feature_set::disable_sbpf_v0_execution::id()` | `agave-feature-set` 4.4.0-beta.0 |
| `agave_feature_set::reenable_sbpf_v0_execution::id()` | `agave-feature-set` 4.4.0-beta.0 |
| `solana_transaction_context::instruction_accounts::InstructionAccount` | `solana-transaction-context` 4.4.0-beta.0 |
| `solana_account::Account` | `solana-account` 5.1.0 |

`mollusk-svm-fuzz-fs` 0.16.0, its one non-Solana-runtime dependency, needs no
patch: it depends only on `bs58`, `prost`, `serde`, `serde_json`, and
`solana-keccak-hasher`, none of which conflict with the 4.4/5.x line.

**When to drop this.** As soon as upstream publishes a build against agave 4.4
(the crate's release history shows per-agave prerelease tags such as
`0.15.0-agave-4.3.0-beta.0`, so expect `0.16.x-agave-4.4.0-beta.0`). Delete this
directory and the matching `[patch.crates-io]` entry in the workspace
`Cargo.toml`; nothing else in the workspace references the vendor path.

**Refreshing the patch.** Copy `src/`, `proto/`, `build.rs`, and `LICENSE` from
the new crates.io release, then re-apply only the three requirement widenings and
re-check the symbol table above.