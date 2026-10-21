//! The outcome of a comparison or a check.
//!
//! Both [`ExecutionSnapshot::compare_with`](crate::ExecutionSnapshot::compare_with)
//! and [`ExecutionSnapshot::run_checks`](crate::ExecutionSnapshot::run_checks)
//! evaluate a list of predicates, stop at the first one that does not hold,
//! and report the failure. The reporting lives here so the two share one
//! implementation and one failure shape.
//!
//! A failure is *data*, not a panic: the library never aborts the caller's
//! process because a fixture did not match. The `panic` flag on
//! [`ResultConfig`](crate::ResultConfig) is an explicit opt-in for callers
//! that want the old fail-fast behaviour, and it is off by default.

use std::io::Write;

use crate::ResultConfig;

/// Whether a comparison or a check failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MismatchKind {
    /// A `Compare` predicate did not hold.
    Comparison,
    /// A `Check` predicate did not hold.
    Check,
}

impl MismatchKind {
    /// Returns the human-readable label of this kind.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Comparison => "comparison",
            Self::Check => "check",
        }
    }
}

/// A failed comparison or check.
///
/// Carries the kind of evaluation, the name of the failing predicate and a
/// human-readable description, so a caller can report the failure without
/// parsing a message string.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Mismatch {
    /// Whether a comparison or a check failed.
    pub kind: MismatchKind,
    /// The name of the failing predicate.
    pub predicate: String,
    /// A human-readable description of the failure.
    pub message: String,
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Mismatch {}

/// Reports the first failing predicate of a comparison or a check.
///
/// The shared implementation behind `compare_with` and `run_checks`. The
/// failure is returned as a [`Mismatch`] so the caller decides what to do
/// with it; the only side effect is the optional diagnostic line, which
/// `ResultConfig::verbose` controls.
pub(crate) fn report_mismatch(
    config: &ResultConfig,
    kind: MismatchKind,
    predicate: impl Into<String>,
) -> Mismatch {
    let predicate = predicate.into();
    let message = format!("{} failed: {predicate}", kind.label());
    // `panic` is an explicit opt-in for the fail-fast mode; the default
    // configuration reports the mismatch as data.
    assert!(!config.panic, "{message}");
    if config.verbose {
        // A closed stderr must not change the outcome of the evaluation.
        let _ = writeln!(std::io::stderr(), "{message}");
    }
    Mismatch { kind, predicate, message }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mismatch_renders_its_message() {
        let mismatch = Mismatch {
            kind: MismatchKind::Comparison,
            predicate: "Status".to_owned(),
            message: "comparison failed: Status".to_owned(),
        };
        assert_eq!(mismatch.to_string(), "comparison failed: Status");
    }

    #[test]
    fn the_default_config_reports_a_mismatch_instead_of_panicking() {
        let config = ResultConfig::default();
        let mismatch = report_mismatch(&config, MismatchKind::Check, "Success");
        assert_eq!(mismatch.kind, MismatchKind::Check);
        assert_eq!(mismatch.predicate, "Success");
        assert_eq!(mismatch.message, "check failed: Success");
    }
}
