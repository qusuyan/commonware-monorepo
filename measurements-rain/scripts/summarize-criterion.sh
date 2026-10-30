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

printf 'benchmark,auth_mode,index_order,value_layout,merkle_family,bitmap_chunk_bytes,keyspace_keys,write_calls_per_batch,write_type,write_skew_exponent,input_generation_timed,batch_parent,seed_synced,commonware_page_cache,mean_us,mean_ci_low_us,mean_ci_high_us,median_us,mad_us,sample_count\n' > "$csv"
: > "$json_lines"
while IFS= read -r estimate; do
    new_dir="$(dirname "$estimate")"
    benchmark_json="$new_dir/benchmark.json"
    sample_json="$new_dir/sample.json"
    benchmark="$(jq -r '.full_id' "$benchmark_json")"

    # Expected form:
    # qmdb::merkleize/v=<any|current>::<ordered|unordered>::<fixed|variable>::<mmr|mmb>
    # [chunk=N] k=N [u=N] [w=<overwrite|insert|mixed>] [zipf=S] ch=<bool> s=<bool> cc=<bool>
    if [[ "$benchmark" =~ ^qmdb::merkleize/v=([^[:space:]]+)(\ chunk=([0-9]+))?\ k=([0-9]+)(\ u=([0-9]+))?(\ w=(overwrite|insert|mixed))?(\ zipf=([^[:space:]]+))?\ ch=(true|false)\ s=(true|false)\ cc=(true|false)$ ]]; then
        variant="${BASH_REMATCH[1]}"
        chunk="${BASH_REMATCH[3]:-}"
        keys="${BASH_REMATCH[4]}"
        updates="${BASH_REMATCH[6]:-$((keys / 10))}"
        write_type="${BASH_REMATCH[8]:-overwrite}"
        skew="${BASH_REMATCH[10]:-0}"
        chained="${BASH_REMATCH[11]}"
        synced="${BASH_REMATCH[12]}"
        cache_cleared="${BASH_REMATCH[13]}"
        skew="$("$root/scripts/validate-skew.sh" "$skew")"
        generation_timed=true

        auth_mode="${variant%%::*}"
        remainder="${variant#*::}"
        index_order="${remainder%%::*}"
        remainder="${remainder#*::}"
        value_layout="${remainder%%::*}"
        merkle_family="${remainder##*::}"
    else
        # Leave parsed fields blank for a non-merkleize Criterion target instead of silently
        # inventing a configuration. The full benchmark name remains available in every row.
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
            $samples
        ] | @csv' "$estimate" >> "$csv"

    jq -c \
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
done < <(find "$criterion_dir" -path '*/new/estimates.json' -type f | sort)

jq -s '.' "$json_lines" > "$json"
rm "$json_lines"
