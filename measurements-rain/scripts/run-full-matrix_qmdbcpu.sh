#!/usr/bin/env bash
# Run the full 12-step controlled B-Tree CPU matrix via qmdb_cpu with resume support.
set -euo pipefail

usage() {
    echo "usage: $0 <suite-id> [--phase merkleize|apply] [--repeats N] [--samples N] [--resume-from-step <1-12>]" >&2
    exit 2
}

if [ "$#" -lt 1 ]; then
    usage
fi

cpu_matrix_suite_id="$1"
shift
[[ "$cpu_matrix_suite_id" =~ ^[a-zA-Z0-9][a-zA-Z0-9._-]*$ ]] || {
    echo "suite-id must contain only letters, digits, dots, underscores or hyphens" >&2
    exit 2
}

cpu_matrix_phase="${QMDB_STUDY_OPERATION:-merkleize}"
repeats=3
samples=10
cpu_matrix_resume_step=1

while [ "$#" -gt 0 ]; do
    case "$1" in
        --phase)
            [ "$#" -ge 2 ] || usage
            cpu_matrix_phase="$2"
            shift 2
            ;;
        --repeats)
            [ "$#" -ge 2 ] || usage
            repeats="$2"
            shift 2
            ;;
        --samples)
            [ "$#" -ge 2 ] || usage
            samples="$2"
            shift 2
            ;;
        --resume-from-step)
            [ "$#" -ge 2 ] || usage
            cpu_matrix_resume_step="$2"
            shift 2
            ;;
        *)
            echo "unknown option: $1" >&2
            usage
            ;;
    esac
done

case "$cpu_matrix_phase" in
    merkleize|apply) ;;
    *) echo "--phase must be merkleize or apply" >&2; exit 2 ;;
esac
[[ "$repeats" =~ ^[1-9][0-9]{0,2}$ ]] || { echo "--repeats must be 1 through 999" >&2; exit 2; }
[[ "$samples" =~ ^[1-9][0-9]{0,3}$ ]] || { echo "--samples must be positive integer" >&2; exit 2; }
[[ "$cpu_matrix_resume_step" =~ ^([1-9]|1[0-2])$ ]] || {
    echo "--resume-from-step must be 1 through 12" >&2
    exit 2
}
command -v jq >/dev/null 2>&1 || { echo "jq is required" >&2; exit 1; }

cpu_matrix_samples="$samples"
cpu_matrix_total_steps=12

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$script_dir/cpu-matrix-common.sh"
cpu_matrix_python >/dev/null

cpu_matrix_init "full"
cpu_matrix_setup_traps

experiment_preflight_snapshots 1000000 2000000 4000000 10000000

run_writesize_step() {
    local u="$1" r
    for ((r = 1; r <= repeats; r++)); do
        cpu_matrix_run_pair writesize 4000000 "$u" overwrite 0 "$r"
    done
}

run_writetype_step() {
    local wt r
    for wt in overwrite insert delete; do
        for ((r = 1; r <= repeats; r++)); do
            cpu_matrix_run_pair write-type 4000000 2000 "$wt" 0 "$r"
        done
    done
}

run_skew_step() {
    local s r
    for s in 0 0.5 1 1.5 2; do
        for ((r = 1; r <= repeats; r++)); do
            cpu_matrix_run_pair skew 4000000 2000 overwrite "$s" "$r"
        done
    done
}

run_dbscale_step() {
    local k r
    for k in 1000000 2000000 4000000 10000000; do
        for ((r = 1; r <= repeats; r++)); do
            cpu_matrix_run_pair db-scale "$k" 2000 overwrite 0 "$r"
        done
    done
}

write_sizes=(200 500 1000 1500 2000 5000 7000 8000 10000)
for index in "${!write_sizes[@]}"; do
    step=$((index + 1))
    if [ "$cpu_matrix_resume_step" -le "$step" ]; then
        updates="${write_sizes[$index]}"
        cpu_matrix_run_step "writesize u=${updates}" run_writesize_step "$updates"
    fi
done

if [ "$cpu_matrix_resume_step" -le 10 ]; then
    cpu_matrix_run_step "write-type u=2000" run_writetype_step
fi
if [ "$cpu_matrix_resume_step" -le 11 ]; then
    cpu_matrix_run_step "skew u=2000" run_skew_step
fi
if [ "$cpu_matrix_resume_step" -le 12 ]; then
    cpu_matrix_run_step "db-scale u=2000" run_dbscale_step
fi

cpu_matrix_finalize
