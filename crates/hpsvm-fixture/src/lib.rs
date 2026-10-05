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
#[cfg(feature = "fd-codec")]
mod fd;
#[cfg(feature = "json-codec")]
mod json;
mod matrix;
mod model;
mod report;
mod runner;
mod single;
mod snapshot;

#[cfg(feature = "fd-codec")]
pub use crate::fd::{AdapterError, FiredancerFixture};
#[cfg(feature = "instruction-fixture")]
pub use crate::model::{InstructionAccountMeta, InstructionFixture};
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
        ProgramBinding, RuntimeFixtureConfig, TransactionFixture,
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

pub(crate) const BUILTIN_VARIANT_NAME: &str = "builtin";

pub(crate) fn generated_at_string() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or_else(|_| String::from("0"), |duration| duration.as_secs().to_string())
}

pub(crate) fn solana_runtime_version_string() -> String {
    option_env!("HPSVM_AGAVE_FEATURE_SET_VERSION").unwrap_or("unknown").to_owned()
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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use solana_address::Address;

    use super::*;

    fn scratch_path(stem: &str, extension: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "hpsvm-lib-{}-{}.{}",
            stem,
            Address::new_unique(),
            extension
        ))
    }

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

    #[cfg(any(feature = "json-codec", feature = "bin-codec"))]
    #[test]
    fn fixture_format_for_path_maps_extensions() {
        assert_eq!(fixture_format_for_path(Path::new("a.json")).unwrap(), FixtureFormat::Json);
        assert_eq!(fixture_format_for_path(Path::new("a.bin")).unwrap(), FixtureFormat::Binary);
    }

    /// An unrecognised extension must be rejected instead of guessing a codec.
    #[cfg(any(feature = "json-codec", feature = "bin-codec"))]
    #[test]
    fn fixture_format_for_path_rejects_unknown_extensions() {
        for name in ["a.txt", "a.fix", "a", "a.", "a.JSON", "a.Bin"] {
            let path = Path::new(name);
            let error = fixture_format_for_path(path).expect_err("unknown extension should fail");
            match error {
                FixtureError::UnsupportedFormat { path: reported } => {
                    assert_eq!(reported, name);
                }
                other => panic!("expected FixtureError::UnsupportedFormat, got {other:?}"),
            }
        }
    }

    /// `load` dispatches on the extension, so an unknown one fails before any
    /// file IO happens.
    #[cfg(any(feature = "json-codec", feature = "bin-codec"))]
    #[test]
    fn load_rejects_an_unknown_extension() {
        let path = scratch_path("load", "txt");
        std::fs::write(&path, b"whatever").unwrap();

        let error = Fixture::load(&path).expect_err("unknown extension should fail");
        assert!(matches!(error, FixtureError::UnsupportedFormat { .. }), "got {error:?}");

        std::fs::remove_file(path).ok();
    }

    #[cfg(any(feature = "json-codec", feature = "bin-codec"))]
    #[test]
    fn load_reports_a_missing_file_as_an_io_error() {
        let path = scratch_path("load-missing", "json");
        let error = Fixture::load(&path).expect_err("missing file should fail");
        assert!(matches!(error, FixtureError::Io(_)), "got {error:?}");
    }

    #[cfg(feature = "json-codec")]
    #[test]
    fn load_reports_malformed_json() {
        let path = scratch_path("load-malformed", "json");
        std::fs::write(&path, b"{ not json").unwrap();

        let error = Fixture::load(&path).expect_err("malformed json should fail");
        assert!(matches!(error, FixtureError::Json(_)), "got {error:?}");

        std::fs::remove_file(path).ok();
    }

    #[cfg(feature = "bin-codec")]
    #[test]
    fn load_reports_an_undecodable_binary_fixture() {
        let path = scratch_path("load-binary", "bin");
        std::fs::write(&path, b"\xff\xff\xff\xff not a wincode fixture").unwrap();

        let error = Fixture::load(&path).expect_err("corrupt bytes should fail");
        assert!(matches!(error, FixtureError::DecodeFixture(_)), "got {error:?}");

        std::fs::remove_file(path).ok();
    }

    #[cfg(feature = "bin-codec")]
    #[test]
    fn load_reports_an_empty_binary_fixture() {
        let path = scratch_path("load-empty", "bin");
        std::fs::write(&path, b"").unwrap();

        let error = Fixture::load(&path).expect_err("empty bytes should fail");
        assert!(matches!(error, FixtureError::DecodeFixture(_)), "got {error:?}");

        std::fs::remove_file(path).ok();
    }

    #[cfg(feature = "json-codec")]
    #[test]
    fn save_reports_an_unwritable_output_path() {
        // A missing parent directory makes the write fail with an IO error.
        let path = scratch_path("save", "json").join("missing-dir").join("out.json");
        let fixture = minimal_fixture();

        let error =
            fixture.save(&path, FixtureFormat::Json).expect_err("unwritable path should fail");
        assert!(matches!(error, FixtureError::Io(_)), "got {error:?}");
    }

    #[cfg(all(feature = "json-codec", feature = "bin-codec"))]
    #[test]
    fn a_fixture_round_trips_through_both_codecs() {
        let fixture = minimal_fixture();
        let json_path = scratch_path("roundtrip", "json");
        let binary_path = scratch_path("roundtrip", "bin");

        fixture.save(&json_path, FixtureFormat::Json).expect("json save should succeed");
        fixture.save(&binary_path, FixtureFormat::Binary).expect("binary save should succeed");

        let from_json = Fixture::load(&json_path).expect("json load should succeed");
        let from_binary = Fixture::load(&binary_path).expect("binary load should succeed");

        assert_eq!(from_json, fixture);
        assert_eq!(from_binary, fixture);
        assert_eq!(from_json, from_binary);

        std::fs::remove_file(json_path).ok();
        std::fs::remove_file(binary_path).ok();
    }

    /// An empty fixture (no programs, no pre-accounts, empty transaction bytes)
    /// must still round-trip, which pins the codec's handling of empty
    /// sequences.
    #[cfg(all(feature = "json-codec", feature = "bin-codec"))]
    #[test]
    fn an_empty_fixture_round_trips() {
        let fixture = Fixture::new(
            FixtureHeader::new("empty", FixtureKind::Transaction),
            FixtureInput::Transaction(TransactionFixture::new(
                RuntimeFixtureConfig::new(0, None, false, false),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )),
            FixtureExpectations::new(empty_snapshot(), Vec::new()),
        );
        let path = scratch_path("empty", "bin");

        fixture.save(&path, FixtureFormat::Binary).expect("binary save should succeed");
        assert_eq!(Fixture::load(&path).expect("binary load should succeed"), fixture);

        std::fs::remove_file(path).ok();
    }

    fn empty_snapshot() -> ExecutionSnapshot {
        ExecutionSnapshot::from_fields(ExecutionSnapshotFields {
            status: ExecutionStatus::Success,
            included: true,
            compute_units_consumed: 0,
            fee: 0,
            logs: Vec::new(),
            return_data: None,
            inner_instructions: Vec::new(),
            post_accounts: Vec::new(),
        })
    }

    #[cfg(all(feature = "json-codec", feature = "bin-codec"))]
    fn minimal_fixture() -> Fixture {
        Fixture::new(
            FixtureHeader::new("minimal", FixtureKind::Transaction),
            FixtureInput::Transaction(TransactionFixture::new(
                RuntimeFixtureConfig::new(7, Some(1024), true, false),
                Vec::new(),
                vec![AccountSnapshot::new(
                    Address::new_unique(),
                    12,
                    Address::new_unique(),
                    false,
                    0,
                    vec![1, 2, 3],
                )],
                vec![4, 5, 6, 7],
            )),
            FixtureExpectations::new(empty_snapshot(), Compare::everything()),
        )
    }

    #[cfg(feature = "json-codec")]
    #[test]
    fn runtime_config_compute_unit_limit_defaults_when_absent() {
        // `compute_unit_limit` is `#[serde(default)]`, so an old baseline without
        // the field must still deserialize.
        let json = r#"{"slot":1,"log_bytes_limit":null,"sigverify":true,"blockhash_check":false}"#;
        let config: RuntimeFixtureConfig = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(config.compute_unit_limit, None);
        assert_eq!(config.slot, 1);
        assert!(config.sigverify);
        assert!(!config.blockhash_check);
    }

    #[test]
    fn runtime_config_new_leaves_the_compute_unit_limit_unset() {
        let config = RuntimeFixtureConfig::new(3, None, false, true);
        assert_eq!(config.slot, 3);
        assert_eq!(config.log_bytes_limit, None);
        assert!(!config.sigverify);
        assert!(config.blockhash_check);
        assert_eq!(config.compute_unit_limit, None);
    }

    #[test]
    fn runtime_config_with_compute_unit_limit_overrides_any_previous_value() {
        let config = RuntimeFixtureConfig::new(0, None, true, false).with_compute_unit_limit(1_000);
        assert_eq!(config.compute_unit_limit, Some(1_000));
        assert_eq!(config.with_compute_unit_limit(2_000).compute_unit_limit, Some(2_000));
    }

    #[test]
    fn program_binding_stores_the_role_when_given() {
        let program_id = Address::new_unique();
        let loader_id = Address::new_unique();
        assert_eq!(
            ProgramBinding::new(program_id, loader_id, None),
            ProgramBinding { program_id, loader_id, role: None }
        );
        assert_eq!(
            ProgramBinding::new(program_id, loader_id, Some(String::from("app"))).role,
            Some(String::from("app"))
        );
    }

    #[test]
    fn fixture_header_starts_at_schema_version_one_without_source_or_tags() {
        let header = FixtureHeader::new("name", FixtureKind::Transaction);
        assert_eq!(header.schema_version, 1);
        assert_eq!(header.name, "name");
        assert_eq!(header.source, None);
        assert!(header.tags.is_empty());
    }

    #[test]
    fn fixture_header_source_and_tags_accumulate() {
        let header = FixtureHeader::new("name", FixtureKind::Transaction)
            .source("agave-test-validator")
            .tag("alpha")
            .tag("beta");
        assert_eq!(header.source, Some(String::from("agave-test-validator")));
        assert_eq!(header.tags, vec![String::from("alpha"), String::from("beta")]);
    }

    #[test]
    fn fixture_expectations_store_the_baseline_and_compares() {
        let baseline = empty_snapshot();
        let expectations = FixtureExpectations::new(baseline.clone(), Compare::everything());
        assert_eq!(expectations.baseline, baseline);
        assert_eq!(expectations.compares, Compare::everything());
    }
}
