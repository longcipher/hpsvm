//! Shared lookup for the SBF test programs that hpsvm's integration tests,
//! benches, and doctests load at run time.
//!
//! The programs are ordinary Rust crates that live under `crates/hpsvm/test_programs`
//! and must be compiled for the SBF target, which needs Solana's `cargo build-sbf`.
//! That toolchain is not available everywhere — notably the anza platform-tools
//! rustc is currently broken on some macOS hosts — so callers should treat an
//! absent program as "skip", not as a failure.
//!
//! Each program is resolved from the first directory in [`program_dirs`] that
//! contains it:
//!
//! 1. `test_programs/target/deploy/<name>`, where `cargo build-sbf` writes its output. This is a
//!    gitignored build artifact.
//! 2. `test_programs/prebuilt/<name>`, for binaries vendored into the repository so that a checkout
//!    without the SBF toolchain can still exercise the suite.
//!
//! Use [`read_program`] when a test only needs the bytes; it prints an
//! actionable notice when the program is missing and returns `None` so the
//! caller can skip.

#![allow(clippy::print_stderr)]

use std::{
    fs,
    path::{Path, PathBuf},
};

/// SBF program that adds one to a `u32` counter stored in the first account.
pub const COUNTER: &str = "counter.so";

/// SBF program that always fails with `ProgramError::Custom(0)`.
pub const FAILURE: &str = "failure.so";

/// SBF program that calls the `sol_burn_cus` custom syscall, taking the amount
/// to burn from the first eight instruction-data bytes.
pub const CUSTOM_SYSCALL: &str = "test_program_custom_syscall.so";

/// SBF program that traps unless the `Clock` sysvar's `unix_timestamp` is below
/// 100, i.e. within the first minute and forty seconds after the epoch.
pub const CLOCK_EXAMPLE: &str = "hpsvm_clock_example.so";

/// Every SBF program the suite can load, for callers that report on all of them
/// at once.
pub const PROGRAMS: [&str; 4] = [COUNTER, FAILURE, CUSTOM_SYSCALL, CLOCK_EXAMPLE];

/// Returns the directories searched for SBF test programs, in priority order.
#[must_use]
pub fn program_dirs() -> Vec<PathBuf> {
    let test_programs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../hpsvm/test_programs");
    vec![test_programs.join("target/deploy"), test_programs.join("prebuilt")]
}

/// Returns the command that builds the SBF test programs.
#[must_use]
pub const fn build_command() -> &'static str {
    "cd crates/hpsvm/test_programs && cargo build-sbf"
}

/// Locates `name` in the first of `dirs` that holds it.
///
/// This is the pure lookup behind [`program_dirs`]; it takes the search roots
/// explicitly so it can be tested without a real `cargo build-sbf` output tree.
fn find_in(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    dirs.iter().map(|dir| dir.join(name)).find(|candidate| candidate.is_file())
}

/// Prints an actionable notice about a program that is not present anywhere.
fn report_missing(name: &str, dirs: &[PathBuf]) {
    eprintln!("skipping: SBF program `{name}` was not found; searched:");
    for dir in dirs {
        eprintln!("    {}", dir.display());
    }
    eprintln!("  fix: build the SBF programs with `{}`", build_command());
}

/// Prints an actionable notice about a program that exists but cannot be read.
fn report_unreadable(name: &str, path: &Path, error: &std::io::Error) {
    eprintln!(
        "skipping: SBF program `{name}` was found at {} but could not be read: {error}",
        path.display()
    );
    eprintln!("  fix: rebuild it with `{}`", build_command());
}

/// Returns the path of SBF program `name`, or `None` when it is present in none
/// of [`program_dirs`].
///
/// Callers that also want the bytes, or the missing-program guidance, should
/// use [`read_program`] instead of searching by hand.
#[must_use]
pub fn find_program(name: &str) -> Option<PathBuf> {
    find_in(&program_dirs(), name)
}

/// Reads SBF program `name`, or returns `None` after printing why it is
/// unavailable.
///
/// A `None` result means the program is missing, not that it is malformed: the
/// caller should skip the test rather than fail it.
#[must_use]
pub fn read_program(name: &str) -> Option<Vec<u8>> {
    let dirs = program_dirs();
    let Some(path) = find_in(&dirs, name) else {
        report_missing(name, &dirs);
        return None;
    };
    match fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) => {
            report_unreadable(name, &path, &error);
            None
        }
    }
}

