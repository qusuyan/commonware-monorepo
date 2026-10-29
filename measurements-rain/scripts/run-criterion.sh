#!/usr/bin/env bash
# Run a Criterion QMDB benchmark and archive its raw output with run metadata.
set -euo pipefail

if [ "$#" -lt 2 ]; then
    echo "usage: $0 <run-id> <bench-target> [--updates N] [--skew S] [criterion arguments...]" >&2
    exit 2
fi

run_id="$1"
bench="$2"
shift 2
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
unset QMDB_MERKLEIZE_UPDATES
while [ "$#" -gt 0 ]; do
    case "$1" in
        --updates|--skew)
            if [ "$bench" != "qmdb" ] || [ "$#" -lt 2 ]; then
                echo "$1 requires the qmdb target and a value" >&2
                exit 2
            fi
            if [ "$1" = --skew ]; then
                export QMDB_MERKLEIZE_SKEW="$2"
            else
                updates="$2"
                if [[ ! "$updates" =~ ^[1-9][0-9]*$ ]] ||
                   [ "${#updates}" -gt 20 ] ||
                   { [ "${#updates}" -eq 20 ] && [[ "$updates" > "18446744073709551615" ]]; }; then
                    echo "--updates must be a positive u64" >&2
                    exit 2
                fi
                export QMDB_MERKLEIZE_UPDATES="$updates"
            fi
            shift 2
            ;;
        --) shift; break ;;
        *) break ;;
    esac
done
if [ "${QMDB_MERKLEIZE_SKEW+x}" = x ]; then
    QMDB_MERKLEIZE_SKEW="$("$root/scripts/validate-skew.sh" "$QMDB_MERKLEIZE_SKEW")"
    export QMDB_MERKLEIZE_SKEW
fi
run_dir="$root/results/$run_id"
repo_root="$(git rev-parse --show-toplevel)"
target_dir="${CARGO_TARGET_DIR:-$repo_root/target/measurements/$run_id}"
mkdir -p "$run_dir/criterion"

command=(cargo bench -p commonware-storage --features test-traits --bench "$bench" -- "$@")
: > "$run_dir/command.txt"
printf 'CARGO_TARGET_DIR=%q RUSTFLAGS=%q ' "$target_dir" "${RUSTFLAGS:-}" >> "$run_dir/command.txt"
if [ -n "${QMDB_MERKLEIZE_UPDATES:-}" ]; then
    printf 'QMDB_MERKLEIZE_UPDATES=%q ' "$QMDB_MERKLEIZE_UPDATES" >> "$run_dir/command.txt"
fi
if [ "${QMDB_MERKLEIZE_SKEW+x}" = x ]; then
    printf 'QMDB_MERKLEIZE_SKEW=%q ' "$QMDB_MERKLEIZE_SKEW" >> "$run_dir/command.txt"
fi
if [ "${QMDB_MERKLEIZE_WRITE_TYPE+x}" = x ]; then
    printf 'QMDB_MERKLEIZE_WRITE_TYPE=%q ' "$QMDB_MERKLEIZE_WRITE_TYPE" >> "$run_dir/command.txt"
fi
printf '%q ' "${command[@]}" >> "$run_dir/command.txt"
printf '\n' >> "$run_dir/command.txt"

"$root/scripts/write-manifest.sh" "$run_id" "criterion" "$(cat "$run_dir/command.txt")"
CARGO_TARGET_DIR="$target_dir" "${command[@]}" 2>&1 | tee "$run_dir/output.log"

if [ -d "$target_dir/criterion" ]; then
    cp -R "$target_dir/criterion/." "$run_dir/criterion/"
fi

"$root/scripts/summarize-criterion.sh" "$run_id"
