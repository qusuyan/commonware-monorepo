#!/usr/bin/env bash
# Sweep database sizes across the controlled B-Tree configuration pair.
set -euo pipefail

usage() {
    echo "usage: $0 [--updates N] [--repeats N] [keys...]" >&2
    exit 2
}

updates=2000
repeats=3
keys_list=()

while [ "$#" -gt 0 ]; do
    case "$1" in
        --updates)
            [ "$#" -ge 2 ] || usage
            updates="$2"
            shift 2
            ;;
        --repeats)
            [ "$#" -ge 2 ] || usage
            repeats="$2"
            shift 2
            ;;
        --*) usage ;;
        *) keys_list+=("$1"); shift ;;
    esac
done

[[ "$updates" =~ ^[1-9][0-9]*$ ]] || usage
[[ "$repeats" =~ ^[1-9][0-9]{0,2}$ ]] || usage
if [ "${#keys_list[@]}" -eq 0 ]; then
    keys_list=(1000000 2000000 4000000 10000000)
fi

measurements_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
repo_root="$(cd "$measurements_root/.." && pwd)"
cd "$repo_root"
source "$measurements_root/scripts/experiment-configs.sh"
experiment_study_init

target_dir="$repo_root/target/measurements/db-scale-build"
lock_dir="$target_dir/.run-lock"
mkdir -p "$target_dir"
if ! experiment_acquire_lock "$lock_dir" database-scale; then
    exit 1
fi

trap 'experiment_release_lock "$lock_dir"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

export CARGO_TARGET_DIR="$target_dir"
export QMDB_MERKLEIZE_UPDATES="$updates"
export RUSTFLAGS="-A deprecated --cfg huge_bench"
unset QMDB_MERKLEIZE_WRITE_TYPE
unset QMDB_MERKLEIZE_SKEW

timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
group_id="${study_prefix}db-scale/$timestamp"
group_dir="$measurements_root/results/$group_id"
mkdir -p "$group_dir"

printf '# Database scale experiment\n\nKeys: %s; write calls: %s; process repeats: %s.\n' \
    "${keys_list[*]}" "$updates" "$repeats" > "$group_dir/notes.md"
printf '\nConfigurations: Any unordered logic on B-Tree and Current ordered; fixed MMB, current chunk=32, ch=false, s=true, cc=false.\n' >> "$group_dir/notes.md"

printf '\nOperation: %s.\n' "$benchmark_operation" >> "$group_dir/notes.md"
summary_paths=()
for ((repeat = 1; repeat <= repeats; repeat++)); do
    experiment_study_modes "$repeat"
    for k in "${keys_list[@]}"; do
        export QMDB_MERKLEIZE_KEYS="$k"
        case_summaries=()
        for mode in "${modes[@]}"; do
            experiment_variant "$mode"
            name="qmdb::${benchmark_operation}/v=${variant} k=${k} u=${updates} ch=false s=true cc=false"
            run_id="${group_id}/k${k}-r${repeat}-${mode}"
            rm -rf -- "$target_dir/criterion"
            "$measurements_root/scripts/run-criterion.sh" "$run_id" qmdb \
                --updates "$updates" --keys "$k" -- --exact "$name" "${criterion_study_flags[@]}"
            summary="$measurements_root/results/$run_id/summary.json"
            summary_paths+=("$summary")
            case_summaries+=("$summary")
        done
        order="$(IFS=,; echo "${modes[*]}")"
        jq -s 'add' "${case_summaries[@]}" | jq -e \
            --arg experiment db-scale --argjson keys "$k" --argjson updates "$updates" \
            --argjson repeat "$repeat" --argjson skew 0 --arg order "$order" \
            -f "$measurements_root/scripts/compare-experiment.jq" \
            > "$group_dir/k${k}-r${repeat}-comparison.json"
    done
done

jq -s 'add' "${summary_paths[@]}" > "$group_dir/summary.json"
echo "Archived database scale sweep at $group_dir"
