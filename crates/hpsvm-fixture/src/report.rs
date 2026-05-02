use std::{collections::BTreeMap, path::Path};

use crate::{BenchError, generated_at_string, solana_runtime_version_string};

#[cfg(feature = "markdown")]
const BASELINE_REPORT_FILE_NAME: &str = "cu-report.baseline";

#[cfg(feature = "markdown")]
const BASELINE_REPORT_MAGIC: &[u8] = b"HPSVM-CU-BASELINE-V1\n";

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

#[cfg(feature = "markdown")]
pub(crate) const MARKDOWN_REPORT_FILE_NAME: &str = "cu-report.md";

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

        #[cfg(feature = "markdown")]
        {
            load_baseline_rows(path)
        }

        #[cfg(not(feature = "markdown"))]
        {
            let _ = path;
            Err(BenchError::ReportIoDisabled { operation: "baseline_dir" })
        }
    }

    pub(crate) fn write_to_dir(&self, path: Option<&Path>) -> Result<(), BenchError> {
        let Some(path) = path else {
            return Ok(());
        };

        #[cfg(feature = "markdown")]
        {
            write_report(self, path)
        }

        #[cfg(not(feature = "markdown"))]
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

#[cfg(feature = "markdown")]
fn load_baseline_rows(path: &Path) -> Result<BTreeMap<String, u64>, BenchError> {
    if let Some(rows) = load_sidecar_rows(path)? {
        return Ok(rows);
    }

    load_markdown_rows(path)
}

#[cfg(feature = "markdown")]
fn load_sidecar_rows(path: &Path) -> Result<Option<BTreeMap<String, u64>>, BenchError> {
    let baseline_path = path.join(BASELINE_REPORT_FILE_NAME);
    let bytes = match std::fs::read(&baseline_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(BenchError::ReadBaseline { path: baseline_path, source: error });
        }
    };

    parse_sidecar_rows(&baseline_path, &bytes).map(Some)
}

#[cfg(feature = "markdown")]
fn load_markdown_rows(path: &Path) -> Result<BTreeMap<String, u64>, BenchError> {
    let baseline_path = path.join(MARKDOWN_REPORT_FILE_NAME);
    let markdown = match std::fs::read_to_string(&baseline_path) {
        Ok(markdown) => markdown,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => {
            return Err(BenchError::ReadBaseline { path: baseline_path, source: error });
        }
    };

    parse_baseline_rows(&baseline_path, &markdown)
}

#[cfg(feature = "markdown")]
fn parse_baseline_rows(path: &Path, markdown: &str) -> Result<BTreeMap<String, u64>, BenchError> {
    let mut rows = BTreeMap::new();

    for line in markdown.lines().map(str::trim).filter(|line| line.starts_with('|')) {
        let columns = parse_markdown_columns(path, line)?;
        if columns[0] == "Name" || is_separator_row(&columns) {
            continue;
        }

        let compute_units = columns[1].parse().map_err(|error| BenchError::InvalidBaseline {
            path: path.to_path_buf(),
            reason: format!("failed to parse compute units for `{}`: {error}", columns[0]),
        })?;

        if rows.insert(columns[0].clone(), compute_units).is_some() {
            return Err(BenchError::InvalidBaseline {
                path: path.to_path_buf(),
                reason: format!("duplicate baseline row `{}`", columns[0]),
            });
        }
    }

    Ok(rows)
}

#[cfg(feature = "markdown")]
fn parse_markdown_columns(path: &Path, line: &str) -> Result<Vec<String>, BenchError> {
    let mut columns = Vec::new();
    let mut current = String::new();
    let mut escaped = false;

    for character in line.chars().skip(1) {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }

        match character {
            '\\' => escaped = true,
            '|' => {
                columns.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(character),
        }
    }

    if escaped {
        return Err(BenchError::InvalidBaseline {
            path: path.to_path_buf(),
            reason: String::from("unterminated escape sequence in markdown table row"),
        });
    }

    if !current.trim().is_empty() {
        return Err(BenchError::InvalidBaseline {
            path: path.to_path_buf(),
            reason: format!("markdown table row is missing a trailing separator: {line}"),
        });
    }

    if matches!(columns.last(), Some(last) if last.is_empty()) {
        columns.pop();
    }

    if columns.len() != 4 {
        return Err(BenchError::InvalidBaseline {
            path: path.to_path_buf(),
            reason: format!(
                "expected 4 columns in markdown table row, found {}: {line}",
                columns.len()
            ),
        });
    }

    Ok(columns)
}

