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
  just check-mutation-scope

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

# Mutation testing is intentionally absent from the default loop and from CI.
#
# Whole-workspace mutation runs are PROHIBITED here: a `cargo mutants` run over
# the whole workspace takes hours and rarely finishes, so it is never a gate and
# never a completion check. Only the lines the current change touched are
# mutated, within the /pb-build budget: at most 3 files, 60 mutants, 300 s.
#
#   just mutants                            # every changed .rs file
#   just mutants crates/core/src/parser.rs  # one specific file
#
# The budget is verified before the run starts, so an over-large change fails
# immediately instead of grinding for hours. A surviving mutant is a test
# failure: strengthen the assertion, or record an exemption in
# `.cargo/mutants.toml` under `exclude_re` with a one-line reason.
#
# Mutate the lines this change touched, within the /pb-build budget.
mutants *args:
  #!/usr/bin/env bash
  set -euo pipefail
  max_files=3
  max_mutants=60
  max_seconds=300

  base=""
  upstream=$(git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' 2>/dev/null || true)
  for ref in "$upstream" origin/main origin/master main master; do
    [ -n "$ref" ] || continue
    if git rev-parse --verify --quiet "$ref" >/dev/null 2>&1; then
      base=$(git merge-base HEAD "$ref" 2>/dev/null || true)
      [ -n "$base" ] && break
    fi
  done
  [ -n "$base" ] || base=HEAD

  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
  if [ "$#" -gt 0 ]; then
    printf '%s\n' "$@" | sed '/^[[:space:]]*$/d' > "$work/files"
  else
    git diff --name-only --diff-filter=ACMR "$base" -- '*.rs' | sort -u > "$work/files"
  fi
  file_count=$(grep -c . "$work/files" || true)
  if [ "$file_count" -eq 0 ]; then
    echo "error: no changed Rust files against $base; there is nothing to mutate." >&2
    echo "       Whole-workspace mutation is prohibited: it takes hours and rarely finishes." >&2
    exit 2
  fi
  if [ "$file_count" -gt "$max_files" ]; then
    echo "error: $file_count files exceed the ${max_files}-file mutation budget." >&2
    echo "       Mutate fewer files at a time; never raise the budget, never run the workspace." >&2
    exit 3
  fi

  : > "$work/diff"
  while IFS= read -r file; do
    [ -n "$file" ] || continue
    git diff --no-ext-diff --no-textconv "$base" -- "$file" >> "$work/diff" || true
  done < "$work/files"
  if ! grep -q '^diff --git ' "$work/diff"; then
    echo "error: the diff against $base is empty; there is nothing to mutate." >&2
    exit 2
  fi
  RUSTC_WRAPPER= cargo mutants --workspace --in-diff "$work/diff" --list --no-times > "$work/list" 2>&1 \
    || { cat "$work/list" >&2; echo "error: cargo-mutants could not list the mutants." >&2; exit 1; }
  mutant_count=$(grep -Ec ':[0-9]+:[0-9]+:' "$work/list" || true)
  if [ "$mutant_count" -gt "$max_mutants" ]; then
    echo "error: $mutant_count mutants exceed the ${max_mutants}-mutant budget." >&2
    echo "       Narrow the change; never raise the budget, never fall back to a workspace run." >&2
    exit 3
  fi
  if [ "$mutant_count" -eq 0 ]; then
    echo "ok: no mutants in the changed lines."
    exit 0
  fi

  echo "mutating $mutant_count mutant(s) in $file_count file(s) (budget: $max_mutants mutants / ${max_seconds}s)"
  if command -v timeout >/dev/null 2>&1; then
    timeout "$max_seconds" env RUSTC_WRAPPER= cargo mutants --workspace --in-diff "$work/diff" --timeout-multiplier 2 --no-times
  elif command -v gtimeout >/dev/null 2>&1; then
    gtimeout "$max_seconds" env RUSTC_WRAPPER= cargo mutants --workspace --in-diff "$work/diff" --timeout-multiplier 2 --no-times
  else
    echo "warning: neither 'timeout' nor 'gtimeout' is available; the ${max_seconds}s wall-clock cap is not enforced." >&2
    echo "         Install coreutils ('brew install coreutils') to get 'gtimeout'." >&2
    env RUSTC_WRAPPER= cargo mutants --workspace --in-diff "$work/diff" --timeout-multiplier 2 --no-times
  fi

# Fail if an unbounded mutation command reappears in a build or CI file.
# Whole-workspace mutation runs take hours and rarely finish, so they are
# prohibited: the only permitted forms are a diff-scoped (`--in-diff`), a
# file-scoped (`--file`), or a branch-scoped (`--since`) run.
#
# Check that no build or CI file regains an unbounded mutation command.
check-mutation-scope:
  #!/usr/bin/env bash
  set -euo pipefail
  status=0
  for file in Justfile justfile Makefile makefile .github/workflows/*.yml .github/workflows/*.yaml; do
    [ -f "$file" ] || continue
    offenders=$(grep -nE 'cargo[[:space:]]+mutants' "$file" \
      | grep -vE '^[0-9]+:[[:space:]]*#' \
      | grep -vE -- '--in-diff|--file|--list|--since|-f[[:space:]]' \
      || true)
    if [ -n "$offenders" ]; then
      echo "error: $file runs an unbounded mutation command:" >&2
      printf '%s\n' "$offenders" >&2
      echo "       Whole-workspace mutation runs are prohibited. Use the diff-scoped 'just mutants'." >&2
      status=1
    fi
  done
  exit "$status"

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

# Update all dependencies
update-deps:
  cargo update

# Full check (lint + test + build + audit)
full-check: lint test build audit
