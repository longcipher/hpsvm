/// Controls how a failed comparison or check is reported.
///
/// `panic` is an explicit opt-in for the fail-fast mode: when it is enabled,
/// a mismatch aborts the process with the failure message. It is **off by
/// default**, because a library must never abort its caller's process
/// because a fixture did not match. `verbose` prints the failure to stderr.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ResultConfig {
    /// Abort the process on a mismatch instead of reporting it.
    pub panic: bool,
    /// Print the failure to stderr.
    pub verbose: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_reports_failures_and_is_quiet() {
        let config = ResultConfig::default();
        assert!(!config.panic, "the default must not abort the caller's process");
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