/// Reads SBF program `name` or aborts.
///
/// Meant for callers that cannot meaningfully skip — benches, where silently
/// measuring nothing would hide the problem — and for the repository's
/// prerequisite check.
#[must_use]
pub fn read_program_or_panic(name: &str) -> Vec<u8> {
    let dirs = program_dirs();
    let Some(path) = find_in(&dirs, name) else {
        report_missing(name, &dirs);
        panic!("required SBF program `{name}` is unavailable");
    };
    fs::read(&path).unwrap_or_else(|error| panic!("failed to read SBF program `{name}`: {error}"))
}

/// Reports every SBF program that is currently unavailable and returns their
/// names.
#[must_use]
pub fn report_unavailable() -> Vec<&'static str> {
    let dirs = program_dirs();
    let mut missing = Vec::new();
    for name in PROGRAMS {
        if find_in(&dirs, name).is_none() {
            report_missing(name, &dirs);
            missing.push(name);
        }
    }
    missing
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{COUNTER, PROGRAMS, build_command, find_in, find_program, program_dirs};

    /// Returns a per-test scratch directory, emptied first.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hpsvm-test-support-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Creates `base/sub` and returns it.
    fn subdir(base: &Path, sub: &str) -> PathBuf {
        let dir = base.join(sub);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn program_dirs_prefers_deploy_then_prebuilt() {
        let dirs = program_dirs();
        assert_eq!(dirs.len(), 2);
        assert!(dirs[0].ends_with("test_programs/target/deploy"));
        assert!(dirs[1].ends_with("test_programs/prebuilt"));
    }

    #[test]
    fn build_command_targets_the_test_programs_workspace() {
        assert_eq!(build_command(), "cd crates/hpsvm/test_programs && cargo build-sbf");
    }

    #[test]
    fn find_in_returns_the_first_directory_that_has_the_file() {
        let base = scratch("first-wins");
        let deploy = subdir(&base, "deploy");
        let prebuilt = subdir(&base, "prebuilt");
        std::fs::write(deploy.join(COUNTER), b"from-deploy").unwrap();
        std::fs::write(prebuilt.join(COUNTER), b"from-prebuilt").unwrap();

        let found = find_in(&[deploy.clone(), prebuilt], COUNTER).unwrap();
        assert_eq!(found, deploy.join(COUNTER));
        assert_eq!(std::fs::read(found).unwrap(), b"from-deploy");
    }

    #[test]
    fn find_in_falls_through_to_a_later_directory() {
        let base = scratch("fallthrough");
        let deploy = subdir(&base, "deploy");
        let prebuilt = subdir(&base, "prebuilt");
        std::fs::write(prebuilt.join(COUNTER), b"from-prebuilt").unwrap();

        let found = find_in(&[deploy, prebuilt], COUNTER).unwrap();
        assert_eq!(std::fs::read(found).unwrap(), b"from-prebuilt");
    }

    #[test]
    fn find_in_ignores_a_directory_named_like_a_program() {
        let base = scratch("dir-is-not-a-file");
        let deploy = subdir(&base, "deploy");
        // A directory must not be mistaken for the program binary.
        std::fs::create_dir_all(deploy.join(COUNTER)).unwrap();

        assert!(find_in(&[deploy], COUNTER).is_none());
    }

    #[test]
    fn find_in_returns_none_when_no_directory_has_the_file() {
        let base = scratch("absent");
        let deploy = subdir(&base, "deploy");
        assert!(find_in(&[deploy], "no-such-program.so").is_none());
    }

    #[test]
    fn find_in_searches_nothing_given_no_directories() {
        assert!(find_in(&[], COUNTER).is_none());
    }

    #[test]
    fn known_program_names_are_all_sbf_shared_objects() {
        assert_eq!(PROGRAMS.len(), 4);
        for name in PROGRAMS {
            assert!(name.ends_with(".so"), "{name} is not an SBF shared object");
            // A lookup for a program this checkout has not built must be a
            // clean miss rather than a panic.
            let _ = find_program(name);
        }
    }

    #[test]
    fn find_program_returns_none_for_an_unknown_name() {
        assert!(find_program("definitely-not-a-program.so").is_none());
    }
}
