#!/usr/bin/env bash
# Capture native process accounting for one QMDB variant and phase.
set -euo pipefail

if [ "$#" -lt 2 ]; then
    echo "usage: $0 <run-id> --phase merkleize|apply --variant any_unordered_btree|current_ordered --keys N --updates N [--samples N] [--write-type W] [--skew S]" >&2
    exit 2
fi
run_id="$1"
shift
if [[ ! "$run_id" =~ ^[a-zA-Z0-9][a-zA-Z0-9._-]*$ ]]; then
    echo "run-id must contain only letters, digits, dots, underscores or hyphens" >&2
    exit 2
fi
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo_root="$(cd "$root/.." && pwd)"
run_dir="$root/results/$run_id"
args=("$@")
export QMDB_MERKLEIZE_WRITE_TYPE=overwrite
export QMDB_MERKLEIZE_SKEW=0
while [ "$#" -gt 0 ]; do
    if [ "$#" -lt 2 ]; then echo "each option requires a value" >&2; exit 2; fi
    case "$1" in
        --keys|--updates|--samples)
            if [[ ! "$2" =~ ^[1-9][0-9]*$ ]] || [ "${#2}" -gt 20 ] ||
                { [ "${#2}" -eq 20 ] && [[ "$2" > "18446744073709551615" ]]; }; then
                echo "$1 must be a positive u64 without leading zeros" >&2
                exit 2
            fi ;;
    esac
    case "$1" in
        --phase) export QMDB_STUDY_OPERATION="$2" ;;
        --keys) export QMDB_MERKLEIZE_KEYS="$2" ;;
        --updates) export QMDB_MERKLEIZE_UPDATES="$2" ;;
        --write-type) export QMDB_MERKLEIZE_WRITE_TYPE="$2" ;;
        --skew) export QMDB_MERKLEIZE_SKEW="$2" ;;
        --variant|--samples) ;;
        *) echo "unknown option: $1 (--output is managed by this script)" >&2; exit 2 ;;
    esac
    shift 2
done
cd "$repo_root"
mkdir -p "$root/results"
# mkdir atomically rejects an existing run, including partial earlier runs.
mkdir "$run_dir"
command=(cargo bench -p commonware-storage --features test-traits --bench qmdb_cpu -- "${args[@]}" --output "$run_dir/cpu")
printf 'QMDB_SNAPSHOT_REUSE=%q CARGO_TARGET_DIR=%q RUSTFLAGS=%q ' "${QMDB_SNAPSHOT_REUSE:-required}" "${CARGO_TARGET_DIR:-target}" "${RUSTFLAGS:-}" > "$run_dir/command.txt"
printf '%q ' "${command[@]}" >> "$run_dir/command.txt"
printf '\n' >> "$run_dir/command.txt"
"$root/scripts/write-manifest.sh" "$run_id" cpu "$(cat "$run_dir/command.txt")"
"${command[@]}" 2>&1 | tee "$run_dir/output.log"
