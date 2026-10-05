use std::{collections::BTreeMap, path::Path};

use crate::{BenchError, generated_at_string, solana_runtime_version_string};

/// Machine-readable baseline sidecar written next to the human-readable report.
///
/// Compute-unit baselines round-trip through this file, never through the
/// Markdown report. Keeping the two concerns separate means the Markdown output
/// is a pure rendering target: it can be restyled freely without breaking
/// baseline comparison, and no reverse parser has to exist.
#[cfg(feature = "report-io")]
pub(crate) const BASELINE_REPORT_FILE_NAME: &str = "cu-report.baseline.json";

/// Human-readable report. Write-only: never parsed back.
#[cfg(feature = "markdown")]
pub(crate) const MARKDOWN_REPORT_FILE_NAME: &str = "cu-report.md";

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct CuReport {
    pub generated_at: String,
    pub solana_runtime_version: String,
    pub rows: Vec<CuReportRow>,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct CuReportRow {
    pub name: String,
    pub compute_units: u64,
    pub delta: Option<CuDelta>,
    pub pass: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct CuDelta {
    pub absolute: i64,
    pub percent: f64,
}

impl CuReport {
    pub(crate) fn new(rows: Vec<CuReportRow>) -> Self {
        Self {
            generated_at: generated_at_string(),
            solana_runtime_version: solana_runtime_version_string(),
            rows,
        }
    }

    #[cfg(feature = "markdown")]
    pub fn render_markdown(&self) -> String {
        let mut markdown = String::from("# Compute Unit Report\n\n");
        markdown.push_str(&format!("Generated at: {}\n", self.generated_at));
        markdown.push_str(&format!("Solana runtime version: {}\n\n", self.solana_runtime_version));
        markdown.push_str(&render_table(self));
        markdown
    }

    pub(crate) fn baseline_map(path: Option<&Path>) -> Result<BTreeMap<String, u64>, BenchError> {
        let Some(path) = path else {
            return Ok(BTreeMap::new());
        };

        #[cfg(feature = "report-io")]
        {
            load_baseline_rows(path)
        }

        #[cfg(not(feature = "report-io"))]
        {
            let _ = path;
            Err(BenchError::ReportIoDisabled { operation: "baseline_dir" })
        }
    }

    pub(crate) fn write_to_dir(&self, path: Option<&Path>) -> Result<(), BenchError> {
        let Some(path) = path else {
            return Ok(());
        };

        #[cfg(feature = "report-io")]
        {
            write_report(self, path)
        }

        #[cfg(not(feature = "report-io"))]
        {
            let _ = path;
            Err(BenchError::ReportIoDisabled { operation: "output_dir" })
        }
    }
}

impl CuDelta {
    pub fn between(baseline: u64, current: u64) -> Self {
        // Both operands come from a baseline sidecar that a user can edit, so the
        // difference is widened through `i128` and saturated. Subtracting
        // `u64 as i64` directly would overflow (and panic, since the workspace
        // builds with `overflow-checks`) for values above `i64::MAX`.
        let absolute = i64::try_from(i128::from(current) - i128::from(baseline))
            .unwrap_or(if current >= baseline { i64::MAX } else { i64::MIN });
        let percent = if baseline == 0 {
            if current == 0 { 0.0 } else { 100.0 }
        } else {
            (absolute as f64 / baseline as f64) * 100.0
        };

        Self { absolute, percent }
    }
}

#[cfg(feature = "markdown")]
pub(crate) fn render_table(report: &CuReport) -> String {
    let mut markdown = String::new();
    markdown.push_str("| Name | Compute Units | Delta | Pass |\n");
    markdown.push_str("| --- | ---: | --- | --- |\n");

    for row in &report.rows {
        markdown.push_str("| ");
        markdown.push_str(&escape_markdown_cell(&row.name));
        markdown.push_str(" | ");
        markdown.push_str(&row.compute_units.to_string());
        markdown.push_str(" | ");
        markdown.push_str(&format_delta(row.delta));
        markdown.push_str(" | ");
        markdown.push_str(if row.pass { "PASS" } else { "FAIL" });
        markdown.push_str(" |\n");
    }

    markdown
}

#[cfg(feature = "markdown")]
fn escape_markdown_cell(value: &str) -> String {
    value.replace('\\', "\\\\").replace('|', "\\|")
}

#[cfg(feature = "markdown")]
fn format_delta(delta: Option<CuDelta>) -> String {
    delta.map_or_else(
        || String::from("n/a"),
        |delta| format!("{:+} ({:+.2}%)", delta.absolute, delta.percent),
    )
}

#[cfg(feature = "report-io")]
fn load_baseline_rows(path: &Path) -> Result<BTreeMap<String, u64>, BenchError> {
    let baseline_path = path.join(BASELINE_REPORT_FILE_NAME);
    let bytes = match std::fs::read(&baseline_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BTreeMap::new());
        }
        Err(error) => {
            return Err(BenchError::ReadBaseline { path: baseline_path, source: error });
        }
    };

    let report: CuReport = serde_json::from_slice(&bytes).map_err(|error| {
        BenchError::InvalidBaseline { path: baseline_path.clone(), reason: error.to_string() }
    })?;

    let mut rows = BTreeMap::new();
    for row in report.rows {
        let name = row.name;
        if rows.insert(name.clone(), row.compute_units).is_some() {
            return Err(BenchError::InvalidBaseline {
                path: baseline_path,
                reason: format!("duplicate baseline row `{name}`"),
            });
        }
    }

    Ok(rows)
}

