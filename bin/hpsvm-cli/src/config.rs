use std::path::Path;

use hpsvm_fixture::Compare;

use crate::error::CliError;

#[derive(Debug, serde::Deserialize)]
pub(crate) struct CompareConfigFile {
    compares: Vec<Compare>,
}

pub(crate) fn load_compares(
    path: Option<&Path>,
    fallback: &[Compare],
    ignore_compute_units: bool,
) -> Result<Vec<Compare>, CliError> {
    let mut compares = if let Some(path) = path {
        let file = std::fs::read_to_string(path)?;
        match path.extension().and_then(|value| value.to_str()) {
            Some("json") => serde_json::from_str::<CompareConfigFile>(&file)
                .map(|config| config.compares)
                .map_err(|error| CliError::ConfigParse {
                    path: path.display().to_string(),
                    reason: error.to_string(),
                })?,
            _ => return Err(CliError::UnsupportedConfigFormat { path: path.display().to_string() }),
        }
    } else {
        fallback.to_vec()
    };

    if ignore_compute_units {
        compares.retain(|compare| !matches!(compare, Compare::ComputeUnits));
    }

    Ok(compares)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use hpsvm_fixture::Compare;
    use solana_address::Address;

    use super::load_compares;
    use crate::error::CliError;

    fn temp_config_path(extension: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "hpsvm-cli-compare-config-{}.{}",
            Address::new_unique(),
            extension
        ))
    }

    #[test]
    fn load_compares_uses_fallback_and_can_ignore_compute_units() {
        let compares = load_compares(None, &[Compare::Status, Compare::ComputeUnits], true)
            .expect("fallback compares should load");

        assert_eq!(compares, vec![Compare::Status]);
    }

    #[test]
    fn load_compares_reads_json_config() {
        let path = temp_config_path("json");
        std::fs::write(&path, r#"{"compares":["Status","Logs"]}"#)
            .expect("json config should write");

        let compares =
            load_compares(Some(&path), &[Compare::Fee], false).expect("json compares should load");

        assert_eq!(compares, vec![Compare::Status, Compare::Logs]);

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn load_compares_rejects_unsupported_config_format() {
        let path = temp_config_path("yaml");
        std::fs::write(&path, "compares: []").expect("yaml config should write");

        let error = load_compares(Some(&path), &[Compare::Fee], false)
            .expect_err("unsupported extension should fail");

        assert!(matches!(error, CliError::UnsupportedConfigFormat { .. }));

        std::fs::remove_file(path).ok();
    }

    /// An extension-less config path is not `json`, so it takes the same
    /// unsupported-format branch.
    #[test]
    fn load_compares_rejects_an_extensionless_path() {
        let path = std::env::temp_dir().join(format!("hpsvm-cli-cfg-{}", Address::new_unique()));
        std::fs::write(&path, r#"{"compares":[]}"#).expect("config should write");

        let error = load_compares(Some(&path), &[Compare::Fee], false)
            .expect_err("extensionless path should fail");
        assert!(matches!(error, CliError::UnsupportedConfigFormat { .. }));

        std::fs::remove_file(path).ok();
    }

    /// Malformed JSON must surface as `ConfigParse` naming the offending path.
    #[test]
    fn load_compares_reports_malformed_json() {
        let path = temp_config_path("json");
        std::fs::write(&path, "{not json").expect("config should write");

        let error = load_compares(Some(&path), &[Compare::Fee], false)
            .expect_err("malformed json should fail");

        match error {
            CliError::ConfigParse { path: reported, reason } => {
                assert_eq!(reported, path.display().to_string());
                assert!(!reason.is_empty(), "the serde reason must be reported");
            }
            other => panic!("expected CliError::ConfigParse, got {other:?}"),
        }

        std::fs::remove_file(path).ok();
    }

    /// Valid JSON with the wrong shape is also a parse error.
    #[test]
    fn load_compares_reports_a_missing_compares_key() {
        let path = temp_config_path("json");
        std::fs::write(&path, r#"{"not_compares":["Status"]}"#).expect("config should write");

        let error = load_compares(Some(&path), &[Compare::Fee], false)
            .expect_err("missing `compares` should fail");
        assert!(matches!(error, CliError::ConfigParse { .. }));

        std::fs::remove_file(path).ok();
    }

    /// An unknown `Compare` variant name is rejected by serde.
    #[test]
    fn load_compares_rejects_an_unknown_compare_variant() {
        let path = temp_config_path("json");
        std::fs::write(&path, r#"{"compares":["Nonsense"]}"#).expect("config should write");

        let error = load_compares(Some(&path), &[Compare::Fee], false)
            .expect_err("unknown variant should fail");
        assert!(matches!(error, CliError::ConfigParse { .. }));

        std::fs::remove_file(path).ok();
    }

    /// A missing config file must surface as an IO error, not a parse error.
    #[test]
    fn load_compares_reports_a_missing_file_as_an_io_error() {
        let path = temp_config_path("json");
        let error = load_compares(Some(&path), &[Compare::Fee], false)
            .expect_err("missing file should fail");
        assert!(matches!(error, CliError::Io(_)), "expected CliError::Io, got {error:?}");
    }

    /// `--ignore-compute-units` strips `ComputeUnits` from the loaded config too,
    /// not just from the fallback.
    #[test]
    fn load_compares_strips_compute_units_from_a_json_config() {
        let path = temp_config_path("json");
        std::fs::write(&path, r#"{"compares":["Status","ComputeUnits","Logs"]}"#)
            .expect("config should write");

        let compares =
            load_compares(Some(&path), &[Compare::Fee], true).expect("json compares should load");
        assert_eq!(compares, vec![Compare::Status, Compare::Logs]);

        std::fs::remove_file(path).ok();
    }

    /// Every `Compare::ComputeUnits` entry must be removed, not just the first.
    #[test]
    fn load_compares_removes_every_compute_units_entry() {
        let path = temp_config_path("json");
        std::fs::write(&path, r#"{"compares":["ComputeUnits","Status","ComputeUnits"]}"#)
            .expect("config should write");

        let compares =
            load_compares(Some(&path), &[Compare::Fee], true).expect("json compares should load");
        assert_eq!(compares, vec![Compare::Status]);

        std::fs::remove_file(path).ok();
    }

    /// Without the flag, `ComputeUnits` survives untouched.
    #[test]
    fn load_compares_keeps_compute_units_when_not_ignoring_them() {
        let path = temp_config_path("json");
        std::fs::write(&path, r#"{"compares":["ComputeUnits","Status"]}"#)
            .expect("config should write");

        let compares =
            load_compares(Some(&path), &[Compare::Fee], false).expect("json compares should load");
        assert_eq!(compares, vec![Compare::ComputeUnits, Compare::Status]);

        std::fs::remove_file(path).ok();
    }

    /// An empty `compares` array is valid and yields no comparisons.
    #[test]
    fn load_compares_accepts_an_empty_compare_list() {
        let path = temp_config_path("json");
        std::fs::write(&path, r#"{"compares":[]}"#).expect("config should write");

        assert!(load_compares(Some(&path), &[Compare::Fee], false).unwrap().is_empty());

        std::fs::remove_file(path).ok();
    }

    /// The fallback is copied, so mutating the result must not alias the caller's
    /// slice.
    #[test]
    fn load_compares_copies_the_fallback() {
        let fallback = vec![Compare::Status, Compare::ComputeUnits];
        let mut compares =
            load_compares(None, &fallback, false).expect("fallback compares should load");
        compares.clear();

        assert_eq!(fallback, vec![Compare::Status, Compare::ComputeUnits]);
    }

    #[test]
    fn load_compares_of_an_empty_fallback_is_empty() {
        assert!(load_compares(None, &[], false).unwrap().is_empty());
        assert!(load_compares(None, &[], true).unwrap().is_empty());
    }
}
