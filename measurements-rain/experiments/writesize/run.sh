#!/usr/bin/env bash
# Measure the controlled Any-unordered-B-Tree/Current-ordered pair per repeat.
set -euo pipefail

usage() {
    echo "usage: $0 <200|500|1000|1500|2000|5000|7000|8000|10000> [repeats]" >&2
    exit 2
}

[ "$#" -ge 1 ] && [ "$#" -le 2 ] || usage
updates="$1"
case "$updates" in
    200|500|1000|1500|2000|5000|7000|8000|10000) ;;
    *) usage ;;
esac
repeats="${2:-1}"
[[ "$repeats" =~ ^[1-9][0-9]{0,2}$ ]] || usage
command -v jq >/dev/null 2>&1 || { echo "jq is required" >&2; exit 1; }

measurements_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
repo_root="$(cd "$measurements_root/.." && pwd)"
cd "$repo_root"
source "$measurements_root/scripts/experiment-configs.sh"
experiment_study_init

# A shared build directory avoids recompilation across variants and write counts. Only its
# Criterion data is cleared between groups; each group is archived first.
target_dir="$repo_root/target/measurements/writesize-build"
lock_dir="$target_dir/.run-lock"
mkdir -p "$target_dir"
if ! experiment_acquire_lock "$lock_dir" write-size; then
    exit 1
fi

run_dir=""
source_paths=(
    storage/src/qmdb/benches/merkleize.rs
    storage/src/qmdb/benches/common.rs
    storage/src/qmdb/benches/apply_study.rs
    storage/src/qmdb/benches/snapshots.rs
    storage/src/qmdb/benches/merkleize_config.rs
    bench/src/lib.rs
    storage/Cargo.toml
    Cargo.lock
)
capture_source_hashes() {
    for source in "${source_paths[@]}"; do
        printf '%s  %s\n' "$(git hash-object "$source")" "$source"
    done
}
archive_raw() {
    if [ -n "$run_dir" ] && [ -d "$target_dir/criterion" ]; then
        destination="$run_dir/criterion${1:+/$1}"
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
            printf '\nRun exited with status %s; inspect the case logs and archived raw output.\n' "$status" >> "$run_dir/notes.md"
        fi
    fi
    experiment_release_lock "$lock_dir"
    exit "$status"
}
trap on_exit EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

keys="${QMDB_MERKLEIZE_KEYS:-4000000}"
export QMDB_MERKLEIZE_KEYS="$keys"
export CARGO_TARGET_DIR="$target_dir"
export QMDB_MERKLEIZE_UPDATES="$updates"
export RUSTFLAGS="-A deprecated --cfg huge_bench"
unset QMDB_MERKLEIZE_WRITE_TYPE
unset QMDB_MERKLEIZE_SKEW

timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
for ((repeat = 1; repeat <= repeats; repeat++)); do
    run_id="${study_prefix}writesize/${timestamp}-u${updates}-r${repeat}"
    next_run_dir="$measurements_root/results/$run_id"
    if [ -e "$next_run_dir" ]; then
        echo "result directory already exists: $next_run_dir" >&2
        exit 1
    fi
    run_dir="$next_run_dir"
    mkdir -p "$run_dir/criterion"
    rm -rf -- "$target_dir/criterion"

    experiment_study_modes "$repeat"

    printf 'QMDB_STUDY_OPERATION=%q\n' "$QMDB_STUDY_OPERATION" > "$run_dir/command.txt"
    printf 'unset QMDB_MERKLEIZE_WRITE_TYPE QMDB_MERKLEIZE_SKEW\n' >> "$run_dir/command.txt"
    printf '# %s seeded keys; %s write calls per timed batch; repeat %s\n' "$keys" "$updates" "$repeat" > "$run_dir/notes.md"
    printf '# selected configurations, fixed MMB, 32-byte current chunks, ch=false, s=true, cc=false\n' >> "$run_dir/notes.md"
    printf '# operation: %s; configurations: %s\n' "$benchmark_operation" "${modes[*]}" >> "$run_dir/notes.md"
    printf '# configuration order: %s\n' "${modes[*]}" >> "$run_dir/notes.md"
    capture_source_hashes > "$run_dir/source-hashes.txt"

    for mode in "${modes[@]}"; do
        experiment_variant "$mode"
        name="qmdb::${benchmark_operation}/v=${variant} k=${keys} u=${updates} ch=false s=true cc=false"
        command=(cargo bench -p commonware-storage --features test-traits --bench qmdb -- --exact "$name" "${criterion_study_flags[@]}")
        printf 'RUSTFLAGS=%q QMDB_MERKLEIZE_KEYS=%q QMDB_MERKLEIZE_UPDATES=%q CARGO_TARGET_DIR=%q ' \
            "$RUSTFLAGS" "$QMDB_MERKLEIZE_KEYS" "$QMDB_MERKLEIZE_UPDATES" "$CARGO_TARGET_DIR" >> "$run_dir/command.txt"
        printf '%q ' "${command[@]}" >> "$run_dir/command.txt"
        printf '\n' >> "$run_dir/command.txt"
    done
    "$measurements_root/scripts/write-manifest.sh" "$run_id" criterion \
        "measurements-rain/experiments/writesize/run.sh $updates $repeats"

    for mode in "${modes[@]}"; do
        experiment_variant "$mode"
        name="qmdb::${benchmark_operation}/v=${variant} k=${keys} u=${updates} ch=false s=true cc=false"
        echo "Running $mode: $name"
        rm -rf -- "$target_dir/criterion"
        cargo bench -p commonware-storage --features test-traits --bench qmdb -- --exact "$name" "${criterion_study_flags[@]}" \
            2>&1 | tee "$run_dir/$mode.log"
        archive_raw "$mode"
    done

    capture_source_hashes > "$run_dir/source-hashes-after.txt"
    if ! cmp -s "$run_dir/source-hashes.txt" "$run_dir/source-hashes-after.txt"; then
        echo "benchmark source changed during the configuration group" >&2
        exit 1
    fi

    "$measurements_root/scripts/summarize-criterion.sh" "$run_id"
    order="$(IFS=,; echo "${modes[*]}")"
    jq -e --arg experiment writesize --argjson keys "$keys" \
        --argjson updates "$updates" --argjson repeat "$repeat" --argjson skew 0 --arg order "$order" \
        -f "$measurements_root/scripts/compare-experiment.jq" \
        "$run_dir/summary.json" > "$run_dir/comparison.json"
    echo "Archived configuration group at $run_dir"
    run_dir=""
done
