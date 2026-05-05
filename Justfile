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
  cargo +nightly clippy --fix --all --allow-dirty

# Run all lints
lint:
	typos
	rumdl check .
	cargo sort -w -g -c
	cargo +nightly fmt --all -- --check
	cargo +nightly clippy --all -- -D warnings -A deprecated -A clippy::missing_const_for_fn -A clippy::unwrap_used -A clippy::or_fun_call -A unused-mut -A clippy::result_large_err
	cargo machete

# Run tests
test:
  cargo test --all-features

# Run BDD scenarios
bdd:
  cargo test -p hpsvm --test bdd

# Run both TDD and BDD suites
test-all:
  cargo test --all-features
  cargo test -p hpsvm --test bdd

# Run tests with coverage
test-coverage:
  cargo tarpaulin --all-features --workspace --timeout 300

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

# Refresh committed default runtime baselines from the current machine
bench-runtime-baseline-refresh: bench-runtime
  mkdir -p docs/benchmarks/runtime-baselines
  python3 scripts/criterion_export.py target/criterion docs/benchmarks/runtime-baselines --include simple_bench --include max_perf --include core_interfaces

# Compare a fresh default runtime benchmark run against committed baselines
bench-runtime-baseline-compare:
  mkdir -p target/runtime-benchmarks
  python3 scripts/criterion_export.py target/criterion target/runtime-benchmarks --include simple_bench --include max_perf --include core_interfaces
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

# Publish all crates to crates.io
publish:
  cargo publish --workspace

# Check for Chinese characters
check-cn:
  rg --line-number --column "\p{Han}"

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
  cargo install cargo-machete
  cargo install cargo-sort
  cargo install typos-cli

# Generate documentation for the workspace
docs:
  cargo doc --no-deps --open
