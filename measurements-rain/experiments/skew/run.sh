#!/usr/bin/env bash
# Sweep exponents over the controlled B-Tree configuration pair.
set -euo pipefail

usage() {
    echo "usage: $0 [--smoke] [--updates N] [--repeats N] [exponents...]" >&2
    exit 2
}
smoke=false
updates=2000
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
source "$measurements_root/scripts/experiment-configs.sh"
experiment_study_init
for ((i = 0; i < ${#exponents[@]}; i++)); do
    exponents[$i]="$("$measurements_root/scripts/validate-skew.sh" "${exponents[$i]}")"
done

keys="${QMDB_MERKLEIZE_KEYS:-4000000}"
criterion_args=(--sample-size 10)
export RUSTFLAGS="-A deprecated --cfg huge_bench"
if [ "$smoke" = true ]; then
    keys=10000
    export RUSTFLAGS="-A deprecated"
    criterion_args=(--sample-size 10 --warm-up-time 0.1 --measurement-time 0.2 --noplot)
fi
export QMDB_MERKLEIZE_KEYS="$keys"
export CARGO_TARGET_DIR="$repo_root/target/measurements/skew-build"
unset QMDB_MERKLEIZE_WRITE_TYPE QMDB_MERKLEIZE_SKEW
mkdir -p "$CARGO_TARGET_DIR"
lock_dir="$CARGO_TARGET_DIR/.run-lock"
if ! experiment_acquire_lock "$lock_dir" skew; then
    exit 1
fi
trap 'experiment_release_lock "$lock_dir"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
group_id="${study_prefix}skew/$timestamp"
group_dir="$measurements_root/results/$group_id"
if [ -e "$group_dir" ]; then
    echo "result directory already exists: $group_dir" >&2
    exit 1
fi
mkdir -p "$group_dir"
printf '# Write skew experiment\n\nKeys: %s; write calls: %s; process repeats: %s; smoke: %s.\n' \
    "$keys" "$updates" "$repeats" "$smoke" > "$group_dir/notes.md"
printf '\nExponents: %s. Uniform is zero. Inputs are prepared before timing.\n' \
    "${exponents[*]}" >> "$group_dir/notes.md"
printf '\nConfigurations: Any unordered logic on B-Tree and Current ordered; fixed MMB, current chunk=32, ch=false, s=true, cc=false.\n' >> "$group_dir/notes.md"
printf '\nOperation: %s.\n' "$benchmark_operation" >> "$group_dir/notes.md"
summary_paths=()
for ((repeat = 1; repeat <= repeats; repeat++)); do
    experiment_study_modes "$repeat"
    for skew in "${exponents[@]}"; do
        for mode in "${modes[@]}"; do
            experiment_variant "$mode"
            # Rust and jq may spell scientific notation differently. Match the selected setting
            # through its environment value, then verify the numeric exponent in the summary.
            filter="^qmdb::${benchmark_operation}/v=${variant} k=${keys} u=${updates} zipf=[^ ]+ ch=false s=true cc=false$"
            run_id="${group_id}/zipf${skew}-r${repeat}-${mode}"
            if [ -e "$measurements_root/results/$run_id" ]; then
                echo "duplicate exponent or run directory: $run_id" >&2
                exit 2
            fi
            # The previous case is archived before this dedicated build's Criterion data is cleared.
            rm -rf -- "$CARGO_TARGET_DIR/criterion"
            "$measurements_root/scripts/run-criterion.sh" "$run_id" qmdb \
                --updates "$updates" --skew "$skew" --keys "$keys" -- "$filter" "${criterion_args[@]}" "${criterion_study_flags[@]}"
            summary="$measurements_root/results/$run_id/summary.json"
            summary_paths+=("$summary")
        done
        case_summaries=( )
        for mode in "${modes[@]}"; do
            case_summaries+=("$measurements_root/results/${group_id}/zipf${skew}-r${repeat}-${mode}/summary.json")
        done
        order="$(IFS=,; echo "${modes[*]}")"
        jq -s 'add' "${case_summaries[@]}" | jq -e \
            --arg experiment skew --argjson keys "$keys" --argjson updates "$updates" \
            --argjson repeat "$repeat" --argjson skew "$skew" --arg order "$order" \
            -f "$measurements_root/scripts/compare-experiment.jq" \
            > "$group_dir/zipf${skew}-r${repeat}-comparison.json"
    done
done
jq -s 'add' "${summary_paths[@]}" > "$group_dir/summary.json"
echo "Archived skew sweep at $group_dir"
