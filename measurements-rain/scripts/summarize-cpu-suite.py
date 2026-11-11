#!/usr/bin/env python3
"""Validate a completed qmdb_cpu suite and generate pairwise comparison JSON and aspect CSVs."""

import argparse
import csv
import json
from pathlib import Path
import statistics
import sys

VARIANTS = ("any_unordered_btree", "current_ordered")


def pct_overhead(target, baseline):
    if baseline == 0:
        return None
    return ((target - baseline) / baseline) * 100.0


def load_runs(suite_dir: Path):
    runs_file = suite_dir / "suite_runs.jsonl"
    if not runs_file.is_file():
        raise FileNotFoundError(f"missing suite runs file: {runs_file}")

    entries = []
    seen_ids = set()
    for line in runs_file.read_text().splitlines():
        if not line.strip():
            continue
        rec = json.loads(line)
        run_id = rec["run_id"]
        if run_id in seen_ids:
            raise ValueError(f"duplicate run_id in suite_runs.jsonl: {run_id}")
        seen_ids.add(run_id)

        run_dir = Path(rec["run_dir"])
        summary_path = run_dir / "cpu" / "summary.json"
        if not summary_path.is_file():
            raise FileNotFoundError(f"missing summary.json for {run_id}: {summary_path}")

        summary = json.loads(summary_path.read_text())
        cfg = summary["configuration"]
        if (
            cfg["phase"] != rec["phase"]
            or cfg["variant"] != rec["variant"]
            or int(cfg["keys"]) != int(rec["keys"])
            or int(cfg["submitted_write_attempts"]) != int(rec["updates"])
            or cfg["write_type"] != rec["write_type"]
            or float(cfg["skew"]) != float(rec["skew"])
            or summary["sample_count"] <= 0
            or summary["overhead_subtracted"] is not False
        ):
            raise ValueError(f"mismatched metadata in {summary_path}")

        entries.append((rec, summary))
    if not entries:
        raise ValueError("no completed runs found in suite_runs.jsonl")
    return entries


def aggregate_variant(summaries):
    stats_keys = (
        "wall_ns",
        "total_cpu_ns",
        "user_cpu_ns",
        "system_cpu_ns",
        "cpu_ns_per_batch",
        "cpu_ns_per_write",
        "average_cores_used",
        "cpu_usage_percent",
        "involuntary_context_switches",
        "voluntary_context_switches",
        "minor_page_faults",
        "major_page_faults",
    )
    out = {
        "repeat_count": len(summaries),
        "total_samples": sum(s["sample_count"] for s in summaries),
        "precision_warning_count": sum(s["precision_warning_count"] for s in summaries),
    }
    for key in stats_keys:
        means = [s["statistics"][key]["mean"] for s in summaries]
        medians = [s["statistics"][key]["median"] for s in summaries]
        out[f"{key}_mean"] = statistics.mean(means)
        out[f"{key}_median"] = statistics.median(medians)
    return out


