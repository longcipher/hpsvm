# Default recipe to display help
default:
  @just --list

# Format all code
format:
  rumdl fmt .
  cargo sort -w -g
  cargo +nightly fmt --all

# Auto-fix linting issues
fix:
  rumdl check --fix .
  RUSTC_WRAPPER= cargo +nightly clippy --fix --all --allow-dirty
  cargo workspace-inheritance-check --fix

# Run all lints
lint:
  rumdl check .
  cargo sort -w -g -c
  cargo workspace-inheritance-check
  cargo +nightly fmt --all -- --check
  RUSTC_WRAPPER= cargo +nightly clippy --all -- -D warnings -A deprecated -A clippy::missing_const_for_fn -A clippy::unwrap_used -A clippy::or_fun_call -A unused-mut -A clippy::result_large_err
  cargo shear

# Run tests
test:
  cargo test --all-features

# Verify the SBF test programs the integration tests and benches load
sbf-test-programs:
  @if ! ./scripts/check-sbf-test-programs.sh; then \
    echo "warning: SBF test programs are missing. On macOS this is expected due to a known"; \
    echo "         toolchain bug (mis-aligned LINKEDIT string pool). Tests that need them"; \
    echo "         will skip. Build on Linux and copy target/deploy/ to fix."; \
  fi

# The `--all-targets` pass compiles benches but skips doctests, so the doc
# examples in `lib.rs` and the README are verified by a separate pass below.

# Run the full test suite across the workspace
test-all: sbf-test-programs
  cargo test --workspace --all-features --all-targets
  cargo test --workspace --all-features --doc

# Run tests with coverage
test-coverage:
  cargo tarpaulin --all-features --workspace --timeout 300

# Run mutation testing across the workspace
mutants:
  cargo mutants --workspace

# The diff must be produced with `--no-ext-diff --no-textconv`: a global
# `diff.external` (difftastic, delta, ...) makes `git diff` emit its own tool
# output instead of a patch, and cargo-mutants then reads an empty diff, mutates
# nothing and still exits 0. That silently turns this gate off.
#
# Three-dot covers committed work on the branch; the appended `git diff HEAD`
# covers uncommitted work, so the gate is still useful before a commit.

# Run mutation testing on everything not yet merged into master (fast feedback)
mutants-quick:
  mkdir -p target/mutants
  git diff --no-ext-diff --no-textconv origin/master...HEAD > target/mutants/branch.diff
  git diff --no-ext-diff --no-textconv HEAD >> target/mutants/branch.diff
  @if ! grep -q '^diff --git ' target/mutants/branch.diff; then \
    echo 'error: target/mutants/branch.diff contains no file headers, so there is nothing to mutate.' >&2; \
    echo '       This usually means the base ref is wrong or an external diff driver is active.' >&2; \
    exit 1; \
  fi
  cargo mutants --workspace --in-diff target/mutants/branch.diff

# List the mutants that would be generated without running them
mutants-list:
  cargo mutants --workspace --list

# Run mutation testing across the workspace (canonical alias of `mutants`)
mutation: mutants

# Build entire workspace
build:
  cargo build --workspace

# Run hotpath-enabled benchmark baselines
bench-hotpath:
  mkdir -p target/hotpath
  HPSVM_HOTPATH=1 cargo bench -p hpsvm --features hotpath --bench core_interfaces -- --noplot --sample-size 10 --measurement-time 1 --warm-up-time 1
  HPSVM_HOTPATH=1 cargo bench -p hpsvm --features hotpath --bench max_perf -- --noplot --sample-size 10 --measurement-time 1 --warm-up-time 1

# Run the steady-state benchmark with hotpath and register trace metrics enabled
bench-hotpath-trace:
  mkdir -p target/hotpath
  HPSVM_HOTPATH=1 HPSVM_TRACE_METRICS=1 cargo bench -p hpsvm --features "hotpath register-tracing" --bench max_perf -- --noplot --sample-size 10 --measurement-time 1 --warm-up-time 1
  HPSVM_HOTPATH=1 HPSVM_TRACE_METRICS=1 cargo bench -p hpsvm --features "hotpath register-tracing" --bench simple_bench -- --noplot --sample-size 10 --measurement-time 1 --warm-up-time 1

# Run default runtime benchmarks without hotpath overhead
bench-runtime:
  cargo bench -p hpsvm --bench simple_bench -- --noplot --sample-size 10 --measurement-time 1 --warm-up-time 1
  cargo bench -p hpsvm --bench max_perf -- --noplot --sample-size 10 --measurement-time 1 --warm-up-time 1
  cargo bench -p hpsvm --bench core_interfaces -- --noplot --sample-size 10 --measurement-time 1 --warm-up-time 1
  cargo bench -p hpsvm --bench program_cache -- --noplot --sample-size 10 --measurement-time 1 --warm-up-time 1

