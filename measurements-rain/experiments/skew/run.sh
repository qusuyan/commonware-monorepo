#!/usr/bin/env bash
# Sweep script-selected exponents over matched any/current speculative overwrite cases.
set -euo pipefail

usage() {
    echo "usage: $0 [--smoke] [--updates N] [--repeats N] [exponents...]" >&2
    exit 2
}
smoke=false
updates=1000
repeats=1
exponents=()
while [ "$#" -gt 0 ]; do
    case "$1" in
        --smoke) smoke=true; shift ;;
        --updates|--repeats)
            [ "$#" -ge 2 ] || usage
            if [ "$1" = --updates ]; then updates="$2"; else repeats="$2"; fi
            shift 2
            ;;
        --*) usage ;;
        *) exponents+=("$1"); shift ;;
    esac
done
[[ "$updates" =~ ^[1-9][0-9]*$ ]] || usage
if [ "${#updates}" -gt 20 ] ||
   { [ "${#updates}" -eq 20 ] && [[ "$updates" > "18446744073709551615" ]]; }; then usage; fi
[[ "$repeats" =~ ^[1-9][0-9]{0,2}$ ]] || usage
if [ "${#exponents[@]}" -eq 0 ]; then exponents=(0 0.5 1 1.5 2); fi

measurements_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
repo_root="$(cd "$measurements_root/.." && pwd)"
cd "$repo_root"
for ((i = 0; i < ${#exponents[@]}; i++)); do
    exponents[$i]="$("$measurements_root/scripts/validate-skew.sh" "${exponents[$i]}")"
done

keys=10000000
criterion_args=(--sample-size 10)
export RUSTFLAGS="-A deprecated --cfg huge_bench"
if [ "$smoke" = true ]; then
    keys=10000
    export RUSTFLAGS="-A deprecated"
    criterion_args=(--sample-size 10 --warm-up-time 0.1 --measurement-time 0.2 --noplot)
fi
export CARGO_TARGET_DIR="$repo_root/target/measurements/skew-build"
unset QMDB_MERKLEIZE_WRITE_TYPE QMDB_MERKLEIZE_SKEW
mkdir -p "$CARGO_TARGET_DIR"
lock_dir="$CARGO_TARGET_DIR/.run-lock"
if ! mkdir "$lock_dir" 2>/dev/null; then
    echo "another skew run holds $lock_dir" >&2
    exit 1
fi
trap 'rmdir "$lock_dir"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
group_id="skew/$timestamp"
group_dir="$measurements_root/results/$group_id"
if [ -e "$group_dir" ]; then
    echo "result directory already exists: $group_dir" >&2
    exit 1
fi
mkdir -p "$group_dir"
printf '# Write skew experiment\n\nKeys: %s; write calls: %s; process repeats: %s; smoke: %s.\n' \
    "$keys" "$updates" "$repeats" "$smoke" > "$group_dir/notes.md"
printf '\nExponents: %s. Uniform is zero. Input generation is timed.\n' \
    "${exponents[*]}" >> "$group_dir/notes.md"
printf '\nCases: unordered fixed MMB, current chunk=32, ch=false, s=true, cc=false.\n' >> "$group_dir/notes.md"
summary_paths=()
for ((repeat = 1; repeat <= repeats; repeat++)); do
    modes=(any current)
    if ((repeat % 2 == 0)); then modes=(current any); fi
    for skew in "${exponents[@]}"; do
        for mode in "${modes[@]}"; do
            variant="any::unordered::fixed::mmb"
            if [ "$mode" = current ]; then variant="current::unordered::fixed::mmb chunk=32"; fi
            # Rust and jq may spell scientific notation differently. Match the selected setting
            # through its environment value, then verify the numeric exponent in the summary.
            filter="^qmdb::merkleize/v=${variant} k=${keys} u=${updates} zipf=[^ ]+ ch=false s=true cc=false$"
            run_id="${group_id}/zipf${skew}-r${repeat}-${mode}"
            if [ -e "$measurements_root/results/$run_id" ]; then
                echo "duplicate exponent or run directory: $run_id" >&2
                exit 2
            fi
            # The previous case is archived before this dedicated build's Criterion data is cleared.
            rm -rf -- "$CARGO_TARGET_DIR/criterion"
            "$measurements_root/scripts/run-criterion.sh" "$run_id" qmdb \
                --updates "$updates" --skew "$skew" -- "$filter" "${criterion_args[@]}"
            summary="$measurements_root/results/$run_id/summary.json"
            jq -e --arg mode "$mode" --argjson skew "$skew" --argjson keys "$keys" \
                --argjson updates "$updates" '
                length == 1 and .[0].auth_mode == $mode and
                (.[0].keyspace_keys | tonumber) == $keys and
                .[0].write_calls_per_batch == $updates and
                .[0].write_skew_exponent == $skew and
                .[0].write_type == "overwrite" and .[0].input_generation_timed == true and
                .[0].batch_parent == "directly from database" and
                .[0].merkle_family == "mmb" and .[0].mean_us > 0
            ' "$summary" >/dev/null
            summary_paths+=("$summary")
        done
    done
done
jq -s 'add' "${summary_paths[@]}" > "$group_dir/summary.json"
echo "Archived skew sweep at $group_dir"
