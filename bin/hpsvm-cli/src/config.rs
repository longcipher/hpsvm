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
            Some("yaml" | "yml") => serde_yaml::from_str::<CompareConfigFile>(&file)
                .map(|config| config.compares)
                .map_err(|error| CliError::ConfigParse {
                    path: path.display().to_string(),
                    reason: error.to_string(),
                })?,
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
    fn load_compares_reads_yaml_config() {
        let path = temp_config_path("yaml");
        std::fs::write(&path, "compares:\n  - Status\n  - ComputeUnits\n")
            .expect("yaml config should write");

        let compares =
            load_compares(Some(&path), &[Compare::Fee], false).expect("yaml compares should load");

        assert_eq!(compares, vec![Compare::Status, Compare::ComputeUnits]);

        std::fs::remove_file(path).ok();
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
        let path = temp_config_path("txt");
        std::fs::write(&path, "compares: []").expect("text config should write");

        let error = load_compares(Some(&path), &[Compare::Fee], false)
            .expect_err("unsupported extension should fail");

        assert!(matches!(error, CliError::UnsupportedConfigFormat { .. }));

        std::fs::remove_file(path).ok();
    }
}
