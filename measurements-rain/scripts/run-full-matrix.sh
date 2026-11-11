#!/usr/bin/env bash
# Run the defined QMDB latency matrix sequentially and preserve enough state for monitoring.
set -euo pipefail

if [ "$#" -ne 1 ] && [ "$#" -ne 3 ]; then
    echo "usage: $0 <suite-id> [--resume-from-step <1-12>]" >&2
    exit 2
fi

suite_id="$1"
resume_from_step=1
if [ "$#" -eq 3 ]; then
    [ "$2" = "--resume-from-step" ] || { echo "unknown option: $2" >&2; exit 2; }
    [[ "$3" =~ ^([1-9]|1[0-2])$ ]] || { echo "resume step must be 1 through 12" >&2; exit 2; }
    resume_from_step="$3"
fi
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo_root="$(cd "$root/.." && pwd)"
source "$root/scripts/experiment-configs.sh"
experiment_preflight_snapshots 1000000 2000000 4000000 10000000
suite_dir="$root/results/suites/$suite_id"
minimum_free_kib=$((20 * 1024 * 1024))
source_paths=(
    storage/src/qmdb/benches/merkleize.rs
    storage/src/qmdb/benches/apply_study.rs
    storage/src/qmdb/benches/snapshots.rs
    storage/src/qmdb/benches/merkleize_config.rs
    storage/src/qmdb/benches/common.rs
    bench/src/lib.rs
    storage/Cargo.toml
    Cargo.lock
)

if [ -e "$suite_dir" ] && [ "$resume_from_step" -eq 1 ]; then
    echo "suite directory already exists: $suite_dir" >&2
    exit 2
fi
mkdir -p "$suite_dir"
exec > >(tee -a "$suite_dir/suite.log") 2>&1
printf '%s\n' "$$" > "$suite_dir/runner.pid"

status_file="$suite_dir/status.json"
completed=$((resume_from_step - 1))
total=12
current="initializing"

write_status() {
    jq -n \
        --arg suite_id "$suite_id" \
        --arg state "$1" \
        --arg current "$current" \
        --argjson completed "$completed" \
        --argjson total "$total" \
        --arg updated_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
        '{suite_id: $suite_id, state: $state, current: $current,
          completed_steps: $completed, total_steps: $total, updated_at_utc: $updated_at}' \
        > "$status_file"
}

check_space() {
    local available_kib
    available_kib="$(df -Pk "$repo_root" | awk 'NR == 2 { print $4 }')"
    if [ "$available_kib" -lt "$minimum_free_kib" ]; then
        echo "available disk space is below 20 GiB: ${available_kib} KiB" >&2
        return 1
    fi
}

capture_source_hashes() {
    for source in "${source_paths[@]}"; do
        printf '%s  %s\n' "$(git hash-object "$source")" "$source"
    done
}

check_sources_unchanged() {
    capture_source_hashes > "$suite_dir/source-hashes-current.txt"
    cmp -s "$suite_dir/source-hashes-at-start.txt" "$suite_dir/source-hashes-current.txt"
}

on_exit() {
    local exit_status=$?
    trap - EXIT
    if [ "$exit_status" -eq 0 ]; then
        current="complete"
        write_status complete
    else
        write_status failed
    fi
    exit "$exit_status"
}
trap on_exit EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

cd "$repo_root"
if [ "$resume_from_step" -eq 1 ]; then
    git status --short > "$suite_dir/git-status-at-start.txt"
    git rev-parse HEAD > "$suite_dir/git-revision-at-start.txt"
    capture_source_hashes > "$suite_dir/source-hashes-at-start.txt"
elif ! check_sources_unchanged; then
    echo "benchmark source changed since the failed suite started" >&2
    exit 1
fi
write_status running

run_step() {
    local label="$1"
    shift
    current="$label"
    write_status running
    check_space
    "$@"
    if ! check_sources_unchanged; then
        echo "benchmark source changed during the suite" >&2
        return 1
    fi
    completed=$((completed + 1))
    current="completed: $label"
    write_status running
}

write_sizes=(200 500 1000 1500 2000 5000 7000 8000 10000)
for index in "${!write_sizes[@]}"; do
    step=$((index + 1))
    if [ "$resume_from_step" -le "$step" ]; then
        updates="${write_sizes[$index]}"
        run_step "writesize u=${updates}" "$root/experiments/writesize/run.sh" "$updates" 3
    fi
done
if [ "$resume_from_step" -le 10 ]; then
    run_step "write-type u=2000" "$root/experiments/write-type/run.sh" 2000 3
fi
if [ "$resume_from_step" -le 11 ]; then
    run_step "skew u=2000" "$root/experiments/skew/run.sh" --updates 2000 --repeats 3
fi
if [ "$resume_from_step" -le 12 ]; then
    run_step "db-scale u=2000" "$root/experiments/db-scale/run.sh" --updates 2000 --repeats 3
fi
current="validating coverage and generating CSV summaries"
write_status running
python3 "$root/scripts/summarize-isolated-suite.py" "$suite_dir"