# Refresh committed default runtime baselines from the current machine
bench-runtime-baseline-refresh: bench-runtime
  mkdir -p docs/benchmarks/runtime-baselines
  python3 scripts/criterion_export.py target/criterion docs/benchmarks/runtime-baselines --include simple_bench --include max_perf --include core_interfaces --include program_cache

# Compare a fresh default runtime benchmark run against committed baselines
bench-runtime-baseline-compare:
  mkdir -p target/runtime-benchmarks
  python3 scripts/criterion_export.py target/criterion target/runtime-benchmarks --include simple_bench --include max_perf --include core_interfaces --include program_cache
  python3 scripts/criterion_compare.py docs/benchmarks/runtime-baselines target/runtime-benchmarks --output target/runtime-benchmarks/summary.md

# Refresh committed hotpath benchmark baselines from the current machine
bench-baseline-refresh: bench-hotpath
  mkdir -p docs/benchmarks/baselines
  cp target/hotpath/core_interfaces.json docs/benchmarks/baselines/core_interfaces.json
  cp target/hotpath/max_perf.json docs/benchmarks/baselines/max_perf.json

# Compare current hotpath benchmark output against committed baselines
bench-baseline-compare:
  python3 scripts/hotpath_compare.py docs/benchmarks/baselines target/hotpath --output target/hotpath/summary.md

# Check all targets compile
check:
  cargo check --all-targets --all-features

# Verify all workspace crates package and compile from their published layout
publish-check:
  cargo package --workspace --allow-dirty

# Mirrors the `Publish to crates.io` step in .github/workflows/release.yml:
# every workspace member declared in `[workspace.dependencies]` is published,
# preceded by the `hpsvm-fd-fixture` fork (source in vendor/), and the
# cargo-metadata guard there fails the release if the member list rots.

# Publish all crates to crates.io, in dependency order (dev-deps included)
publish:
  (cd vendor/hpsvm-fd-fixture && cargo publish --registry crates-io --allow-dirty --no-verify)
  sleep 20
  cargo publish -p hpsvm-macros --registry crates-io --allow-dirty --no-verify
  sleep 20
  cargo publish -p hpsvm-test-support --registry crates-io --allow-dirty --no-verify
  sleep 20
  cargo publish -p hpsvm --registry crates-io --allow-dirty --no-verify
  sleep 20
  cargo publish -p hpsvm-fixture --registry crates-io --allow-dirty --no-verify
  sleep 20
  cargo publish -p hpsvm-inspectors --registry crates-io --allow-dirty --no-verify
  sleep 20
  cargo publish -p hpsvm-cli --registry crates-io --allow-dirty --no-verify

# Check for Chinese characters in English-only sources and docs
check-cn:
  #!/usr/bin/env bash
  set -euo pipefail
  if rg --line-number --column "\p{Han}" --glob '*.md' --glob '*.toml' --glob '!target/**' --glob '!specs/**'; then
    echo "error: Chinese characters found in English-only files" >&2
    exit 1
  fi

# Full CI check
ci: lint test-all build

# ============================================================
# Maintenance & Tools
# ============================================================

# Clean build artifacts
clean:
  cargo clean

# Install all required development tools
setup:
  cargo install cargo-shear
  cargo install cargo-sort
  cargo install cargo-workspace-inheritance-check
  cargo install typos-cli
  cargo install rumdl
  cargo install cargo-tarpaulin
  cargo install cargo-mutants
  cargo install cargo-nextest
  cargo install cargo-audit
  cargo install cargo-outdated

# Generate documentation for the workspace
docs:
  cargo doc --no-deps --open

# Run security audit
audit:
  cargo audit

# Check for outdated dependencies
outdated:
  cargo outdated

# Run clippy with all lints
clippy-all:
  RUSTC_WRAPPER= cargo +nightly clippy --all --all-targets --all-features -- -W clippy::pedantic -W clippy::nursery -W clippy::cargo

# Run tests with nextest
test-nextest:
  cargo nextest run --workspace --all-features

# Run miri on unsafe code
miri:
  cargo +nightly miri test --all-features

# Check for unused dependencies
unused-deps:
  cargo shear

# Update all dependencies
update-deps:
  cargo update

# Full check (lint + test + build + audit)
full-check: lint test build audit
