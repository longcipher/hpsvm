#![allow(missing_debug_implementations, missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

mod bench_error;
#[cfg(feature = "bin-codec")]
mod binary;
mod capture;
mod check;
mod compare;
mod config;
mod error;
#[cfg(feature = "json-codec")]
mod json;
mod matrix;
mod model;
mod report;
mod runner;
mod single;
mod snapshot;

pub use crate::{
    bench_error::BenchError,
    capture::CaptureBuilder,
    check::{AccountExpectation, AccountExpectationBuilder, Check},
    compare::{AccountCompareScope, Compare},
    config::ResultConfig,
    error::FixtureError,
    matrix::{ComputeUnitMatrixBencher, MatrixReport},
    model::{
        Fixture, FixtureExpectations, FixtureFormat, FixtureHeader, FixtureInput, FixtureKind,
        InstructionAccountMeta, InstructionFixture, ProgramBinding, RuntimeFixtureConfig,
        TransactionFixture,
    },
    report::{CuDelta, CuReport, CuReportRow},
    runner::{FixtureExecution, FixtureRunner},
    single::ComputeUnitBencher,
    snapshot::{
        AccountSnapshot, ExecutionSnapshot, ExecutionSnapshotFields, ExecutionStatus,
        InnerInstructionSnapshot, ReturnDataSnapshot,
    },
};

pub type FixtureBenchCase<'a> = (&'a str, &'a Fixture);

const WORKSPACE_MANIFEST: &str = include_str!("../../../Cargo.toml");

pub(crate) const BUILTIN_VARIANT_NAME: &str = "builtin";

pub(crate) fn generated_at_string() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or_else(|_| String::from("0"), |duration| duration.as_secs().to_string())
}

pub(crate) fn solana_runtime_version_string() -> String {
    WORKSPACE_MANIFEST
        .lines()
        .map(str::trim)
        .find_map(|line| {
            line.strip_prefix("agave-feature-set = \"")
                .and_then(|value| value.strip_suffix('"'))
                .map(String::from)
        })
        .unwrap_or_else(|| String::from("unknown"))
}

impl Fixture {
    #[cfg(any(feature = "json-codec", feature = "bin-codec"))]
    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self, FixtureError> {
        let path = path.as_ref();
        match fixture_format_for_path(path)? {
            FixtureFormat::Json => {
                #[cfg(feature = "json-codec")]
                {
                    json::load(path)
                }
                #[cfg(not(feature = "json-codec"))]
                {
                    Err(FixtureError::UnsupportedFormat { path: path.display().to_string() })
                }
            }
            FixtureFormat::Binary => {
                #[cfg(feature = "bin-codec")]
                {
                    binary::load(path)
                }
                #[cfg(not(feature = "bin-codec"))]
                {
                    Err(FixtureError::UnsupportedFormat { path: path.display().to_string() })
                }
            }
        }
    }

    #[cfg(any(feature = "json-codec", feature = "bin-codec"))]
    pub fn save(
        &self,
        path: impl AsRef<std::path::Path>,
        format: FixtureFormat,
    ) -> Result<(), FixtureError> {
        match format {
            FixtureFormat::Json => {
                #[cfg(feature = "json-codec")]
                {
                    json::save(self, path.as_ref())
                }
                #[cfg(not(feature = "json-codec"))]
                {
                    Err(FixtureError::UnsupportedFormat {
                        path: path.as_ref().display().to_string(),
                    })
                }
            }
            FixtureFormat::Binary => {
                #[cfg(feature = "bin-codec")]
                {
                    binary::save(self, path.as_ref())
                }
                #[cfg(not(feature = "bin-codec"))]
                {
                    Err(FixtureError::UnsupportedFormat {
                        path: path.as_ref().display().to_string(),
                    })
                }
            }
        }
    }
}

#[cfg(any(feature = "json-codec", feature = "bin-codec"))]
fn fixture_format_for_path(path: &std::path::Path) -> Result<FixtureFormat, FixtureError> {
    match path.extension().and_then(|value| value.to_str()) {
        Some("json") => Ok(FixtureFormat::Json),
        Some("bin") => Ok(FixtureFormat::Binary),
        _ => Err(FixtureError::UnsupportedFormat { path: path.display().to_string() }),
    }
}
