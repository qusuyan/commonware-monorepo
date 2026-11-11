#!/usr/bin/env python3
"""Plot one archived writesize group using Matplotlib."""

import argparse
import json
import os
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
    parser.add_argument("comparison", type=Path, help="archived comparison.json")
    parser.add_argument("output_prefix", type=Path, nargs="?", help="output path without extension")
    args = parser.parse_args()

    comparison_path = args.comparison.resolve(strict=True)
    result, cases, differences = read_comparison(comparison_path)
    manifest = json.loads((comparison_path.parent / "manifest.json").read_text())
    values = [(name, case["mean_us"] / 1000, case["mean_ci_low_us"] / 1000,
               case["mean_ci_high_us"] / 1000) for name, case in cases.items()]

    fig, ax = plt.subplots(figsize=(12, 7.5))
    fig.set_facecolor("white")
    fig.subplots_adjust(left=0.24, right=0.96, top=0.78, bottom=0.43)
    colors = COLORS
    for y, (mode, mean, low, high) in zip(range(len(values) - 1, -1, -1), values):
        ax.hlines(y, low, high, color="#566B84", linewidth=3, zorder=2)
        ax.vlines((low, high), y - 0.025, y + 0.025, color="#566B84", linewidth=2, zorder=2)
        ax.scatter(mean, y, s=280, color=colors[mode], edgecolor="white", linewidth=1, zorder=3)
        ax.text(high + max(v[3] for v in values) * 0.03, y, f"{mean:.3g} ms  [{low:.3g}, {high:.3g}]", va="center", fontsize=11, color="#253347")

    ax.set_xlim(0, max(case[3] for case in values) * 1.36)
    ax.set_ylim(-0.6, len(values) - 0.4)
    ax.set_yticks(range(len(values) - 1, -1, -1), [LABELS[v[0]] for v in values])
    ax.set_xlabel("Mean latency per timed batch (ms)", fontsize=12, fontweight="bold", color="#253347")
    
    ax.tick_params(axis="both", length=0, labelsize=11, colors="#46586B")
    ax.grid(axis="x", color="#E5E9ED", linewidth=1)
    ax.set_axisbelow(True)
    for spine in ax.spines.values():
        spine.set_visible(False)

    fig.text(0.12, 0.94, "QMDB speculative write latency", ha="left", va="top",
             fontsize=19, fontweight="bold", color="#253347")
    fig.text(0.12, 0.865,
             f"{result['keyspace_keys']:,} seeded keys | {result['write_calls_per_batch']:,} write calls | fixed MMB",
             ha="left", va="top", fontsize=11.5, color="#46586B")
    fig.text(0.12, 0.13,
             f"Source: archived writesize comparison.json | {manifest['timestamp_utc']}\n"
             "Points: means | whiskers: Criterion 95% confidence intervals | "
             "one process run per configuration\n"
             "Timed path: staging, speculative merkleization, and root retrieval; inputs are prepared first.",
             ha="left", va="top", fontsize=9.5, color="#607082", linespacing=1.4)

    difference_lines = [
        f"{LABELS[diff['target']]} − {LABELS[diff['baseline']]}: "
        f"{diff['delta_mean_us'] / 1000:+.3g} ms ({diff['overhead_percent']:+.1f}%)"
        for diff in differences.values()
    ]
    fig.text(0.12, 0.32, "Descriptive differences (no difference confidence intervals)\n" +
             "\n".join(difference_lines), ha="left", va="top", fontsize=10, linespacing=1.5)

    output_prefix = args.output_prefix or comparison_path.with_name("latency-comparison")
    for extension in ("png", "pdf"):
        fig.savefig(f"{output_prefix}.{extension}", dpi=180, facecolor="white")
    plt.close(fig)


if __name__ == "__main__":
    main()
