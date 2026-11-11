#!/usr/bin/env python3
"""Render standalone, presentation-ready figures from archived QMDB measurements."""

import argparse
import json
import os
import statistics
import tempfile
from collections import defaultdict
from pathlib import Path

cache_root = Path(tempfile.gettempdir()) / "qmdb-experiment-figures"
cache_root.mkdir(parents=True, exist_ok=True)
os.environ.setdefault("XDG_CACHE_HOME", str(cache_root))
os.environ.setdefault("MPLCONFIGDIR", str(cache_root / "matplotlib"))

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt


NAVY = "#20364F"
SLATE = "#53677C"
GRID = "#DCE3E9"
ANY = "#287D8E"
CURRENT = "#D06D45"
ORDERED = "#735DA5"


def style(ax):
    ax.grid(color=GRID, linewidth=0.8)
    ax.set_axisbelow(True)
    ax.tick_params(length=0, colors=SLATE, labelsize=10)
    for spine in ax.spines.values():
        spine.set_visible(False)


def save(fig, output: Path):
    output.parent.mkdir(parents=True, exist_ok=True)
    for suffix in (".png", ".pdf"):
        fig.savefig(output.with_suffix(suffix), dpi=220, facecolor="white", bbox_inches="tight")
    plt.close(fig)


def add_title(fig, title, subtitle):
    fig.text(0.125, 0.965, title, ha="left", va="top", fontsize=17, fontweight="bold", color=NAVY)
    fig.text(0.125, 0.918, subtitle, ha="left", va="top", fontsize=10.5, color=SLATE)


def timer_boundary_figures(results: Path, output: Path):
    rows = json.loads((results / "summary.json").read_text())
    rows.sort(key=lambda row: (row["skew"], row["writes"]))
    skews = sorted({row["skew"] for row in rows})

    fig, axes = plt.subplots(1, len(skews), figsize=(12, 5.4), sharey=True)
    fig.set_facecolor("white")
    if len(skews) == 1:
        axes = [axes]
    for ax, skew in zip(axes, skews):
        subset = [row for row in rows if row["skew"] == skew]
        x = [row["writes"] for row in subset]
        for key, label, color in (("any", "Any", ANY), ("current", "Current", CURRENT)):
            y = [row[key]["median_mean_us"] / 1000 for row in subset]
            low = [row[key]["min_mean_us"] / 1000 for row in subset]
            high = [row[key]["max_mean_us"] / 1000 for row in subset]
            ax.plot(x, y, "o-", color=color, linewidth=2.5, markersize=8, label=label)
            ax.fill_between(x, low, high, color=color, alpha=0.15)
        ax.set_xscale("log")
        ax.set_xticks(x, [f"{value:,}" for value in x])
        ax.set_title(f"Zipf exponent {skew:g}", fontsize=11, fontweight="bold", color=NAVY, pad=12)
        ax.set_xlabel("Write calls per batch", fontweight="bold", color=NAVY)
        style(ax)
    axes[0].set_ylabel("Latency per batch (ms)", fontweight="bold", color=NAVY)
    axes[0].legend(title="Variant", frameon=False, loc="upper left")
    add_title(fig, "Timer-boundary review: batch latency", "Median of 3 process runs; shaded range is min–max across runs | 10K keys | fixed MMB")
    fig.text(0.125, 0.035, "Timed path: batch creation, prepared-write staging, merkleization, and root retrieval. Input generation excluded.", color=SLATE, fontsize=9)
    fig.subplots_adjust(left=0.125, right=0.97, top=0.82, bottom=0.17, wspace=0.07)
    save(fig, output / "timer-boundary-latency")

    fig, ax = plt.subplots(figsize=(8.6, 5.1))
    fig.set_facecolor("white")
    for skew in skews:
        subset = [row for row in rows if row["skew"] == skew]
        ax.plot([row["writes"] for row in subset], [row["current_over_any_percent"] for row in subset], "o-", linewidth=2.5, markersize=8, label=f"Zipf {skew:g}")
    ax.axhline(0, linewidth=1, color=SLATE)
    ax.set_xscale("log")
    ax.set_xticks([100, 1000], ["100", "1,000"])
    ax.set_xlabel("Write calls per batch", fontweight="bold", color=NAVY)
    ax.set_ylabel("Current vs Any overhead (%)", fontweight="bold", color=NAVY)
    ax.legend(title="Access distribution", frameon=False)
    style(ax)
    add_title(fig, "Timer-boundary review: Current overhead", "Difference in median batch latency across 3 independent process runs | 10K keys | fixed MMB")
    fig.subplots_adjust(left=0.14, right=0.97, top=0.82, bottom=0.16)
    save(fig, output / "timer-boundary-overhead")


