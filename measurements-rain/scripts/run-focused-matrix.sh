#!/usr/bin/env bash
# Run the reduced controlled B-Tree matrix: anchor plus workload extremes.
set -euo pipefail

if [ "$#" -gt 1 ]; then
    echo "usage: $0 [repeats]" >&2
    exit 2
fi
repeats="${1:-3}"
[[ "$repeats" =~ ^[1-9][0-9]{0,2}$ ]] || { echo "repeats must be 1 through 999" >&2; exit 2; }

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export QMDB_STUDY_OPERATION="${QMDB_STUDY_OPERATION:-merkleize}"
source "$root/scripts/experiment-configs.sh"
experiment_preflight_snapshots 1000000 4000000 10000000

echo "Focused QMDB matrix: any_unordered_btree vs current_ordered"
echo "1/4 anchor write size: 4M keys, 2K uniform overwrites"
"$root/experiments/writesize/run.sh" 2000 "$repeats"

echo "2/4 pure write types: 4M keys, 2K writes"
"$root/experiments/write-type/run.sh" 2000 "$repeats"

echo "3/4 skew endpoints: 4M keys, 2K writes, Zipf 0 and 2"
"$root/experiments/skew/run.sh" --updates 2000 --repeats "$repeats" 0 2

echo "4/4 database-size endpoints: 1M and 10M keys, 2K writes"
"$root/experiments/db-scale/run.sh" --updates 2000 --repeats "$repeats" 1000000 10000000

echo "Focused matrix complete. Results are under $root/results/."
