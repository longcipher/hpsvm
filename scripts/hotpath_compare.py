#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path

NANOSECONDS = {
    "ns": 1.0,
    "us": 1_000.0,
    "µs": 1_000.0,
    "ms": 1_000_000.0,
    "s": 1_000_000_000.0,
}


@dataclass(frozen=True)
class FunctionMetric:
    calls: int
    avg_ns: float
    total_ns: float
    percent_total: float


@dataclass(frozen=True)
class Report:
    name: str
    total_elapsed_ns: int
    functions: dict[str, FunctionMetric]


def parse_duration(value: str) -> float:
    match = re.fullmatch(r"([0-9]+(?:\.[0-9]+)?)\s*(ns|us|µs|ms|s)", value.strip())
    if match is None:
        raise ValueError(f"unsupported duration format: {value!r}")

    magnitude = float(match.group(1))
    unit = match.group(2)
    return magnitude * NANOSECONDS[unit]


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
    timing = payload.get("functions_timing", {})
    functions: dict[str, FunctionMetric] = {}
    for entry in timing.get("data", []):
        functions[entry["name"]] = FunctionMetric(
            calls=int(entry["calls"]),
            avg_ns=parse_duration(entry["avg"]),
            total_ns=parse_duration(entry["total"]),
            percent_total=float(str(entry["percent_total"]).rstrip("%")),
        )

    return Report(
        name=path.stem,
        total_elapsed_ns=int(timing.get("total_elapsed_ns", 0)),
        functions=functions,
    )


def build_summary(baseline: Report, current: Report, limit: int) -> str:
    common_names = sorted(
        set(baseline.functions).intersection(current.functions),
        key=lambda name: max(
            baseline.functions[name].total_ns,
            current.functions[name].total_ns,
        ),
        reverse=True,
    )

    lines = [f"## {current.name}", "", "| Metric | Baseline | Current | Delta |", "| --- | --- | --- | --- |"]
    lines.append(
        f"| total elapsed | {format_duration(baseline.total_elapsed_ns)} | {format_duration(current.total_elapsed_ns)} | {format_delta(baseline.total_elapsed_ns, current.total_elapsed_ns)} |"
    )

    for name in common_names[:limit]:
        baseline_metric = baseline.functions[name]
        current_metric = current.functions[name]
        lines.append(
            f"| {name} avg | {format_duration(baseline_metric.avg_ns)} | {format_duration(current_metric.avg_ns)} | {format_delta(baseline_metric.avg_ns, current_metric.avg_ns)} |"
        )
        lines.append(
            f"| {name} total | {format_duration(baseline_metric.total_ns)} | {format_duration(current_metric.total_ns)} | {format_delta(baseline_metric.total_ns, current_metric.total_ns)} |"
        )

    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description="Compare hotpath benchmark reports")
    parser.add_argument("baseline_dir", type=Path)
    parser.add_argument("current_dir", type=Path)
    parser.add_argument("--limit", type=int, default=8)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()

    baseline_paths = {path.name: path for path in sorted(args.baseline_dir.glob("*.json"))}
    current_paths = {path.name: path for path in sorted(args.current_dir.glob("*.json"))}

    common_files = sorted(set(baseline_paths).intersection(current_paths))
    if not common_files:
        summary = "# Hotpath Regression Summary\n\nNo overlapping benchmark reports were found."
    else:
        sections = ["# Hotpath Regression Summary", ""]
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