def controlled_skew_records(skew_root: Path):
    notes = (skew_root / "notes.md").read_text()
    if "Keys: 10000000; write calls: 10000; process repeats: 3; smoke: false." not in notes:
        raise ValueError("skew archive is not the controlled 10M-key, 10K-write experiment")
    records = []
    for manifest_path in skew_root.rglob("manifest.json"):
        manifest = json.loads(manifest_path.read_text())
        if (manifest.get("kind") != "criterion" or manifest.get("write_type") != "overwrite"
                or manifest.get("fixed_updates_per_batch") != 10_000
                or manifest.get("merkleize_input_generation_timed") is not False):
            continue
        summary_paths = list(manifest_path.parent.glob("summary.json"))
        if len(summary_paths) != 1:
            continue
        summaries = json.loads(summary_paths[0].read_text())
        if len(summaries) != 1 or summaries[0].get("keyspace_keys") != "10000000":
            continue
        mean = summaries[0]["mean_us"]
        run = manifest["run_id"].rsplit("/", 1)[-1]
        if run.endswith("-any_unordered"):
            variant, color = "Any + unordered", ANY
        elif run.endswith("-current_unordered"):
            variant, color = "Current + unordered", CURRENT
        elif run.endswith("-current_ordered"):
            variant, color = "Current + ordered", ORDERED
        else:
            continue
        records.append((manifest["write_skew_exponent"], variant, color, mean / 1000))
    grouped = defaultdict(list)
    colors = {}
    for exponent, variant, color, latency in records:
        grouped[(exponent, variant)].append(latency)
        colors[variant] = color
    exponents = sorted({key[0] for key in grouped})
    variants = ["Any + unordered", "Current + unordered", "Current + ordered"]
    expected = {(exponent, variant) for exponent in exponents for variant in variants}
    if set(grouped) != expected or any(len(values) != 3 for values in grouped.values()):
        raise ValueError("controlled skew archive is incomplete")

    return exponents, variants, grouped, colors


def skew_figure(skew_root: Path, output: Path):
    exponents, _, grouped, colors = controlled_skew_records(skew_root)
    # Any + unordered is the baseline QMDB configuration used for the isolated
    # skew plot. The configuration comparison is rendered separately at Zipf 0.
    variant = "Any + unordered"
    values = [grouped[(exponent, variant)] for exponent in exponents]
    baseline = statistics.median(values[0])
    medians = [statistics.median(value) / baseline for value in values]
    lows = [min(value) / baseline for value in values]
    highs = [max(value) / baseline for value in values]
    fig, ax = plt.subplots(figsize=(8.6, 5.0))
    fig.set_facecolor("white")
    ax.errorbar(exponents, medians,
                yerr=[[mid - low for mid, low in zip(medians, lows)],
                      [high - mid for mid, high in zip(medians, highs)]],
                fmt="o-", linewidth=2.8, markersize=9, capsize=4,
                color=colors[variant])
    ax.axhline(1, color=SLATE, linewidth=1, zorder=0)
    ax.set_xlabel("Zipf skew exponent", fontweight="bold", color=NAVY)
    ax.set_ylabel("Latency relative to uniform (×)", fontweight="bold", color=NAVY)
    ax.set_xticks(exponents, ["Uniform" if exponent == 0 else f"{exponent:g}" for exponent in exponents])
    style(ax)
    add_title(fig, "Effect of write skew", "Any + unordered QMDB | all other factors fixed: 10M keys | overwrite | 10K writes per batch | fixed MMB")
    fig.text(0.125, 0.035, "Points: median of 3 process runs; bars: min–max. Values are normalized to the uniform baseline.", color=SLATE, fontsize=9)
    fig.subplots_adjust(left=0.13, right=0.97, top=0.82, bottom=0.16)
    save(fig, output / "skew-effect-controlled")


