#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class BenchmarkMetric:
    mean_ns: float
    median_ns: float
    slope_ns: float | None


@dataclass(frozen=True)
class Report:
    name: str
    benchmarks: dict[str, BenchmarkMetric]


def format_duration(ns: float) -> str:
    absolute = abs(ns)
    if absolute >= 1_000_000_000:
        return f"{ns / 1_000_000_000:.2f} s"
    if absolute >= 1_000_000:
        return f"{ns / 1_000_000:.2f} ms"
    if absolute >= 1_000:
        return f"{ns / 1_000:.2f} µs"
    return f"{ns:.0f} ns"


def format_delta(baseline: float, current: float) -> str:
    if baseline == 0:
        return "n/a"

    delta_pct = ((current - baseline) / baseline) * 100.0
    sign = "+" if delta_pct >= 0 else ""
    return f"{sign}{delta_pct:.2f}%"


def load_report(path: Path) -> Report:
    payload = json.loads(path.read_text())
    benchmarks = {
        entry["name"]: BenchmarkMetric(
            mean_ns=float(entry["mean_ns"]),
            median_ns=float(entry["median_ns"]),
            slope_ns=None if entry.get("slope_ns") is None else float(entry["slope_ns"]),
        )
        for entry in payload.get("benchmarks", [])
    }
    return Report(name=path.stem, benchmarks=benchmarks)


def build_summary(baseline: Report, current: Report, limit: int | None) -> str:
    common_names = sorted(
        set(baseline.benchmarks).intersection(current.benchmarks),
        key=lambda name: max(
            baseline.benchmarks[name].mean_ns,
            current.benchmarks[name].mean_ns,
        ),
        reverse=True,
    )

    if limit is not None:
        common_names = common_names[:limit]

    lines = [
        f"## {current.name}",
        "",
        "| Benchmark | Baseline mean | Current mean | Delta | Baseline median | Current median | Median delta |",
        "| --- | --- | --- | --- | --- | --- | --- |",
    ]

    if not common_names:
        lines.append("| no overlapping benchmarks | n/a | n/a | n/a | n/a | n/a | n/a |")
        return "\n".join(lines)

    for name in common_names:
        baseline_metric = baseline.benchmarks[name]
        current_metric = current.benchmarks[name]
        lines.append(
            "| {name} | {baseline_mean} | {current_mean} | {mean_delta} | {baseline_median} | {current_median} | {median_delta} |".format(
                name=name,
                baseline_mean=format_duration(baseline_metric.mean_ns),
                current_mean=format_duration(current_metric.mean_ns),
                mean_delta=format_delta(baseline_metric.mean_ns, current_metric.mean_ns),
                baseline_median=format_duration(baseline_metric.median_ns),
                current_median=format_duration(current_metric.median_ns),
                median_delta=format_delta(baseline_metric.median_ns, current_metric.median_ns),
            )
        )

    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description="Compare exported Criterion benchmark reports")
    parser.add_argument("baseline_dir", type=Path)
    parser.add_argument("current_dir", type=Path)
    parser.add_argument("--limit", type=int)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()

    baseline_paths = {path.name: path for path in sorted(args.baseline_dir.glob("*.json"))}
    current_paths = {path.name: path for path in sorted(args.current_dir.glob("*.json"))}

    common_files = sorted(set(baseline_paths).intersection(current_paths))
    if not common_files:
        summary = "# Criterion Regression Summary\n\nNo overlapping benchmark reports were found.\n"
    else:
        sections = ["# Criterion Regression Summary", ""]
        for filename in common_files:
            baseline_report = load_report(baseline_paths[filename])
            current_report = load_report(current_paths[filename])
            sections.append(build_summary(baseline_report, current_report, args.limit))
            sections.append("")
        summary = "\n".join(sections).rstrip() + "\n"

    if args.output is not None:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(summary)
    else:
        sys.stdout.write(summary)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())