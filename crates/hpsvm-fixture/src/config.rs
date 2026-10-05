#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ResultConfig {
    pub panic: bool,
    pub verbose: bool,
}

impl Default for ResultConfig {
    fn default() -> Self {
        Self { panic: true, verbose: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_panics_on_failure_and_is_quiet() {
        let config = ResultConfig::default();
        assert!(config.panic, "the default must be fail-fast");
        assert!(!config.verbose, "the default must not print diagnostics");
    }

    #[cfg(feature = "json-codec")]
    #[test]
    fn result_config_round_trips_through_serde() {
        let config = ResultConfig { panic: false, verbose: true };
        let json = serde_json::to_string(&config).unwrap();
        let decoded: ResultConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, config);
    }
}
