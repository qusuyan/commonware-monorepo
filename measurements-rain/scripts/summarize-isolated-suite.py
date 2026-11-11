#!/usr/bin/env python3
"""Validate one completed isolated suite and write the requested aspect CSVs."""

import argparse
import csv
import json
import math
from pathlib import Path
import re
import statistics

MODES = ("any_unordered", "current_unordered", "current_ordered")
SIZES = (200, 500, 1000, 1500, 2000, 5000, 7000, 8000, 10000)
PAIRWISE_MODES = ("any_unordered", "current_ordered")
BTREE_MODES = ("any_unordered_btree", "current_ordered")


def report_modes(report):
    configs = set(report["configurations"])
    if configs == set(BTREE_MODES):
        return BTREE_MODES
    if configs == set(PAIRWISE_MODES):
        return PAIRWISE_MODES
    if configs == set(MODES):
        return MODES
    raise ValueError("unexpected configurations")


def report_write_types(report):
    if report["experiment"] != "write-type":
        return ("overwrite",)
    return (("overwrite", "insert", "delete") if len(report_modes(report)) == 2
            else ("overwrite", "insert"))


def summarize(suite, aspects=None, json_output=None):
    root = Path(__file__).resolve().parents[1]
    log = (suite / "suite.log").read_text()
    paths = re.findall(r"Archived (?:configuration group|write-type comparison|skew sweep|database scale sweep) at (.+)", log)
    if len(paths) != len(set(paths)):
        raise ValueError("duplicate archived groups in suite log")
    reports = {"writesize": [], "write-type": [], "skew": [], "db-scale": []}
    for folder in map(Path, paths):
        folder = folder.resolve()
        if not folder.is_relative_to(root / "results"):
            raise ValueError(f"unexpected result path: {folder}")
        comparisons = ([folder / "comparison.json"] if (folder / "comparison.json").exists()
                       else sorted(folder.glob("*-comparison.json")))
        for path in comparisons:
            report = json.loads(path.read_text())
            kind = report["experiment"]
            if report["schema_version"] != 2:
                raise ValueError(f"invalid configurations: {path}")
            operation = report.get("operation", "merkleize")
            scope = "apply_batch" if operation == "apply_study" else "batch_creation_staging_merkleization_root"
            durability = "buffered" if operation == "apply_study" else "speculative"
            if operation not in ("merkleize", "apply_study") or report.get("timing_scope", scope) != scope or report.get("durability", durability) != durability:
                raise ValueError(f"invalid timing boundary: {path}")
            modes = report_modes(report)
            for mode in modes:
                cases = ([report["configurations"][mode][t] for t in report_write_types(report)]
                         if kind == "write-type" else [report["configurations"][mode]])
                for case in cases:
                    if (case.get("operation", "merkleize") != operation
                            or case.get("timing_scope", scope) != scope
                            or case.get("durability", durability) != durability
                            or (operation == "apply_study" and case.get("iteration_policy") != "independent_snapshot_fixed_inputs")
                            or int(case["keyspace_keys"]) != report["keyspace_keys"]
                            or case["auth_mode"] + "_" + case["index_order"] != mode
                            or case["input_generation_timed"] is not False
                            or case["value_layout"] != "fixed" or case["merkle_family"] != "mmb"
                            or case["bitmap_chunk_bytes"] != ("" if mode.startswith("any_") else "32")
                            or case["seed_synced"] != "yes"
                            or case["commonware_page_cache"] != "retained (warm)"
                            or case["batch_parent"] != "directly from database"
                            or case["write_calls_per_batch"] != report["write_calls_per_batch"]
                            or case["write_skew_exponent"] != report["write_skew_exponent"]
                            or not math.isfinite(case["mean_us"]) or case["mean_us"] <= 0
                            or case["sample_count"] <= 0):
                        raise ValueError(f"mismatched workload: {path}")
            reports[kind].append((path, report))

    operations = {r.get("operation", "merkleize") for entries in reports.values() for _, r in entries}
    if len(operations) > 1:
        raise ValueError("cannot mix operations across suite reports")
    outputs = {}
    selected = set(aspects) if aspects else {kind for kind, entries in reports.items() if entries}
    if not selected:
        raise ValueError("no completed experiment reports")
    for kind, entries in reports.items():
        if kind not in selected:
            continue
        groups = {}
        for path, report in entries:
            groups.setdefault((report["keyspace_keys"], report["write_calls_per_batch"], report["write_skew_exponent"]), []).append((path, report))
        key_counts = {keys for keys, _, _ in groups}
        if kind == "db-scale":
            if key_counts != {1_000_000, 2_000_000, 4_000_000, 10_000_000}:
                raise ValueError("missing or unexpected database scale points")
        elif len(key_counts) != 1:
            raise ValueError("cannot mix baseline key counts")
        counts = {count for _, count, _ in groups}
        if kind == "writesize":
            expected = {(n, 0) for n in SIZES}
        else:
            if len(counts) != 1:
                raise ValueError("cannot mix write counts within a fixed-size sweep")
            count = next(iter(counts))
            expected = ({(count, s) for s in (0, .5, 1, 1.5, 2)}
                        if kind == "skew" else {(count, 0)})
        if {(count, skew) for _, count, skew in groups} != expected:
            raise ValueError(f"missing or unexpected workload points: {kind}")
        rows = []
        for (keys, count, skew), runs in sorted(groups.items()):
            if len(runs) != 3 or {r["repeat"] for _, r in runs} != {1, 2, 3}:
                raise ValueError(f"missing or duplicate process repeats: {kind} {count} {skew}")
            modes = report_modes(runs[0][1])
            write_types = report_write_types(runs[0][1])
            if any(report_modes(r) != modes or report_write_types(r) != write_types for _, r in runs):
                raise ValueError("cannot mix configuration/workload sets across repeats")
            for write_type in write_types:
                row = dict(keyspace_keys=keys, write_calls_per_batch=count,
                           write_type=write_type, write_skew_exponent=skew, process_repeats=3,
                           operation=runs[0][1].get("operation", "merkleize"),
                           timing_scope=runs[0][1].get("timing_scope", "batch_creation_staging_merkleization_root"),
                           durability=runs[0][1].get("durability", "speculative"),
                           source_comparisons=";".join(str(p) for p, _ in runs))

                def add_stats(name, values):
                    row[name + "_mean"] = statistics.mean(values)
                    row[name + "_sd"] = statistics.stdev(values)

                cases = [{mode: (r["configurations"][mode][write_type] if kind == "write-type"
                                 else r["configurations"][mode]) for mode in modes} for _, r in runs]
                if any(c[mode]["write_type"] != write_type for c in cases for mode in modes):
                    raise ValueError("misidentified write type")
                for mode in modes:
                    add_stats(mode + "_ms", [c[mode]["mean_us"] / 1000 for c in cases])
                for base, target in ((modes[i], modes[j]) for i in range(len(modes)) for j in range(i + 1, len(modes))):
                    name = target + "_minus_" + base
                    add_stats(name + "_ms", [(c[target]["mean_us"] - c[base]["mean_us"]) / 1000 for c in cases])
                    add_stats(name + "_percent", [100 * (c[target]["mean_us"] / c[base]["mean_us"] - 1) for c in cases])
                if kind == "write-type":
                    for mode in modes:
                        pairs = [r["configurations"][mode] for _, r in runs]
                        for mutation in write_types[1:]:
                            add_stats(mode + "_" + mutation + "_minus_overwrite_ms", [
                                (p[mutation]["mean_us"] - p["overwrite"]["mean_us"]) / 1000 for p in pairs])
                            add_stats(mode + "_" + mutation + "_vs_overwrite_percent", [
                                100 * (p[mutation]["mean_us"] / p["overwrite"]["mean_us"] - 1) for p in pairs])
                rows.append(row)
        outputs[kind] = rows

    if json_output is not None:
        json_output.write_text(json.dumps(outputs, indent=2) + "\n")
        return

    # Validate every selected aspect before publishing. Existing archives are never modified.
    for kind, rows in outputs.items():
        folder = root / "results" / (kind.replace("-", "") + "-" + rows[0]["operation"] + "-summary")
        folder.mkdir(exist_ok=True)
        destination = folder / "summary.csv"
        temporary = folder / "summary.csv.tmp"
        with temporary.open("w", newline="") as stream:
            writer = csv.DictWriter(stream, fieldnames=list(rows[0]))
            writer.writeheader()
            writer.writerows(rows)
        temporary.replace(destination)
        print(f"Wrote {len(rows)} rows to {destination}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("suite_dir", type=Path)
    parser.add_argument("--aspects", nargs="+", choices=("writesize", "write-type", "skew", "db-scale"))
    parser.add_argument("--json-output", type=Path, help="write intermediate rows instead of CSVs")
    args = parser.parse_args()
    summarize(args.suite_dir.resolve(), args.aspects, args.json_output)
