#!/usr/bin/env bash
# Verify the SBF test programs exist before running tests that load them.
#
# Several integration tests, benches, and the `lib.rs` doctests read these
# binaries with std::fs::read at run time, so when they are absent the suite
# still compiles and then fails with an opaque `Os { code: 2 }` from deep inside
# a test rather than as the missing prerequisite it actually is. This script
# turns that into one clear message.
#
# The programs are gitignored build artifacts: a clean checkout needs
# `cargo build-sbf` before the affected tests can run.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
deploy_dir="$repo_root/crates/hpsvm/test_programs/target/deploy"

required=(
  counter.so
  failure.so
  hpsvm_clock_example.so
  test_program_custom_syscall.so
)

missing=0
for program in "${required[@]}"; do
  if [[ ! -f "$deploy_dir/$program" ]]; then
    printf 'missing: %s\n' "$deploy_dir/$program" >&2
    missing=1
  fi
done

if [[ $missing -eq 0 ]]; then
  printf 'SBF test programs present in %s\n' "$deploy_dir"
  exit 0
fi

cat >&2 <<'EOF'

The SBF test programs are missing.

Several integration tests, benches, and the `lib.rs` doctests load them at run
time, so the suite compiles but those tests fail once they are executed. Build
them with:

    cd crates/hpsvm/test_programs && cargo build-sbf

These are gitignored build artifacts, so a clean checkout always needs this
step; CI performs it automatically.

If `cargo build-sbf` itself fails, the problem is the local toolchain rather
than this repository. On macOS the anza platform-tools rustc can emit proc-macro
dylibs that dyld rejects with:

    mis-aligned LINKEDIT string pool

which cargo then reports as `can't find crate for <some>_derive` (typically
`borsh_derive` or `bytemuck_derive`). That is a toolchain bug that reproduces
in a two-crate scratch project. Work around it by building the programs on
another host (Linux CI does this) and copying `target/deploy` back.
EOF

exit 1