#[cfg(feature = "report-io")]
fn write_report(report: &CuReport, path: &Path) -> Result<(), BenchError> {
    std::fs::create_dir_all(path)
        .map_err(|source| BenchError::CreateOutputDir { path: path.to_path_buf(), source })?;

    let baseline_path = path.join(BASELINE_REPORT_FILE_NAME);
    let baseline = serde_json::to_vec_pretty(report).map_err(|error| {
        BenchError::InvalidBaseline { path: baseline_path.clone(), reason: error.to_string() }
    })?;
    std::fs::write(&baseline_path, baseline)
        .map_err(|source| BenchError::WriteReport { path: baseline_path, source })?;

    #[cfg(feature = "markdown")]
    {
        let report_path = path.join(MARKDOWN_REPORT_FILE_NAME);
        std::fs::write(&report_path, report.render_markdown())
            .map_err(|source| BenchError::WriteReport { path: report_path, source })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn row(name: &str, compute_units: u64, delta: Option<CuDelta>, pass: bool) -> CuReportRow {
        CuReportRow { name: name.to_string(), compute_units, delta, pass }
    }

    fn scratch_dir(stem: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock is after the unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("hpsvm-{stem}-{}-{unique}", std::process::id()))
    }

    #[test]
    fn cu_delta_handles_zero_baseline() {
        assert_eq!(CuDelta::between(0, 0), CuDelta { absolute: 0, percent: 0.0 });
        assert_eq!(CuDelta::between(0, 10), CuDelta { absolute: 10, percent: 100.0 });
    }

    #[test]
    fn cu_delta_reports_a_negative_change() {
        assert_eq!(CuDelta::between(100, 90), CuDelta { absolute: -10, percent: -10.0 });
    }

    #[test]
    fn cu_delta_percent_is_relative_to_the_baseline() {
        assert_eq!(CuDelta::between(400, 500).absolute, 100);
        assert_eq!(CuDelta::between(400, 500).percent, 25.0);
        assert_eq!(CuDelta::between(200, 300).percent, 50.0);
    }

    // Property: `absolute` is always `current - baseline` (as an `i64`), and
    // `percent` is that difference relative to the baseline.
    proptest! {
        #[test]
        fn cu_delta_follows_its_definition(baseline in any::<u64>(), current in any::<u64>()) {
            let delta = CuDelta::between(baseline, current);

            // `between` widens through `i128` and saturates, so the property
            // compares against the saturated value rather than assuming the
            // difference fits in an `i64`.
            let raw = i128::from(current) - i128::from(baseline);
            let expected_absolute = if raw > i64::MAX as i128 {
                i64::MAX
            } else if raw < i64::MIN as i128 {
                i64::MIN
            } else {
                raw as i64
            };
            prop_assert_eq!(delta.absolute, expected_absolute);
            if baseline == 0 {
                prop_assert_eq!(delta.percent, if current == 0 { 0.0 } else { 100.0 });
            } else {
                let expected = (expected_absolute as f64 / baseline as f64) * 100.0;
                prop_assert!((delta.percent - expected).abs() < f64::EPSILON * expected.abs().max(1.0));
            }
        }
    }

    #[cfg(feature = "markdown")]
    #[test]
    fn render_markdown_includes_the_header_metadata_and_rows() {
        let report = CuReport {
            generated_at: String::from("2024-01-01T00:00:00Z"),
            solana_runtime_version: String::from("4.3.0"),
            rows: vec![
                row("alpha", 1234, None, true),
                row("beta", 42, Some(CuDelta::between(40, 42)), false),
            ],
        };

        let markdown = report.render_markdown();

        assert!(markdown.starts_with("# Compute Unit Report\n"));
        assert!(markdown.contains("Generated at: 2024-01-01T00:00:00Z"));
        assert!(markdown.contains("Solana runtime version: 4.3.0"));
        assert!(markdown.contains("| Name | Compute Units | Delta | Pass |"));
        assert!(markdown.contains("| alpha | 1234 | n/a | PASS |"));
        assert!(markdown.contains("| beta | 42 | +2 (+5.00%) | FAIL |"));
    }

    #[cfg(feature = "markdown")]
    #[test]
    fn render_markdown_of_an_empty_report_still_has_a_table_header() {
        let report = CuReport {
            generated_at: String::from("now"),
            solana_runtime_version: String::from("unknown"),
            rows: Vec::new(),
        };

        let markdown = report.render_markdown();

        assert!(markdown.contains("| Name | Compute Units | Delta | Pass |"));
        assert!(markdown.contains("| --- | ---: | --- | --- |"));
        // No data rows.
        assert!(!markdown.contains("PASS"));
        assert!(!markdown.contains("FAIL"));
    }

    #[cfg(feature = "markdown")]
    #[test]
    fn render_table_escapes_backslashes_and_pipes_in_names() {
        let report = CuReport {
            generated_at: String::new(),
            solana_runtime_version: String::new(),
            rows: vec![row("a|b\\c", 1, None, true)],
        };

        let table = render_table(&report);

        assert!(table.contains("| a\\|b\\\\c | 1 | n/a | PASS |"), "got {table}");
        // Exactly one row of cells, i.e. the raw pipe was escaped rather than
        // producing an extra column.
        assert_eq!(table.matches("\n").count(), 3);
    }

    #[cfg(feature = "markdown")]
    #[test]
    fn escape_markdown_cell_only_touches_backslash_and_pipe() {
        assert_eq!(escape_markdown_cell("plain"), "plain");
        assert_eq!(escape_markdown_cell("a|b"), "a\\|b");
        assert_eq!(escape_markdown_cell("a\\b"), "a\\\\b");
        assert_eq!(escape_markdown_cell("a\\|b"), "a\\\\\\|b");
        assert_eq!(escape_markdown_cell("*bold* _em_ `code`"), "*bold* _em_ `code`");
    }

    #[cfg(feature = "markdown")]
    #[test]
    fn format_delta_renders_signed_absolute_and_percent_or_n_a() {
        assert_eq!(format_delta(None), "n/a");
        assert_eq!(format_delta(Some(CuDelta::between(100, 90))), "-10 (-10.00%)");
        assert_eq!(format_delta(Some(CuDelta::between(100, 110))), "+10 (+10.00%)");
        assert_eq!(format_delta(Some(CuDelta::between(100, 100))), "+0 (+0.00%)");
        assert_eq!(format_delta(Some(CuDelta::between(3, 1))), "-2 (-66.67%)");
    }

    #[cfg(feature = "report-io")]
    #[test]
    fn baseline_round_trips_through_json_sidecar() {
        let dir = scratch_dir("report-roundtrip");

        let report = CuReport::new(vec![
            CuReportRow {
                name: String::from("alpha | with pipe"),
                compute_units: 1234,
                delta: None,
                pass: true,
            },
            CuReportRow {
                name: String::from("beta"),
                compute_units: 42,
                delta: Some(CuDelta::between(40, 42)),
                pass: true,
            },
        ]);

        report.write_to_dir(Some(&dir)).expect("report should write");
        let baseline = CuReport::baseline_map(Some(&dir)).expect("baseline should load");

        assert_eq!(baseline.get("alpha | with pipe").copied(), Some(1234));
        assert_eq!(baseline.get("beta").copied(), Some(42));

        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(feature = "report-io")]
    #[test]
    fn missing_baseline_directory_yields_empty_map() {
        let dir = std::env::temp_dir().join("hpsvm-report-does-not-exist");
        assert!(
            CuReport::baseline_map(Some(&dir))
                .expect("missing baseline is not an error")
                .is_empty()
        );
    }

    /// Two rows with the same name make the baseline ambiguous, so it must be
    /// rejected rather than silently keeping the last one.
    #[cfg(feature = "report-io")]
    #[test]
    fn duplicate_baseline_rows_are_rejected() {
        let dir = scratch_dir("report-duplicate");
        std::fs::create_dir_all(&dir).unwrap();
        let report = CuReport::new(vec![row("dup", 1, None, true), row("dup", 2, None, true)]);
        std::fs::write(
            dir.join(BASELINE_REPORT_FILE_NAME),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();

        let error = CuReport::baseline_map(Some(&dir)).expect_err("duplicate rows should fail");
        match error {
            BenchError::InvalidBaseline { reason, .. } => {
                assert!(reason.contains("duplicate baseline row"), "got {reason}");
                assert!(reason.contains("dup"), "got {reason}");
            }
            other => panic!("expected BenchError::InvalidBaseline, got {other:?}"),
        }

        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(feature = "report-io")]
    #[test]
    fn malformed_baseline_json_is_rejected() {
        let dir = scratch_dir("report-malformed");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(BASELINE_REPORT_FILE_NAME), b"{ not json").unwrap();

        let error = CuReport::baseline_map(Some(&dir)).expect_err("malformed json should fail");
        assert!(matches!(error, BenchError::InvalidBaseline { .. }), "got {error:?}");

        std::fs::remove_dir_all(dir).ok();
    }

    /// A baseline file that is valid JSON but the wrong shape is also invalid.
    #[cfg(feature = "report-io")]
    #[test]
    fn a_baseline_with_the_wrong_shape_is_rejected() {
        let dir = scratch_dir("report-wrong-shape");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(BASELINE_REPORT_FILE_NAME), br#"{"unexpected": true}"#).unwrap();

        let error = CuReport::baseline_map(Some(&dir)).expect_err("wrong shape should fail");
        assert!(matches!(error, BenchError::InvalidBaseline { .. }), "got {error:?}");

        std::fs::remove_dir_all(dir).ok();
    }

    /// An unreadable-but-present baseline is an IO error, not an empty map.
    #[cfg(feature = "report-io")]
    #[test]
    fn an_unreadable_baseline_is_an_io_error() {
        // A directory where the baseline file is expected makes `read` fail with
        // a kind other than NotFound.
        let dir = scratch_dir("report-unreadable");
        std::fs::create_dir_all(dir.join(BASELINE_REPORT_FILE_NAME)).unwrap();

        let error =
            CuReport::baseline_map(Some(&dir)).expect_err("unreadable baseline should fail");
        match error {
            BenchError::ReadBaseline { path, .. } => {
                assert_eq!(path, dir.join(BASELINE_REPORT_FILE_NAME));
            }
            other => panic!("expected BenchError::ReadBaseline, got {other:?}"),
        }

        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(feature = "report-io")]
    #[test]
    fn an_empty_baseline_file_yields_an_empty_map() {
        let dir = scratch_dir("report-empty");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(BASELINE_REPORT_FILE_NAME),
            serde_json::to_vec_pretty(&CuReport::new(Vec::new())).unwrap(),
        )
        .unwrap();

        assert!(CuReport::baseline_map(Some(&dir)).unwrap().is_empty());

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn baseline_map_of_no_path_is_empty() {
        assert!(CuReport::baseline_map(None).unwrap().is_empty());
    }

    #[test]
    fn write_to_dir_of_no_path_is_ok() {
        let report = CuReport::new(vec![row("a", 1, None, true)]);
        assert!(report.write_to_dir(None).is_ok());
    }

    /// Writing creates the directory tree, so a nested path works.
    #[cfg(feature = "report-io")]
    #[test]
    fn write_to_dir_creates_missing_directories() {
        let dir = scratch_dir("report-nested").join("a").join("b");
        let report = CuReport::new(vec![row("a", 1, None, true)]);

        report.write_to_dir(Some(&dir)).expect("nested write should succeed");

        assert!(dir.join(BASELINE_REPORT_FILE_NAME).exists());
        std::fs::remove_dir_all(scratch_dir("report-nested")).ok();
    }

    /// A path blocked by an existing regular file cannot be turned into a
    /// directory, so `create_dir_all` must surface `CreateOutputDir`.
    #[cfg(feature = "report-io")]
    #[test]
    fn write_to_dir_reports_a_blocked_output_directory() {
        let dir = scratch_dir("report-blocked");
        std::fs::create_dir_all(&dir).unwrap();
        let blocked = dir.join("blocked");
        std::fs::write(&blocked, b"i am a file").unwrap();

        let error = CuReport::new(Vec::new())
            .write_to_dir(Some(&blocked))
            .expect_err("a file cannot become a directory");

        match error {
            BenchError::CreateOutputDir { path, .. } => assert_eq!(path, blocked),
            other => panic!("expected BenchError::CreateOutputDir, got {other:?}"),
        }

        std::fs::remove_dir_all(dir).ok();
    }

    /// When the baseline file path is a directory, the write fails with
    /// `WriteReport`.
    #[cfg(feature = "report-io")]
    #[test]
    fn write_to_dir_reports_an_unwritable_baseline() {
        let dir = scratch_dir("report-unwritable");
        std::fs::create_dir_all(dir.join(BASELINE_REPORT_FILE_NAME)).unwrap();

        let error =
            CuReport::new(Vec::new()).write_to_dir(Some(&dir)).expect_err("write should fail");

        match error {
            BenchError::WriteReport { path, .. } => {
                assert_eq!(path, dir.join(BASELINE_REPORT_FILE_NAME));
            }
            other => panic!("expected BenchError::WriteReport, got {other:?}"),
        }

        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(all(feature = "report-io", feature = "markdown"))]
    #[test]
    fn write_to_dir_also_writes_the_markdown_report() {
        let dir = scratch_dir("report-markdown");
        let report = CuReport::new(vec![row("case", 7, None, true)]);

        report.write_to_dir(Some(&dir)).expect("report should write");

        let markdown = std::fs::read_to_string(dir.join(MARKDOWN_REPORT_FILE_NAME))
            .expect("markdown report should exist");
        assert!(markdown.contains("| case | 7 | n/a | PASS |"));
        assert!(markdown.contains("# Compute Unit Report"));

        std::fs::remove_dir_all(dir).ok();
    }

    // Property: any single row round-trips through the baseline sidecar.
    #[cfg(feature = "report-io")]
    proptest! {
        #[test]
        fn any_row_round_trips_through_the_baseline_sidecar(
            name in "[ -~]{0,64}",
            compute_units in any::<u64>(),
            pass in any::<bool>(),
        ) {
            let dir = scratch_dir("report-property");
            let report = CuReport::new(vec![row(&name, compute_units, None, pass)]);
            report.write_to_dir(Some(&dir)).expect("report should write");

            let baseline = CuReport::baseline_map(Some(&dir)).expect("baseline should load");

            prop_assert_eq!(baseline.len(), 1);
            prop_assert_eq!(baseline.get(&name).copied(), Some(compute_units));

            std::fs::remove_dir_all(dir).ok();
        }
    }
}
