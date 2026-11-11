#!/usr/bin/env python3
"""Plot a single controlled six-case Merkleize write-type archive.

Usage: .venv/bin/python measurements-rain/experiments/write-type/plot.py RUN_DIR
Requires Matplotlib. Outputs PNG, PDF, SVG, and the plotted data under RUN_DIR/figures.
"""

import argparse
import json
import math
import os
from pathlib import Path
import tempfile

os.environ.setdefault("MPLCONFIGDIR", str(Path(tempfile.gettempdir()) / "qmdb-write-type-mpl"))
os.environ.setdefault("XDG_CACHE_HOME", str(Path(tempfile.gettempdir()) / "qmdb-write-type-cache"))
import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt


CONFIGS = (("any", "unordered_btree"), ("current", "ordered"))
LABELS = ("Any unordered B-Tree", "Current ordered")
WORKLOADS = ("overwrite", "insert", "delete")
COLORS = ("#0072B2", "#D55E00")


def load_cases(run):
    rows = json.loads((run / "summary.json").read_text())
    if len(rows) != 6:
        raise ValueError("Expected exactly six cases from one process-repeat group")
    controls = (
        "keyspace_keys", "write_calls_per_batch", "write_skew_exponent",
        "value_layout", "merkle_family", "input_generation_timed", "batch_parent",
        "seed_synced", "commonware_page_cache", "iteration_policy",
    )
    required = {
        "operation": "merkleize", "durability": "speculative",
        "timing_scope": "batch_creation_staging_merkleization_root",
        "write_skew_exponent": 0, "input_generation_timed": False,
        "value_layout": "fixed", "merkle_family": "mmb",
    }
    cases = {}
    for row in rows:
        if any(row.get(k) != v for k, v in required.items()):
            raise ValueError("Archive has an unsupported phase or workload setting")
        if any(row.get(k) != rows[0].get(k) for k in controls):
            raise ValueError("Workload or execution controls differ across cases")
        config = (row["auth_mode"], row["index_order"])
        key = (config, row["write_type"])
        if config not in CONFIGS or row["write_type"] not in WORKLOADS or key in cases:
            raise ValueError(f"Unexpected or duplicate case: {key}")
        if config == CONFIGS[1] and str(row["bitmap_chunk_bytes"]) != "32":
            raise ValueError("Expected 32-byte Current bitmap chunks")
        lo, mean, hi = [float(row[k]) for k in ("mean_ci_low_us", "mean_us", "mean_ci_high_us")]
        if not all(math.isfinite(v) for v in (lo, mean, hi)) or not 0 < lo <= mean <= hi:
            raise ValueError(f"Invalid mean or interval: {key}")
        cases[key] = row

    # Case-specific archives can contain orphaned files from a shared Criterion
    # output directory. Match full benchmark identities, never shortened paths.
    verified = set()
    for path in (run / "criterion").rglob("new/estimates.json"):
        benchmark = path.parent / "benchmark.json"
        if not benchmark.exists():
            continue
        name = json.loads(benchmark.read_text()).get("full_id")
        for key, row in cases.items():
            if name != row["benchmark"]:
                continue
            estimate = json.loads(path.read_text())["mean"]
            ci = estimate["confidence_interval"]
            actual = (estimate["point_estimate"], ci["lower_bound"], ci["upper_bound"])
            expected = tuple(row[k] * 1000 for k in ("mean_us", "mean_ci_low_us", "mean_ci_high_us"))
            if not math.isclose(ci["confidence_level"], 0.95):
                raise ValueError("Expected Criterion 95% confidence intervals")
            if not all(math.isclose(a, b, rel_tol=1e-9) for a, b in zip(actual, expected)):
                raise ValueError(f"Summary differs from raw Criterion estimates: {key}")
            verified.add(key)
    if verified != set(cases):
        raise ValueError(f"Missing matching raw estimates for {set(cases) - verified}")
    return cases


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run_dir", type=Path)
    parser.add_argument("--output-dir", type=Path)
    args = parser.parse_args()
    run = args.run_dir.resolve()
    cases = load_cases(run)
    out = args.output_dir or run / "figures"
    out.mkdir(parents=True, exist_ok=True)
    first = next(iter(cases.values()))
    sample_counts = sorted({r["sample_count"] for r in cases.values()})
    sample_label = str(sample_counts[0]) if len(sample_counts) == 1 else ", ".join(map(str, sample_counts))
    plt.rcParams.update({"font.family": "DejaVu Sans", "font.size": 10, "svg.fonttype": "none", "pdf.fonttype": 42})
    fig, (latency, overhead) = plt.subplots(1, 2, figsize=(11, 5.8), gridspec_kw={"width_ratios": [1.35, 1]})
    fig.subplots_adjust(left=0.105, right=0.97, bottom=0.34, top=0.73, wspace=0.35)
    for ax in (latency, overhead):
        ax.set_axisbelow(True)
        ax.grid(axis="x", color="#E2E5E9", linewidth=0.7)
        ax.spines[["top", "right", "left"]].set_visible(False)
        ax.tick_params(axis="y", length=0, pad=8)
        ax.set_yticks(range(3), [w.capitalize() for w in WORKLOADS])
        ax.set_ylim(2.55, -0.65)
        ax.set_xlim(left=0)
    latency.set_title("a  Absolute latency", loc="left", pad=14)
    overhead.set_title("b  Relative latency overhead", loc="left", pad=14)
    maximum = 0
    for s, config in enumerate(CONFIGS):
        for i, workload in enumerate(WORKLOADS):
            row = cases[(config, workload)]
            mean, low, high = [row[k] / 1000 for k in ("mean_us", "mean_ci_low_us", "mean_ci_high_us")]
            maximum = max(maximum, high)
            y = i + (-0.14 if s == 0 else 0.14)
            latency.errorbar(mean, y, xerr=[[mean-low], [high-mean]], fmt="o" if s == 0 else "s",
                             color=COLORS[s], markersize=6, capsize=3, elinewidth=1.2,
                             label=LABELS[s] if i == 0 else None)
            latency.annotate(f"{mean:.2f}", (high, y), xytext=(6, 0), textcoords="offset points", va="center", fontsize=9)
    latency.set_xlim(0, maximum * 1.23)
    latency.set_xlabel("Mean batch latency (ms)", labelpad=10)
    plotted = []
    for i, workload in enumerate(WORKLOADS):
        a, c = [cases[(config, workload)]["mean_us"] for config in CONFIGS]
        pct = 100 * (c/a-1)
        overhead.plot(pct, i, "s", color=COLORS[1], markersize=6)
        overhead.annotate(f"{pct:+.1f}%", (pct, i), xytext=(8, 0), textcoords="offset points", va="center", fontsize=9)
        plotted.append({"write_type": workload, "any_mean_ms": a/1000, "current_mean_ms": c/1000,
                        "delta_ms": (c-a)/1000, "overhead_percent": pct})
    percentages = [r["overhead_percent"] for r in plotted]
    overhead.set_xlim(min(0, min(percentages)*1.2), max(1, max(percentages))*1.32)
    overhead.axvline(0, color="#666666", linewidth=1, linestyle="--")
    overhead.set_xlabel("Current relative to Any (%)", labelpad=10)
    fig.suptitle("Merkleize latency by mutation type", x=0.105, y=0.965, ha="left", fontsize=17)
    fig.text(0.105, 0.90, f"{int(first['keyspace_keys']):,} seeded keys  |  {int(first['write_calls_per_batch']):,} write calls per batch  |  uniform access  |  MMB", fontsize=10)
    fig.legend(*latency.get_legend_handles_labels(), loc="upper left", bbox_to_anchor=(0.098, 0.86), ncol=2, frameon=False)
    fig.text(0.105, 0.145, f"Points: Criterion means. Whiskers: Criterion 95% bootstrap confidence intervals.\nOne process repeat per case; {sample_label} Criterion samples per case. Overhead is descriptive, with no difference CI.", fontsize=9, linespacing=1.6)
    fig.text(0.105, 0.063, "Timed: batch creation, staging, Merkleization, root retrieval. No measured apply, commit, or sync.\nSource: " + run.name + "/summary.json", fontsize=8.5, color="#555555", linespacing=1.6)
    paths = []
    for ext in ("png", "pdf", "svg"):
        path = out / f"write-type-merkleize.{ext}"
        fig.savefig(path, dpi=220, facecolor="white")
        paths.append(path)
    plt.close(fig)
    notes = (run / "notes.md").read_text() if (run / "notes.md").exists() else ""
    payload = {"source": str(run / "summary.json"), "raw_estimates_verified": 6,
               "process_repeats_per_case": 1, "criterion_samples_per_case": sample_counts,
               "interval": "Criterion 95% bootstrap CI within each case",
               "run_notes": notes, "comparisons": plotted}
    path = out / "plotted-data.json"
    path.write_text(json.dumps(payload, indent=2) + "\n")
    print("Verified all six means and 95% intervals against raw Criterion estimates.")
    if "exited with status" in notes:
        print("NOTE: wrapper failure recorded; case estimates validated, run provenance still needs review.")
    for path in paths + [path]:
        print(path)


if __name__ == "__main__":
    main()
