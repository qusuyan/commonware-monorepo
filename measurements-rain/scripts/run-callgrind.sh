#!/usr/bin/env bash
# Run the repository's Gungraun tracking suite and archive its output.
set -euo pipefail

if [ "$#" -ne 1 ]; then
    echo "usage: $0 <run-id>" >&2
    exit 2
fi

run_id="$1"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
run_dir="$root/results/$run_id"
mkdir -p "$run_dir/callgrind"

"$root/scripts/write-manifest.sh" "$run_id" "callgrind" "just benchmark-tracking generate"
just benchmark-tracking generate

if [ -d benchmark-tracking-results ]; then
    cp -R benchmark-tracking-results/. "$run_dir/callgrind/"
fi
