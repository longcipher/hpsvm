#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
from collections import defaultdict
from pathlib import Path


def normalize_group_name(name: str) -> str:
    if name.endswith("_comparison"):
        return name[: -len("_comparison")]
    return name


def load_estimates(path: Path) -> dict[str, float | None]:
    payload = json.loads(path.read_text())

    def point_estimate(key: str) -> float | None:
        entry = payload.get(key)
        if entry is None:
            return None
        return float(entry["point_estimate"])

    return {
        "mean_ns": point_estimate("mean"),
        "median_ns": point_estimate("median"),
        "slope_ns": point_estimate("slope"),
        "std_dev_ns": point_estimate("std_dev"),
    }


def collect_reports(root: Path, includes: set[str] | None) -> dict[str, list[dict[str, float | str | None]]]:
    reports: dict[str, list[dict[str, float | str | None]]] = defaultdict(list)

    for path in sorted(root.glob("**/new/estimates.json")):
        relative = path.relative_to(root)
        parts = relative.parts
        if len(parts) < 3 or parts[-2] != "new":
            continue

        benchmark_path = parts[:-2]
        group_name = normalize_group_name(benchmark_path[0])
        if includes is not None and group_name not in includes:
            continue

        benchmark_name = "/".join(benchmark_path[1:]) if len(benchmark_path) > 1 else group_name
        report = {"name": benchmark_name}
        report.update(load_estimates(path))
        reports[group_name].append(report)

    return reports


def write_reports(reports: dict[str, list[dict[str, float | str | None]]], output_dir: Path) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)

    for group_name, benchmarks in sorted(reports.items()):
        benchmarks.sort(
            key=lambda benchmark: (
                -(benchmark.get("mean_ns") or 0.0),
                str(benchmark["name"]),
            )
        )
        payload = {
            "type": "criterion_report",
            "benchmark_group": group_name,
            "benchmarks": benchmarks,
        }
        (output_dir / f"{group_name}.json").write_text(json.dumps(payload, indent=2) + "\n")


def main() -> int:
    parser = argparse.ArgumentParser(description="Export Criterion benchmark estimates into compact reports")
    parser.add_argument("criterion_dir", type=Path)
    parser.add_argument("output_dir", type=Path)
    parser.add_argument("--include", action="append", dest="includes")
    args = parser.parse_args()

    includes = set(args.includes) if args.includes else None
    reports = collect_reports(args.criterion_dir, includes)
    write_reports(reports, args.output_dir)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())