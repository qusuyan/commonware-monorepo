#!/usr/bin/env bash
# Check exponent validation, legacy/new result parsing, and launcher argument handling.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root/.."
run_id="skew-parser-test-$$"
run_dir="$root/results/$run_id"
mkdir -p "$run_dir/criterion"
trap 'rm -rf -- "$run_dir"' EXIT

for value in 0 0.5 0.75 1 1.5 2 3 1e-8; do
    "$root/scripts/validate-skew.sh" "$value" >/dev/null
done
for invalid in '' bad -1 NaN inf 1e9999; do
    if "$root/scripts/validate-skew.sh" "$invalid" >/dev/null 2>&1; then
        echo "accepted invalid skew: $invalid" >&2
        exit 1
    fi
    if "$root/scripts/run-criterion.sh" "$run_id" qmdb --skew "$invalid" >/dev/null 2>&1; then
        echo "launcher accepted invalid skew: $invalid" >&2
        exit 1
    fi
done

add_case() {
    local case_dir="$run_dir/criterion/$1/new"
    mkdir -p "$case_dir"
    jq -n --arg name "$2" '{full_id: $name}' > "$case_dir/benchmark.json"
    printf '{"iters":[1,2,3]}\n' > "$case_dir/sample.json"
    jq -n '{mean:{point_estimate:1000,confidence_interval:{lower_bound:900,upper_bound:1100}},
            median:{point_estimate:1000},median_abs_dev:{point_estimate:10}}' \
        > "$case_dir/estimates.json"
}
base='qmdb::merkleize/v=any::unordered::fixed::mmb k=10000'
add_case legacy "$base ch=false s=true cc=false"
add_case zero "$base u=1000 zipf=0 ch=false s=true cc=false"
add_case fractional "$base u=1000 w=insert zipf=0.75 ch=false s=true cc=false"
add_case large "$base u=1000 w=mixed zipf=3 ch=true s=false cc=false"
add_case scientific "$base u=1000 zipf=1e-8 ch=false s=true cc=false"
add_case component 'qmdb::component/component=bitmap_mutation family=na keys=0 updates=1024 locality=repeated chunk=32'
"$root/scripts/summarize-criterion.sh" "$run_id"
jq -e '
    length == 6 and
    all(.[] | select(.auth_mode == "any"); .input_generation_timed == true and .write_calls_per_batch == 1000) and
    ([.[] | select(.benchmark | contains("zipf=0.75"))][0] |
        .write_skew_exponent == 0.75 and .write_type == "insert") and
    ([.[] | select(.benchmark | contains("zipf=3 "))][0] |
        .write_skew_exponent == 3 and .write_type == "mixed" and .batch_parent == "one pending speculative parent") and
    ([.[] | select(.benchmark | contains("zipf=1e-8"))][0].write_skew_exponent == 1e-8) and
    ([.[] | select(.benchmark | contains(" zipf=") | not) | select(.auth_mode == "any")][0].write_skew_exponent == 0) and
    ([.[] | select(.benchmark | startswith("qmdb::component/"))][0] |
        .write_skew_exponent == null and .input_generation_timed == null)
' "$run_dir/summary.json" >/dev/null
echo 'Skew validation and legacy/new summary parsing passed'
