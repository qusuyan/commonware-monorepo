#!/usr/bin/env python3
"""Render the selected MMR Criterion comparisons with Matplotlib.

Usage:
    python render_selected_graphs.py <summary.json> <output-directory>
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import matplotlib.pyplot as plt


REQUIRED = {
    "index_order": "unordered",
    "value_layout": "fixed",
    "merkle_family": "mmr",
    "keyspace_keys": "10000",
    "batch_parent": "directly from database",
    "seed_synced": "yes",
    "commonware_page_cache": "retained (warm)",
}


def selected(rows: list[dict], auth_mode: str, chunk: str) -> dict:
    """Find one fully matched primary-baseline row or fail rather than guess."""
    matches = [
        row
        for row in rows
        if all(row[key] == value for key, value in REQUIRED.items())
        and row["auth_mode"] == auth_mode
        and row["bitmap_chunk_bytes"] == chunk
    ]
    if len(matches) != 1:
        raise ValueError(f"expected one row for auth_mode={auth_mode}, chunk={chunk}; found {len(matches)}")
    return matches[0]


def render(
    output_dir: Path,
    stem: str,
    title: str,
    comparison: str,
    cases: list[tuple[str, str, dict]],
) -> None:
    """Render mean and 95% Criterion confidence intervals for two matched cases."""
    figure, axis = plt.subplots(figsize=(11.5, 6.4))
    # Reserve deliberate room for two-line case descriptions on the left and the
    # methodological note below the x-axis. Avoid a tight bounding box, which can
    # crowd those explanatory labels into the plot.
    figure.subplots_adjust(left=0.32, right=0.96, top=0.75, bottom=0.28)
    colors = ["#5c6773", "#1976b9"]
    y_positions = [1, 0]

    confidence_lows = [case[2]["mean_ci_low_us"] for case in cases]
    confidence_highs = [case[2]["mean_ci_high_us"] for case in cases]
    x_min = min(confidence_lows)
    x_max = max(confidence_highs)
    padding = max((x_max - x_min) * 0.18, 12.0)

    for index, ((label, description, row), y_position) in enumerate(zip(cases, y_positions)):
        mean = row["mean_us"]
        lower = row["mean_ci_low_us"]
        upper = row["mean_ci_high_us"]
        axis.errorbar(
            mean,
            y_position,
            xerr=[[mean - lower], [upper - mean]],
            fmt="o",
            markersize=10,
            color=colors[index],
            ecolor=colors[index],
            elinewidth=3,
            capsize=7,
            capthick=2,
            zorder=3,
        )
        axis.annotate(
            f"{mean:.1f} us\n95% CI: {lower:.1f}-{upper:.1f}",
            xy=(mean, y_position),
            xytext=(10, 16),
            textcoords="offset points",
            fontsize=11,
            color=colors[index],
            fontweight="semibold",
        )

    first_mean = cases[0][2]["mean_us"]
    second_mean = cases[1][2]["mean_us"]
    relative_change = (second_mean / first_mean - 1) * 100
    absolute_change = second_mean - first_mean

    axis.set_xlim(x_min - padding, x_max + padding)
    axis.set_ylim(-0.65, 1.65)
    axis.set_yticks(y_positions)
    axis.set_yticklabels([f"{label}\n{description}" for label, description, _ in cases], fontsize=11)
    axis.set_xlabel("Mean time per timed iteration (microseconds)", fontsize=13, fontweight="semibold")
    axis.xaxis.grid(True, color="#d9dee3", linewidth=0.8)
    axis.yaxis.grid(False)
    axis.spines[["top", "right", "left"]].set_visible(False)
    axis.tick_params(axis="y", length=0)
    axis.tick_params(axis="x", labelsize=11)

    figure.suptitle(title, x=0.08, ha="left", fontsize=16, fontweight="bold")
    axis.set_title(
        "Matched settings: MMR | unordered | fixed 32-byte values | 10,000 seeded keys\n"
        "Direct batch (ch=false) | synced seed | Commonware page cache retained (warm)",
        loc="left",
        fontsize=10.5,
        color="#45515e",
        pad=20,
    )
    figure.text(
        0.08,
        0.055,
        f"{comparison}\nSecond case relative to first: {relative_change:+.1f}% ({absolute_change:+.1f} us). "
        "Dot = Criterion mean; bar = 95% bootstrap CI; n=10 samples per case; one benchmark-process run.",
        fontsize=9.5,
        color="#45515e",
    )

    for extension in ("png", "svg"):
        figure.savefig(output_dir / f"{stem}.{extension}", dpi=300)
    plt.close(figure)


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit("usage: render_selected_graphs.py <summary.json> <output-directory>")

    summary_path = Path(sys.argv[1])
    output_dir = Path(sys.argv[2])
    output_dir.mkdir(parents=True, exist_ok=True)
    rows = json.loads(summary_path.read_text())

    any_row = selected(rows, "any", "")
    current_32 = selected(rows, "current", "32")
    current_256 = selected(rows, "current", "256")

    render(
        output_dir,
        "mmr-any-vs-current-chunk-32",
        "MMR baseline: any versus current with 32-byte bitmap chunks",
        "Cases compared: any (local bitmap, not authenticated) versus current (bitmap authenticated through 32-byte grafted chunks).",
        [
            ("any", "Local bitmap; not root-authenticated", any_row),
            ("current, chunk=32", "32-byte chunks; grafted/root-authenticated", current_32),
        ],
    )
    render(
        output_dir,
        "mmr-current-chunk-32-vs-256",
        "MMR chunk-size sensitivity: current 32-byte versus 256-byte chunks",
        "Cases compared: current with 32-byte bitmap chunks versus current with 256-byte bitmap chunks.",
        [
            ("current, chunk=32", "32-byte bitmap chunks", current_32),
            ("current, chunk=256", "256-byte bitmap chunks", current_256),
        ],
    )


if __name__ == "__main__":
    main()
