#!/usr/bin/env bash
# Compare speculative overwrites, inserts, and deletions on the same seeded QMDB state.
set -euo pipefail

usage() {
    echo "usage: $0 <10000|100000|1000000> [repeats]" >&2
    exit 2
}

[ "$#" -ge 1 ] && [ "$#" -le 2 ] || usage
updates="$1"
case "$updates" in
    10000|100000|1000000) ;;
    *) usage ;;
esac
repeats="${2:-1}"
[[ "$repeats" =~ ^[1-9][0-9]{0,2}$ ]] || usage
command -v jq >/dev/null 2>&1 || { echo "jq is required" >&2; exit 1; }

measurements_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
repo_root="$(cd "$measurements_root/.." && pwd)"
# Honor a suite stop request before launching any write-type benchmarks.
for request in "$measurements_root"/results/suites/*/stop-after-writesize; do
    [ -f "$request" ] || continue
    suite_dir="$(dirname "$request")"
    if [ "$(cat "$suite_dir/runner.pid")" = "$PPID" ]; then
        echo "Requested stop after write-size sweep; write-type and skew are suspended."
        exit 75
    fi
done
cd "$repo_root"
source "$measurements_root/scripts/experiment-configs.sh"

# Reuse the writesize build: it has the same feature set and compiler flags.
target_dir="$repo_root/target/measurements/writesize-build"
lock_dir="$target_dir/.run-lock"
mkdir -p "$target_dir"
if ! mkdir "$lock_dir" 2>/dev/null; then
    echo "another write-type run holds $lock_dir" >&2
    exit 1
fi

run_dir=""
archive_raw() {
    if [ -n "$run_dir" ] && [ -d "$target_dir/criterion" ]; then
        destination="$run_dir/criterion"
        if [ "$#" -eq 1 ]; then
            destination="$destination/$1"
        fi
        mkdir -p "$destination"
        cp -R "$target_dir/criterion/." "$destination/"
    fi
}
on_exit() {
    status=$?
    trap - EXIT
    if [ -n "$run_dir" ]; then
        archive_raw || true
        if [ "$status" -ne 0 ]; then
            printf '\nRun exited with status %s; inspect logs and raw output.\n' "$status" >> "$run_dir/notes.md"
        fi
    fi
    rmdir "$lock_dir" || true
    exit "$status"
}
trap on_exit EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

keys="${QMDB_MERKLEIZE_KEYS:-4000000}"
export QMDB_MERKLEIZE_KEYS="$keys"
export CARGO_TARGET_DIR="$target_dir"
export QMDB_MERKLEIZE_UPDATES="$updates"
unset QMDB_MERKLEIZE_SKEW
export RUSTFLAGS="-A deprecated --cfg huge_bench"

timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
for ((repeat = 1; repeat <= repeats; repeat++)); do
    run_id="write-type/${timestamp}-u${updates}-r${repeat}"
    next_run_dir="$measurements_root/results/$run_id"
    if [ -e "$next_run_dir" ]; then
        echo "result directory already exists: $next_run_dir" >&2
        exit 1
    fi
    run_dir="$next_run_dir"
    mkdir -p "$run_dir/criterion"
    rm -rf -- "$target_dir/criterion"

    # Alternate write-type order across process repeats to reduce drift bias.
    case $(( (repeat - 1) % 3 )) in
        0) write_types=(overwrite insert delete) ;;
        1) write_types=(insert delete overwrite) ;;
        2) write_types=(delete overwrite insert) ;;
    esac
    experiment_modes_pairwise "$repeat"

    printf 'unset QMDB_MERKLEIZE_SKEW\n' > "$run_dir/command.txt"
    printf '# %s seeded keys; %s write calls per timed batch; repeat %s\n' "$keys" "$updates" "$repeat" > "$run_dir/notes.md"
    printf '# two configurations (Any unordered vs Current ordered), fixed MMB, 32-byte current chunks, ch=false, s=true, cc=false\n' >> "$run_dir/notes.md"
    printf '# write-type order: %s; configuration order: %s\n' "${write_types[*]}" "${modes[*]}" >> "$run_dir/notes.md"

    for write_type in "${write_types[@]}"; do
        for mode in "${modes[@]}"; do
            experiment_variant "$mode"
            name="qmdb::merkleize/v=${variant} k=${keys} u=${updates} w=${write_type} ch=false s=true cc=false"
            command=(cargo bench -p commonware-storage --features test-traits --bench qmdb -- --exact "$name")
            printf 'RUSTFLAGS=%q QMDB_MERKLEIZE_KEYS=%q QMDB_MERKLEIZE_UPDATES=%q QMDB_MERKLEIZE_WRITE_TYPE=%q CARGO_TARGET_DIR=%q ' \
                "$RUSTFLAGS" "$QMDB_MERKLEIZE_KEYS" "$QMDB_MERKLEIZE_UPDATES" "$write_type" "$CARGO_TARGET_DIR" >> "$run_dir/command.txt"
            printf '%q ' "${command[@]}" >> "$run_dir/command.txt"
            printf '\n' >> "$run_dir/command.txt"
        done
    done
    QMDB_MERKLEIZE_WRITE_TYPE=overwrite,insert,delete \
        "$measurements_root/scripts/write-manifest.sh" "$run_id" criterion \
        "measurements-rain/experiments/write-type/run.sh $updates $repeats"

    for write_type in "${write_types[@]}"; do
        export QMDB_MERKLEIZE_WRITE_TYPE="$write_type"
        for mode in "${modes[@]}"; do
            experiment_variant "$mode"
            name="qmdb::merkleize/v=${variant} k=${keys} u=${updates} w=${write_type} ch=false s=true cc=false"
            echo "Running $mode $write_type: $name"
            # Criterion truncates long report-directory names, which makes the overwrite and
            # insert cases collide at these 4M-key IDs. Give each invocation a fresh output
            # tree and preserve it under an explicit case directory before the next invocation.
            rm -rf -- "$target_dir/criterion"
            cargo bench -p commonware-storage --features test-traits --bench qmdb -- --exact "$name" \
                2>&1 | tee "$run_dir/${mode}-${write_type}.log"
            archive_raw "${mode}-${write_type}"
        done
    done

    "$measurements_root/scripts/summarize-criterion.sh" "$run_id"
    order="$(IFS=,; echo "${modes[*]}")"
    jq -e --arg experiment write-type --argjson keys "$keys" \
        --argjson updates "$updates" --argjson repeat "$repeat" --argjson skew 0 --arg order "$order" \
        -f "$measurements_root/scripts/compare-experiment.jq" \
        "$run_dir/summary.json" > "$run_dir/comparison.json"
    echo "Archived write-type comparison at $run_dir"
    run_dir=""
done
