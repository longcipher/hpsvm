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
        let absolute = current as i64 - baseline as i64;
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
    use super::*;

    #[test]
    fn cu_delta_handles_zero_baseline() {
        assert_eq!(CuDelta::between(0, 0), CuDelta { absolute: 0, percent: 0.0 });
        assert_eq!(CuDelta::between(0, 10), CuDelta { absolute: 10, percent: 100.0 });
    }

    #[cfg(feature = "report-io")]
    #[test]
    fn baseline_round_trips_through_json_sidecar() {
        let dir = std::env::temp_dir().join(format!(
            "hpsvm-report-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock is after the unix epoch")
                .as_nanos()
        ));

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
}