#[cfg(feature = "markdown")]
fn is_separator_row(columns: &[String]) -> bool {
    columns
        .iter()
        .all(|column| column.chars().all(|character| matches!(character, '-' | ':' | ' ')))
}

#[cfg(feature = "markdown")]
fn parse_sidecar_rows(path: &Path, bytes: &[u8]) -> Result<BTreeMap<String, u64>, BenchError> {
    if !bytes.starts_with(BASELINE_REPORT_MAGIC) {
        return Err(BenchError::InvalidBaseline {
            path: path.to_path_buf(),
            reason: String::from("missing baseline sidecar header"),
        });
    }

    let mut rows = BTreeMap::new();
    let mut offset = BASELINE_REPORT_MAGIC.len();

    while offset < bytes.len() {
        let name_len_bytes =
            bytes.get(offset..offset + std::mem::size_of::<u32>()).ok_or_else(|| {
                BenchError::InvalidBaseline {
                    path: path.to_path_buf(),
                    reason: String::from("truncated baseline sidecar row header"),
                }
            })?;
        offset += std::mem::size_of::<u32>();
        let name_len = u32::from_le_bytes(name_len_bytes.try_into().unwrap()) as usize;

        let name_bytes =
            bytes.get(offset..offset + name_len).ok_or_else(|| BenchError::InvalidBaseline {
                path: path.to_path_buf(),
                reason: String::from("truncated baseline sidecar row name"),
            })?;
        offset += name_len;

        let compute_units_bytes = bytes
            .get(offset..offset + std::mem::size_of::<u64>())
            .ok_or_else(|| BenchError::InvalidBaseline {
                path: path.to_path_buf(),
                reason: String::from("truncated baseline sidecar compute units value"),
            })?;
        offset += std::mem::size_of::<u64>();

        let name =
            std::str::from_utf8(name_bytes).map_err(|error| BenchError::InvalidBaseline {
                path: path.to_path_buf(),
                reason: format!("invalid utf-8 in baseline sidecar row name: {error}"),
            })?;
        let compute_units = u64::from_le_bytes(compute_units_bytes.try_into().unwrap());

        if rows.insert(String::from(name), compute_units).is_some() {
            return Err(BenchError::InvalidBaseline {
                path: path.to_path_buf(),
                reason: format!("duplicate baseline row `{name}`"),
            });
        }
    }

    Ok(rows)
}

#[cfg(feature = "markdown")]
fn write_report(report: &CuReport, path: &Path) -> Result<(), BenchError> {
    std::fs::create_dir_all(path)
        .map_err(|source| BenchError::CreateOutputDir { path: path.to_path_buf(), source })?;

    let baseline_path = path.join(BASELINE_REPORT_FILE_NAME);
    std::fs::write(&baseline_path, encode_sidecar_rows(report, &baseline_path)?)
        .map_err(|source| BenchError::WriteReport { path: baseline_path, source })?;

    let report_path = path.join(MARKDOWN_REPORT_FILE_NAME);
    std::fs::write(&report_path, report.render_markdown())
        .map_err(|source| BenchError::WriteReport { path: report_path, source })
}

#[cfg(feature = "markdown")]
fn encode_sidecar_rows(report: &CuReport, path: &Path) -> Result<Vec<u8>, BenchError> {
    let mut bytes = Vec::from(BASELINE_REPORT_MAGIC);

    for row in &report.rows {
        let name_bytes = row.name.as_bytes();
        let name_len =
            u32::try_from(name_bytes.len()).map_err(|_| BenchError::InvalidBaseline {
                path: path.to_path_buf(),
                reason: format!(
                    "case name `{}` is too long to write to baseline sidecar",
                    row.name
                ),
            })?;

        bytes.extend_from_slice(&name_len.to_le_bytes());
        bytes.extend_from_slice(name_bytes);
        bytes.extend_from_slice(&row.compute_units.to_le_bytes());
    }

    Ok(bytes)
}