def configuration_figure(skew_root: Path, output: Path):
    _, variants, grouped, colors = controlled_skew_records(skew_root)
    labels = {"Any + unordered": "Any\nunordered", "Current + unordered": "Current\nunordered", "Current + ordered": "Current\nordered"}
    values = [grouped[(0, variant)] for variant in variants]
    # controlled_skew_records stores latency in milliseconds.
    medians = [statistics.median(value) for value in values]
    lows = [min(value) for value in values]
    highs = [max(value) for value in values]
    fig, ax = plt.subplots(figsize=(8.4, 5.0))
    fig.set_facecolor("white")
    positions = list(range(len(variants)))
    ax.errorbar(positions, medians,
                yerr=[[mid - low for mid, low in zip(medians, lows)],
                      [high - mid for mid, high in zip(medians, highs)]],
                fmt="none", ecolor=SLATE, capsize=5, linewidth=1.8, zorder=2)
    ax.scatter(positions, medians, s=170, color=[colors[variant] for variant in variants], zorder=3)
    ax.set_xticks(positions, [labels[variant] for variant in variants])
    ax.set_ylabel("Latency per batch (ms)", fontweight="bold", color=NAVY)
    ax.set_ylim(bottom=0)
    style(ax)
    add_title(fig, "QMDB configuration comparison", "Uniform access (Zipf 0) | 10M keys | overwrite | 10K writes per batch | fixed MMB")
    fig.text(0.125, 0.035, "Points: median of 3 process runs; bars: min–max. This is the configuration comparison kept separate from skew.", color=SLATE, fontsize=9)
    fig.subplots_adjust(left=0.14, right=0.97, top=0.82, bottom=0.20)
    save(fig, output / "qmdb-configuration-uniform")


def write_type_figure(results: Path, output: Path):
    group = results / "write-type"
    comparison_paths = sorted(group.glob("20261030T111439Z-u10000-r*/comparison.json"))
    if len(comparison_paths) != 3:
        raise ValueError("expected the complete three-repeat 10K write-type group")
    measurements = defaultdict(list)
    for path in comparison_paths:
        data = json.loads(path.read_text())
        if data["keyspace_keys"] != 10_000_000 or data["write_skew_exponent"] != 0 or data["write_calls_per_batch"] != 10_000:
            raise ValueError(f"unexpected write-type controls in {path}")
        for config, cases in data["configurations"].items():
            for write_type in ("overwrite", "insert"):
                measurements[(config, write_type)].append(cases[write_type]["mean_us"] / 1000)
    configs = ["any_unordered", "current_unordered", "current_ordered"]
    labels = {"any_unordered": "Any + unordered", "current_unordered": "Current + unordered", "current_ordered": "Current + ordered"}
    fig, ax = plt.subplots(figsize=(9.0, 5.2))
    fig.set_facecolor("white")
    for position, config in enumerate(configs):
        overwrite = measurements[(config, "overwrite")]
        insert = measurements[(config, "insert")]
        x = [statistics.median(overwrite), statistics.median(insert)]
        ax.plot(x, [position, position], color="#B9C4CE", linewidth=2, zorder=1)
        for value, values, color, label in ((x[0], overwrite, CURRENT, "Overwrite"), (x[1], insert, ANY, "Insert")):
            ax.errorbar(value, position, xerr=[[value - min(values)], [max(values) - value]], fmt="o", markersize=10,
                        color=color, ecolor=color, capsize=4, label=label if position == 0 else None, zorder=2)
    ax.set_xscale("log")
    ax.set_yticks(range(len(configs)), [labels[config] for config in configs])
    ax.set_xlabel("Latency per batch (ms, logarithmic)", fontweight="bold", color=NAVY)
    ax.legend(frameon=False, loc="lower right")
    style(ax)
    add_title(fig, "Effect of write type", "All other factors fixed: 10M keys | uniform access | 10K writes per batch | fixed MMB")
    fig.text(0.125, 0.035, "Points: median of 3 process runs; bars: min–max. Each row compares overwrite and insert within the same QMDB configuration.", color=SLATE, fontsize=9)
    fig.subplots_adjust(left=0.25, right=0.97, top=0.82, bottom=0.17)
    save(fig, output / "write-type-effect")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--results", type=Path, default=Path(__file__).resolve().parents[1] / "results")
    parser.add_argument("--output", type=Path, default=None)
    args = parser.parse_args()
    results = args.results.resolve()
    output = (args.output or results / "figures").resolve()
    skew_root = results / "skew" / "20261030T124219Z"
    skew_figure(skew_root, output)
    configuration_figure(skew_root, output)
    write_type_figure(results, output)
    print(f"Rendered figures to {output}")


if __name__ == "__main__":
    main()