def summarize(suite_dir: Path):
    entries = load_runs(suite_dir)
    phases = {rec["phase"] for rec, _ in entries}
    if len(phases) != 1:
        raise ValueError(f"cannot mix phases in one suite: {sorted(phases)}")
    phase = next(iter(phases))

    grouped = {}
    for rec, summary in entries:
        cell = (
            rec["aspect"],
            int(rec["keys"]),
            int(rec["updates"]),
            rec["write_type"],
            float(rec["skew"]),
        )
        grouped.setdefault(cell, {}).setdefault(rec["variant"], []).append((int(rec["repeat"]), summary))

    comparison_cells = []
    by_aspect = {}

    for cell in sorted(grouped):
        aspect, keys, updates, write_type, skew = cell
        variants_map = grouped[cell]
        if set(variants_map) != set(VARIANTS):
            raise ValueError(f"incomplete variant pair for {cell}: {sorted(variants_map)}")

        per_variant = {}
        for variant in VARIANTS:
            ordered = [s for _, s in sorted(variants_map[variant], key=lambda item: item[0])]
            per_variant[variant] = aggregate_variant(ordered)

        a = per_variant["any_unordered_btree"]
        b = per_variant["current_ordered"]

        deltas = {
            "delta_wall_ns_mean": b["wall_ns_mean"] - a["wall_ns_mean"],
            "delta_wall_ns_median": b["wall_ns_median"] - a["wall_ns_median"],
            "wall_overhead_percent_mean": pct_overhead(b["wall_ns_mean"], a["wall_ns_mean"]),
            "delta_total_cpu_ns_mean": b["total_cpu_ns_mean"] - a["total_cpu_ns_mean"],
            "delta_total_cpu_ns_median": b["total_cpu_ns_median"] - a["total_cpu_ns_median"],
            "cpu_overhead_percent_mean": pct_overhead(b["total_cpu_ns_mean"], a["total_cpu_ns_mean"]),
            "cpu_overhead_percent_median": pct_overhead(b["total_cpu_ns_median"], a["total_cpu_ns_median"]),
            "delta_user_cpu_ns_mean": b["user_cpu_ns_mean"] - a["user_cpu_ns_mean"],
            "delta_system_cpu_ns_mean": b["system_cpu_ns_mean"] - a["system_cpu_ns_mean"],
            "delta_cpu_ns_per_write_mean": b["cpu_ns_per_write_mean"] - a["cpu_ns_per_write_mean"],
        }

        cell_record = {
            "aspect": aspect,
            "phase": phase,
            "keys": keys,
            "updates": updates,
            "write_type": write_type,
            "skew": skew,
            "variants": per_variant,
            "comparison_current_minus_any_btree": deltas,
        }
        comparison_cells.append(cell_record)
        by_aspect.setdefault(aspect, []).append(cell_record)

    comparison_out = {
        "schema_version": 1,
        "source": "libc_getrusage",
        "phase": phase,
        "variants": list(VARIANTS),
        "cells": comparison_cells,
    }
    (suite_dir / "comparison.json").write_text(json.dumps(comparison_out, indent=2) + "\n")

    fieldnames = [
        "aspect",
        "phase",
        "keys",
        "updates",
        "write_type",
        "skew",
        "any_repeat_count",
        "any_total_samples",
        "any_precision_warnings",
        "any_wall_ms_mean",
        "any_total_cpu_ms_mean",
        "any_user_cpu_ms_mean",
        "any_system_cpu_ms_mean",
        "any_cpu_us_per_write_mean",
        "any_avg_cores_mean",
        "any_cpu_usage_pct_mean",
        "current_repeat_count",
        "current_total_samples",
        "current_precision_warnings",
        "current_wall_ms_mean",
        "current_total_cpu_ms_mean",
        "current_user_cpu_ms_mean",
        "current_system_cpu_ms_mean",
        "current_cpu_us_per_write_mean",
        "current_avg_cores_mean",
        "current_cpu_usage_pct_mean",
        "delta_wall_ms_mean",
        "wall_overhead_pct_mean",
        "delta_total_cpu_ms_mean",
        "cpu_overhead_pct_mean",
        "delta_user_cpu_ms_mean",
        "delta_system_cpu_ms_mean",
        "delta_cpu_us_per_write_mean",
    ]

    for aspect, cells in by_aspect.items():
        csv_path = suite_dir / f"{aspect}.csv"
        with csv_path.open("w", newline="") as handle:
            writer = csv.DictWriter(handle, fieldnames=fieldnames)
            writer.writeheader()
            for c in cells:
                a = c["variants"]["any_unordered_btree"]
                b = c["variants"]["current_ordered"]
                d = c["comparison_current_minus_any_btree"]
                writer.writerow(
                    {
                        "aspect": c["aspect"],
                        "phase": c["phase"],
                        "keys": c["keys"],
                        "updates": c["updates"],
                        "write_type": c["write_type"],
                        "skew": c["skew"],
                        "any_repeat_count": a["repeat_count"],
                        "any_total_samples": a["total_samples"],
                        "any_precision_warnings": a["precision_warning_count"],
                        "any_wall_ms_mean": round(a["wall_ns_mean"] / 1e6, 4),
                        "any_total_cpu_ms_mean": round(a["total_cpu_ns_mean"] / 1e6, 4),
                        "any_user_cpu_ms_mean": round(a["user_cpu_ns_mean"] / 1e6, 4),
                        "any_system_cpu_ms_mean": round(a["system_cpu_ns_mean"] / 1e6, 4),
                        "any_cpu_us_per_write_mean": round(a["cpu_ns_per_write_mean"] / 1e3, 4),
                        "any_avg_cores_mean": round(a["average_cores_used_mean"], 4),
                        "any_cpu_usage_pct_mean": round(a["cpu_usage_percent_mean"], 2),
                        "current_repeat_count": b["repeat_count"],
                        "current_total_samples": b["total_samples"],
                        "current_precision_warnings": b["precision_warning_count"],
                        "current_wall_ms_mean": round(b["wall_ns_mean"] / 1e6, 4),
                        "current_total_cpu_ms_mean": round(b["total_cpu_ns_mean"] / 1e6, 4),
                        "current_user_cpu_ms_mean": round(b["user_cpu_ns_mean"] / 1e6, 4),
                        "current_system_cpu_ms_mean": round(b["system_cpu_ns_mean"] / 1e6, 4),
                        "current_cpu_us_per_write_mean": round(b["cpu_ns_per_write_mean"] / 1e3, 4),
                        "current_avg_cores_mean": round(b["average_cores_used_mean"], 4),
                        "current_cpu_usage_pct_mean": round(b["cpu_usage_percent_mean"], 2),
                        "delta_wall_ms_mean": round(d["delta_wall_ns_mean"] / 1e6, 4),
                        "wall_overhead_pct_mean": (
                            round(d["wall_overhead_percent_mean"], 2)
                            if d["wall_overhead_percent_mean"] is not None
                            else ""
                        ),
                        "delta_total_cpu_ms_mean": round(d["delta_total_cpu_ns_mean"] / 1e6, 4),
                        "cpu_overhead_pct_mean": (
                            round(d["cpu_overhead_percent_mean"], 2)
                            if d["cpu_overhead_percent_mean"] is not None
                            else ""
                        ),
                        "delta_user_cpu_ms_mean": round(d["delta_user_cpu_ns_mean"] / 1e6, 4),
                        "delta_system_cpu_ms_mean": round(d["delta_system_cpu_ns_mean"] / 1e6, 4),
                        "delta_cpu_us_per_write_mean": round(d["delta_cpu_ns_per_write_mean"] / 1e3, 4),
                    }
                )
    return comparison_out


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("suite_dir", type=Path, help="Path to results/cpu-suites/<suite-id>")
    args = parser.parse_args()
    summarize(args.suite_dir)


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        sys.exit(1)
