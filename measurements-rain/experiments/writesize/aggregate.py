#!/usr/bin/env python3
"""Combine three archived process groups per write count and plot the sweep."""

import argparse
import csv
import json
import os
import statistics
import tempfile
from pathlib import Path

cache_root = Path(tempfile.gettempdir()) / "qmdb-writesize-plot-cache"
cache_root.mkdir(parents=True, exist_ok=True)
os.environ.setdefault("XDG_CACHE_HOME", str(cache_root))
os.environ.setdefault("MPLCONFIGDIR", str(cache_root / "matplotlib"))

import matplotlib  # noqa: E402

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
from report import read_comparison, LABELS, COLORS

def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output_dir", type=Path)
    parser.add_argument("comparisons", nargs="+", type=Path,
                        help="three successful group comparison.json files per write count")
    args = parser.parse_args()

    groups: dict[int, list[dict]] = {}
    provenance = None
    configuration_set = None
    keyspace_keys = None
    for input_path in args.comparisons:
        path = input_path.resolve(strict=True)
        comparison, cases, differences = read_comparison(path)
        if configuration_set is None:
            configuration_set = tuple(cases)
        elif set(configuration_set) != set(cases):
            raise ValueError("cannot mix runs with different configuration sets")
        manifest = json.loads((path.parent / "manifest.json").read_text())
        hashes = (path.parent / "source-hashes.txt").read_text()
        if hashes != (path.parent / "source-hashes-after.txt").read_text():
            raise ValueError(f"benchmark source changed during {path.parent}")
        if comparison["experiment"] != "writesize":
            raise ValueError(f"unexpected workload in {path}")
        if keyspace_keys is None:
            keyspace_keys = comparison["keyspace_keys"]
        elif keyspace_keys != comparison["keyspace_keys"]:
            raise ValueError(f"cannot mix keyspace sizes in {path}")
        count = comparison["write_calls_per_batch"]
        if count != manifest["fixed_updates_per_batch"]:
            raise ValueError(f"unexpected write count in {path}")
        current_provenance = {
            "git_revision": manifest["git_revision"],
            "host": manifest["host"],
            "os": manifest["os"],
            "cpu": manifest["cpu"],
            "rustc": manifest["rustc"],
            "source_hashes": hashes,
        }
        if provenance is None:
            provenance = current_provenance
        elif provenance != current_provenance:
            raise ValueError(f"host, toolchain, or benchmark sources differ in {path.parent}")
        if any(run["run_id"] == manifest["run_id"] for runs in groups.values() for run in runs):
            raise ValueError("duplicate process run")
        groups.setdefault(count, []).append(
            {"run_id": manifest["run_id"], "cases": cases, "differences": differences}
        )

    counts = tuple(sorted(groups))
    if any(len(groups[count]) != 3 for count in counts):
        raise ValueError("expected exactly three successful process groups at each write count")

    rows = []
    for count in counts:
        pairs = groups[count]
        row = {"write_calls_per_batch": count, "process_runs": 3,
               "run_ids": [run["run_id"] for run in pairs]}
        for mode in configuration_set:
            values = [run["cases"][mode]["mean_us"] / 1000 for run in pairs]
            row[f"{mode}_mean_ms"] = statistics.mean(values)
            row[f"{mode}_between_run_sd_ms"] = statistics.stdev(values)
        for name in pairs[0]["differences"]:
            for metric, scale in (("delta_mean_us", 0.001), ("overhead_percent", 1)):
                values = [run["differences"][name][metric] * scale for run in pairs]
                unit = "delta_ms" if metric == "delta_mean_us" else "overhead_percent"
                row[f"{name}_{unit}"] = statistics.mean(values)
                row[f"{name}_{unit}_sd"] = statistics.stdev(values)
        rows.append(row)

    output_dir = args.output_dir
    if output_dir.exists():
        raise FileExistsError(f"output directory already exists: {output_dir}")
    output_dir.mkdir(parents=True)
    report = {
        "experiment": "writesize-full",
        "keyspace_keys": keyspace_keys,
        "schema_version": 2,
        "configurations": list(configuration_set),
        "process_runs_per_count": 3,
        "provenance": provenance,
        "rows": rows,
        "interpretation": "Means and sample SD are across independent process groups; per-case Criterion confidence intervals remain in each group's comparison.json.",
    }
    (output_dir / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
    with (output_dir / "summary.csv").open("w", newline="") as output:
        writer = csv.DictWriter(output, fieldnames=[name for name in rows[0] if name != "run_ids"])
        writer.writeheader()
        writer.writerows({name: row[name] for name in writer.fieldnames} for row in rows)

    fig, (latency_ax, overhead_ax) = plt.subplots(2, 1, figsize=(11, 8), sharex=True,
                                                 gridspec_kw={"height_ratios": (2, 1)})
    fig.set_facecolor("white")
    fig.subplots_adjust(left=0.13, right=0.96, top=0.84, bottom=0.16, hspace=0.30)
    colors = COLORS
    for mode in configuration_set:
        latency_ax.errorbar(
            counts,
            [row[f"{mode}_mean_ms"] for row in rows],
            yerr=[row[f"{mode}_between_run_sd_ms"] for row in rows],
            fmt="o-", markersize=8, linewidth=2, capsize=4, label=LABELS[mode], color=colors[mode],
        )
    latency_ax.set_yscale("log")
    latency_ax.set_ylabel("Mean latency per batch (ms)", fontsize=11, fontweight="bold")
    latency_ax.legend(frameon=False, title="QMDB mode", loc="upper left")

    for name, diff in groups[counts[0]][0]["differences"].items():
        overhead_ax.errorbar(
            counts, [row[f"{name}_overhead_percent"] for row in rows],
            yerr=[row[f"{name}_overhead_percent_sd"] for row in rows],
            fmt="o-", capsize=4,
            label=f"{LABELS[diff['target']]} vs {LABELS[diff['baseline']]}")
    overhead_ax.legend(frameon=False, fontsize=8)
    overhead_ax.set_ylabel("Incremental overhead (%)", fontsize=11, fontweight="bold")
    overhead_ax.set_xlabel(f"Write calls per timed batch ({keyspace_keys:,} seeded keys)", fontsize=11, fontweight="bold")
    overhead_ax.set_xscale("log")
    overhead_ax.set_xticks(counts, [f"{count / 1000:g}K" for count in counts])
    overhead_ax.minorticks_off()

    for ax in (latency_ax, overhead_ax):
        ax.grid(color="#E5E9ED", linewidth=1)
        ax.set_axisbelow(True)
        ax.tick_params(axis="both", length=0, labelsize=10, colors="#46586B")
        for spine in ax.spines.values():
            spine.set_visible(False)
    fig.text(0.13, 0.95, "QMDB writesize sweep", ha="left", va="top",
             fontsize=17, fontweight="bold", color="#253347")
    fig.text(0.13, 0.905, f"{keyspace_keys:,} seeded keys | fixed MMB | direct, synced, warm-cache batches",
             ha="left", va="top", fontsize=10.5, color="#46586B")
    fig.text(0.13, 0.06,
             "Points: mean of 3 process groups | error bars: between-run sample SD | top axis is logarithmic\n"
             "Timed path: staging, speculative merkleization, and root retrieval; inputs are prepared first.",
             ha="left", va="top", fontsize=9, color="#607082", linespacing=1.4)
    for extension in ("png", "pdf"):
        fig.savefig(output_dir / f"latency-sweep.{extension}", dpi=180, facecolor="white")
    plt.close(fig)


if __name__ == "__main__":
    main()
