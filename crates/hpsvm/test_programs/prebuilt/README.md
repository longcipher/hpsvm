# Prebuilt SBF test programs

This directory holds checked-in SBF binaries for the test programs under
`../counter`, `../failure`, `../custom-syscall`, and `../clock-example`.

## Why it exists

Those crates compile only for the SBF target, which requires Solana's
`cargo build-sbf`. That toolchain is not available on every host — on some
macOS versions the anza platform-tools rustc emits proc-macro dylibs that dyld
rejects with `mis-aligned LINKEDIT string pool`, which cargo then reports as
`can't find crate for <some>_derive`.

Integration tests, benches, and doctests resolve each program from the first
directory that contains it:

1. `../target/deploy/<name>` — freshly built by `cargo build-sbf`.
2. `./prebuilt/<name>` — this directory.

So a checkout without the SBF toolchain still runs the suite: tests that need a
program that is missing from both directories report that they are skipping and
tell you the command to build it.

## Expected contents

| File | Source crate |
| --- | --- |
| `counter.so` | `../counter` |
| `failure.so` | `../failure` |
| `test_program_custom_syscall.so` | `../custom-syscall` |
| `hpsvm_clock_example.so` | `../clock-example` |

## Adding or refreshing binaries

Build the programs and copy the results in, keeping the names above:

    cd crates/hpsvm/test_programs && cargo build-sbf
    cp target/deploy/counter.so \
       target/deploy/failure.so \
       target/deploy/test_program_custom_syscall.so \
       target/deploy/hpsvm_clock_example.so \
       prebuilt/

The lookup itself lives in `crates/hpsvm-test-support`; search for
`program_dirs` there if you need to change the resolution order.
