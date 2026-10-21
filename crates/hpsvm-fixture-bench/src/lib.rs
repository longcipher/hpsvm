//! Compute-unit benchmarking and reporting for `hpsvm` fixtures.
//!
//! This crate owns the benchmark half of the fixture tooling: it replays a
//! fixture, measures the compute units it consumes, and renders a report
//! (Markdown plus a JSON baseline sidecar) that can be diffed against a
//! previous run.
//!
//! It is a separate crate from [`hpsvm_fixture`] so the core fixture model
//! and its codecs stay free of the reporting machinery, and so a consumer
//! that only records and replays fixtures never compiles the benching code.

#![deny(rustdoc::broken_intra_doc_links)]

mod bench_error;
mod matrix;
mod report;
mod single;

pub use crate::{
    bench_error::BenchError,
    matrix::{ComputeUnitMatrixBencher, MatrixReport},
    report::{CuDelta, CuReport, CuReportRow},
    single::ComputeUnitBencher,
};

/// A benchmark case: a name and the fixture to replay.
pub type FixtureBenchCase<'a> = (&'a str, &'a hpsvm_fixture::Fixture);

/// The name the built-in (unmodified) program variant is reported under.
pub const BUILTIN_VARIANT_NAME: &str = "builtin";

pub(crate) fn generated_at_string() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or_else(|_| String::from("0"), |duration| duration.as_secs().to_string())
}

pub(crate) fn solana_runtime_version_string() -> String {
    option_env!("HPSVM_AGAVE_FEATURE_SET_VERSION").unwrap_or("unknown").to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solana_runtime_version_string_is_known() {
        assert_ne!(solana_runtime_version_string(), "unknown");
    }

    #[test]
    fn generated_at_string_is_a_decimal_seconds_count() {
        let generated = generated_at_string();
        assert!(!generated.is_empty());
        assert!(generated.chars().all(|c| c.is_ascii_digit()), "got {generated:?}");
    }
}
