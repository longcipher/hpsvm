#!/usr/bin/env bash
# Verify the SBF test programs exist before running tests that load them.
#
# Integration tests, benches, and the `lib.rs` doctests load these binaries at run
# time. When none are available those tests skip and print an explanation rather
# than fail, which keeps `cargo test` green on a host without the SBF toolchain
# but also means coverage silently disappears. `just test-all` runs this script
# first so the full suite only claims success when the programs are really there.
#
# Each program is resolved from the first directory that holds it, matching
# `hpsvm_test_support::program_dirs`:
#
#   1. crates/hpsvm/test_programs/target/deploy  (cargo build-sbf output)
#   2. crates/hpsvm/test_programs/prebuilt       (vendored into the repository)
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_programs="$repo_root/crates/hpsvm/test_programs"
search_dirs=("$test_programs/target/deploy" "$test_programs/prebuilt")

required=(
  counter.so
  failure.so
  hpsvm_clock_example.so
  test_program_custom_syscall.so
)

resolve() {
  # Prints the directory holding $1, or nothing.
  local program="$1" dir
  for dir in "${search_dirs[@]}"; do
    if [[ -f "$dir/$program" ]]; then
      printf '%s' "$dir"
      return 0
    fi
  done
  return 1
}

missing=0
for program in "${required[@]}"; do
  if found="$(resolve "$program")"; then
    printf 'found: %s/%s\n' "$found" "$program"
  else
    printf 'missing: %s (searched %s)\n' "$program" "$(IFS=', '; printf '%s' "${search_dirs[*]}")" >&2
    missing=1
  fi
done

if [[ $missing -eq 0 ]]; then
  printf 'All SBF test programs are available.\n'
  exit 0
fi

cat >&2 <<'EOF'

The SBF test programs are missing.

Several integration tests, benches, and the `lib.rs` doctests load them at run
time. Without them those tests skip with a notice instead of failing, so
`cargo test` stays green but stops covering the SBF execution paths. Build them
with:

    cd crates/hpsvm/test_programs && cargo build-sbf

`target/deploy` holds that output. Binaries can also be vendored into
`crates/hpsvm/test_programs/prebuilt/`, which is checked in; see the README
there for the expected file names.

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
