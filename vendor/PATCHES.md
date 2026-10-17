# Vendored third-party sources

Third-party code copied into this directory because upstream has not published a
compatible build yet, published to crates.io under a project-owned name.

## `hpsvm-fd-fixture` (fork of `mollusk-svm-fuzz-fixture-firedancer`)

**Why.** Upstream pins `agave-feature-set`, `solana-account`, and
`solana-transaction-context` to `4.3.0` — the newest *stable* agave release.
This workspace tracks agave `4.5.0-alpha.2`, the first line built against the
solana 5.x type generation (`solana-account` 5.1.0, `solana-transaction` 5.1.0,
`solana-sysvar` 5.0.0, `solana-svm-transaction` 6.0.0).

Those two cannot coexist in one build. `4.3.0` is semver-compatible with
`4.5.0-alpha.2`, but a semver range excludes prereleases, and Cargo will not
keep two semver-compatible copies of the same crate. So the requirement fails to
resolve outright rather than merely duplicating.

**What was changed.** Only the three version requirements in `Cargo.toml`,
widened from `4.3.0` to `*` (a range such as `>=4.3.0` still excludes the
`4.5.0-alpha.x` pre-release, so `*` is the only requirement that resolves
against the whole 4.x line). No source file was modified — every symbol the
crate uses still exists upstream:

| Symbol | Present in |
|---|---|
| `agave_feature_set::FeatureSet` | `agave-feature-set` 4.5.0-alpha.2 |
| `FeatureSet::all_enabled()` | `agave-feature-set` 4.5.0-alpha.2 |
| `agave_feature_set::disable_sbpf_v0_execution::id()` | `agave-feature-set` 4.5.0-alpha.2 |
| `agave_feature_set::reenable_sbpf_v0_execution::id()` | `agave-feature-set` 4.5.0-alpha.2 |
| `solana_transaction_context::instruction_accounts::InstructionAccount` | `solana-transaction-context` 4.5.0-alpha.2 |
| `solana_account::Account` | `solana-account` 5.1.0 |

`mollusk-svm-fuzz-fs` 0.16.2, its one non-Solana-runtime dependency, needs no
patch: it depends only on `bs58`, `prost`, `serde`, `serde_json`, and
`solana-keccak-hasher`, none of which conflict with the 4.5/5.x line.

**How it is consumed.** The fork is published to crates.io as
`hpsvm-fd-fixture` (version-tracked with upstream, currently 0.16.2) and pulled
in through `[workspace.dependencies]` with a `path` + `version` entry, so the
workspace builds from this directory while published manifests depend on the
crates.io release. `crates/hpsvm-fixture/fuzz` uses the path directly. There is
no `[patch.crates-io]` entry: patches are stripped when a crate is packaged, so
a patched dependency cannot be published — that is exactly why the fork exists.

**Refreshing the fork.** Copy `src/`, `proto/`, `build.rs`, and `LICENSE` from
the new upstream release, re-apply the three requirement widenings, re-check the
symbol table above, bump `version` to the upstream version, and publish:

```sh
cd vendor/hpsvm-fd-fixture
cargo publish --registry crates-io --allow-dirty --no-verify
```

Then update the version in the root `[workspace.dependencies]` entry and in
`crates/hpsvm-fixture/fuzz/Cargo.toml`. The release workflow publishes the fork
before the workspace members, so future releases only need the version bump.

**When to drop this.** As soon as upstream publishes a build against agave 4.5
(the crate's release history shows per-agave prerelease tags such as
`0.15.0-agave-4.3.0-beta.0`, so expect `0.16.x-agave-4.5.0-alpha.2` or newer).
Point `hpsvm-fixture` back at `mollusk-svm-fuzz-fixture-firedancer`, delete this
directory, and yank the fork if nothing else consumes it.
