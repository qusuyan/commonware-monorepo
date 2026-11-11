#!/usr/bin/env bash
# Run the focused 4-aspect controlled B-Tree CPU matrix via qmdb_cpu.
set -euo pipefail

usage() {
    echo "usage: $0 <suite-id> [--phase merkleize|apply] [--repeats N] [--samples N] [--smoke]" >&2
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
repeats=""
samples=""
smoke=false
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
        --smoke)
            smoke=true
            shift
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

if [ "$smoke" = true ]; then
    repeats="${repeats:-1}"
    samples="${samples:-2}"
    anchor_keys=10000
    anchor_updates=1000
    scale_keys=(5000 10000)
    export QMDB_SNAPSHOT_REUSE=off
else
    repeats="${repeats:-3}"
    samples="${samples:-10}"
    anchor_keys=4000000
    anchor_updates=2000
    scale_keys=(1000000 10000000)
fi

[[ "$repeats" =~ ^[1-9][0-9]{0,2}$ ]] || { echo "--repeats must be 1 through 999" >&2; exit 2; }
[[ "$samples" =~ ^[1-9][0-9]{0,3}$ ]] || { echo "--samples must be positive integer" >&2; exit 2; }
command -v jq >/dev/null 2>&1 || { echo "jq is required" >&2; exit 1; }

cpu_matrix_samples="$samples"
cpu_matrix_total_steps=4

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$script_dir/cpu-matrix-common.sh"
cpu_matrix_python >/dev/null

cpu_matrix_init "focused"
cpu_matrix_setup_traps

if [ "$smoke" = false ]; then
    experiment_preflight_snapshots 1000000 4000000 10000000
fi

run_aspect_writesize() {
    local r
    for ((r = 1; r <= repeats; r++)); do
        cpu_matrix_run_pair writesize "$anchor_keys" "$anchor_updates" overwrite 0 "$r"
    done
}

run_aspect_writetype() {
    local wt r
    for wt in overwrite insert delete; do
        for ((r = 1; r <= repeats; r++)); do
            cpu_matrix_run_pair write-type "$anchor_keys" "$anchor_updates" "$wt" 0 "$r"
        done
    done
}

run_aspect_skew() {
    local s r
    for s in 0 2; do
        for ((r = 1; r <= repeats; r++)); do
            cpu_matrix_run_pair skew "$anchor_keys" "$anchor_updates" overwrite "$s" "$r"
        done
    done
}

run_aspect_dbscale() {
    local k r
    for k in "${scale_keys[@]}"; do
        for ((r = 1; r <= repeats; r++)); do
            cpu_matrix_run_pair db-scale "$k" "$anchor_updates" overwrite 0 "$r"
        done
    done
}

echo "Focused QMDB CPU matrix ($cpu_matrix_phase): any_unordered_btree vs current_ordered (smoke=$smoke)"
cpu_matrix_run_step "1/4 anchor writesize (k=${anchor_keys}, u=${anchor_updates})" run_aspect_writesize
cpu_matrix_run_step "2/4 pure write-type (overwrite/insert/delete)" run_aspect_writetype
cpu_matrix_run_step "3/4 skew endpoints (0 and 2)" run_aspect_skew
cpu_matrix_run_step "4/4 db-scale endpoints (${scale_keys[*]})" run_aspect_dbscale

cpu_matrix_finalize
