#!/usr/bin/env bash
# Normalize Criterion estimates into portable CSV and JSON using only shell tools and jq.
#
# Criterion shortens long report-directory names. Read the sibling `benchmark.json` rather than
# using the directory name, so this summary preserves every QMDB setting (`s`, `cc`, chunk size,
# and so on) in a readable form.
set -euo pipefail

if [ "$#" -ne 1 ]; then
    echo "usage: $0 <run-id>" >&2
    exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
    echo "jq is required to normalize Criterion JSON estimates" >&2
    exit 1
fi

run_id="$1"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
run_dir="$root/results/$run_id"
criterion_dir="$run_dir/criterion"
csv="$run_dir/summary.csv"
json="$run_dir/summary.json"
json_lines="$run_dir/.summary.ndjson"

# Setup records are emitted outside timing and distinguish fresh/reused measurements.
: > "$run_dir/setup-provenance.ndjson"
for log in "$run_dir"/*.log; do
    [ -f "$log" ] || continue
    jq -Rc 'fromjson? | select(.qmdb_setup == true)' "$log" >> "$run_dir/setup-provenance.ndjson"
done

printf 'benchmark,auth_mode,index_order,value_layout,merkle_family,bitmap_chunk_bytes,keyspace_keys,write_calls_per_batch,write_type,write_skew_exponent,input_generation_timed,batch_parent,seed_synced,commonware_page_cache,mean_us,mean_ci_low_us,mean_ci_high_us,median_us,mad_us,sample_count,operation,timing_scope,durability,iteration_policy\n' > "$csv"
: > "$json_lines"
while IFS= read -r benchmark_json; do
    new_dir="$(dirname "$benchmark_json")"
    estimate="$new_dir/estimates.json"
    # Criterion can leave partial report trees when a long report ID collides. A
    # benchmark descriptor is the authoritative indication that this is a
    # complete measurement, and every complete measurement needs estimates.
    [ -f "$estimate" ] || {
        echo "missing Criterion estimates beside $benchmark_json" >&2
        exit 1
    }
    sample_json="$new_dir/sample.json"
    benchmark="$(jq -r '.full_id' "$benchmark_json")"

    # Expected form:
    # qmdb::merkleize/v=<any|current>::<ordered|unordered|unordered_btree>::<fixed|variable>::<mmr|mmb>
    # [chunk=N] k=N [u=N] [w=<overwrite|insert|delete|mixed>] [zipf=S] ch=<bool> s=<bool> cc=<bool>
    if [[ "$benchmark" =~ ^qmdb::(merkleize|apply_study)/v=([^[:space:]]+)(\ chunk=([0-9]+))?\ k=([0-9]+)(\ u=([0-9]+))?(\ w=(overwrite|insert|delete|mixed))?(\ zipf=([^[:space:]]+))?\ ch=(true|false)\ s=(true|false)\ cc=(true|false)$ ]]; then
        operation="${BASH_REMATCH[1]}"
        if [ "$operation" = apply_study ]; then
            timing_scope=apply_batch
            durability=buffered
            iteration_policy=independent_snapshot_fixed_inputs
        else
            timing_scope=batch_creation_staging_merkleization_root
            durability=speculative
            iteration_policy=speculative_batches
        fi
        variant="${BASH_REMATCH[2]}"
        chunk="${BASH_REMATCH[4]:-}"
        keys="${BASH_REMATCH[5]}"
        updates="${BASH_REMATCH[7]:-$((keys / 10))}"
        write_type="${BASH_REMATCH[9]:-overwrite}"
        skew="${BASH_REMATCH[11]:-0}"
        chained="${BASH_REMATCH[12]}"
        synced="${BASH_REMATCH[13]}"
        cache_cleared="${BASH_REMATCH[14]}"
        skew="$("$root/scripts/validate-skew.sh" "$skew")"
        generation_timed=false

        auth_mode="${variant%%::*}"
        remainder="${variant#*::}"
        index_order="${remainder%%::*}"
        remainder="${remainder#*::}"
        value_layout="${remainder%%::*}"
        merkle_family="${remainder##*::}"
    else
        # Leave parsed fields blank for a non-merkleize Criterion target instead of silently
        # inventing a configuration. The full benchmark name remains available in every row.
        operation=""
        timing_scope=""
        durability=""
        iteration_policy=""
        auth_mode=""
        index_order=""
        value_layout=""
        merkle_family=""
        chunk=""
        keys=""
        updates="null"
        write_type=""
        skew="null"
        generation_timed=null
        chained=""
        synced=""
        cache_cleared=""
    fi

    if [ "$chained" = "true" ]; then
        batch_parent="one pending speculative parent"
    else
        batch_parent="directly from database"
    fi
    if [ "$synced" = "true" ]; then
        seed_synced="yes"
    else
        seed_synced="no"
    fi
    if [ "$cache_cleared" = "true" ]; then
        page_cache="cleared after seed sync"
    else
        page_cache="retained (warm)"
    fi
    samples="$(jq '.iters | length' "$sample_json")"

    jq -r \
        --arg operation "$operation" \
        --arg scope "$timing_scope" \
        --arg durability "$durability" \
        --arg policy "$iteration_policy" \
        --arg benchmark "$benchmark" \
        --arg auth_mode "$auth_mode" \
        --arg index_order "$index_order" \
        --arg value_layout "$value_layout" \
        --arg merkle_family "$merkle_family" \
        --arg chunk "$chunk" \
        --arg keys "$keys" \
        --argjson updates "$updates" \
        --arg write_type "$write_type" \
        --argjson skew "$skew" \
        --argjson generation_timed "$generation_timed" \
        --arg batch_parent "$batch_parent" \
        --arg seed_synced "$seed_synced" \
        --arg page_cache "$page_cache" \
        --argjson samples "$samples" \
        '[
            $benchmark, $auth_mode, $index_order, $value_layout, $merkle_family, $chunk,
            $keys, $updates, $write_type, $skew, $generation_timed, $batch_parent, $seed_synced, $page_cache,
            (.mean.point_estimate / 1000),
            (.mean.confidence_interval.lower_bound / 1000),
            (.mean.confidence_interval.upper_bound / 1000),
            (.median.point_estimate / 1000),
            (.median_abs_dev.point_estimate / 1000),
            $samples, $operation, $scope, $durability, $policy
        ] | @csv' "$estimate" >> "$csv"

    jq -c \
        --arg operation "$operation" \
        --arg scope "$timing_scope" \
        --arg durability "$durability" \
        --arg policy "$iteration_policy" \
        --arg benchmark "$benchmark" \
        --arg auth_mode "$auth_mode" \
        --arg index_order "$index_order" \
        --arg value_layout "$value_layout" \
        --arg merkle_family "$merkle_family" \
        --arg chunk "$chunk" \
        --arg keys "$keys" \
        --argjson updates "$updates" \
        --arg write_type "$write_type" \
        --argjson skew "$skew" \
        --argjson generation_timed "$generation_timed" \
        --arg batch_parent "$batch_parent" \
        --arg seed_synced "$seed_synced" \
        --arg page_cache "$page_cache" \
        --argjson samples "$samples" \
        '{
            benchmark: $benchmark,
            operation: $operation,
            timing_scope: $scope,
            durability: $durability,
            iteration_policy: $policy,
            auth_mode: $auth_mode,
            index_order: $index_order,
            value_layout: $value_layout,
            merkle_family: $merkle_family,
            bitmap_chunk_bytes: $chunk,
            keyspace_keys: $keys,
            write_calls_per_batch: $updates,
            write_type: $write_type,
            write_skew_exponent: $skew,
            input_generation_timed: $generation_timed,
            batch_parent: $batch_parent,
            seed_synced: $seed_synced,
            commonware_page_cache: $page_cache,
            mean_us: (.mean.point_estimate / 1000),
            mean_ci_low_us: (.mean.confidence_interval.lower_bound / 1000),
            mean_ci_high_us: (.mean.confidence_interval.upper_bound / 1000),
            median_us: (.median.point_estimate / 1000),
            mad_us: (.median_abs_dev.point_estimate / 1000),
            sample_count: $samples
        }' "$estimate" >> "$json_lines"
done < <(find "$criterion_dir" -path '*/new/benchmark.json' -type f | sort)

jq -s '.' "$json_lines" > "$json"
rm "$json_lines"
