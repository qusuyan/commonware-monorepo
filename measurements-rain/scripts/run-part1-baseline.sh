#!/usr/bin/env bash
# Launch one explicitly requested Part 1 baseline family and preserve each run separately.
set -euo pipefail

if [ "$#" -lt 2 ]; then
    echo "usage: $0 <run-prefix> <merkleize|constantinople|chained-growth> [standard|full|huge] [--updates N] [--skew S]" >&2
    exit 2
fi

prefix="$1"
suite="$2"
shift 2
tier="standard"
if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
    tier="$1"
    shift
fi
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
options=()
suffix=""
while [ "$#" -gt 0 ]; do
    if [ "$suite" != merkleize ] || [ "$#" -lt 2 ]; then
        echo "--updates and --skew require merkleize and a value" >&2
        exit 2
    fi
    case "$1" in
        --updates) suffix="${suffix}-u$2"; options+=(--updates "$2") ;;
        --skew)
            skew="$("$root/scripts/validate-skew.sh" "$2")"
            suffix="${suffix}-zipf${skew}"
            options+=(--skew "$skew")
            ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift 2
done

case "$tier" in
    standard) rustflags="" ;;
    full) rustflags="--cfg full_bench" ;;
    huge) rustflags="--cfg huge_bench" ;;
    *) echo "unknown tier: $tier" >&2; exit 2 ;;
esac

case "$suite" in
    merkleize)
        # `full_bench` enables the 100K and 1M keyspace cases. Fixed-count runs exclude churned
        # cases, which retain the original 10% workload.
        filter="qmdb::merkleize"
        if [ "${#options[@]}" -gt 0 ]; then
            # Write-count and skew controls apply only to the main workload.
            filter="qmdb::merkleize/v="
            RUSTFLAGS="$rustflags" "$root/scripts/run-criterion.sh" "${prefix}-merkleize-${tier}${suffix}" qmdb "${options[@]}" "$filter"
        else
            RUSTFLAGS="$rustflags" "$root/scripts/run-criterion.sh" "${prefix}-merkleize-${tier}" qmdb "$filter"
        fi
        ;;
    constantinople)
        echo "Run each required database configuration separately; example:" >&2
        echo "  cargo bench -p commonware-storage --bench constantinople -- any::unordered::fixed::mmb" >&2
        echo "Archive its stdout in ${root}/results/${prefix}-constantinople/notes.md." >&2
        exit 2
        ;;
    chained-growth)
        "$root/scripts/run-criterion.sh" "${prefix}-chained-growth" qmdb chained_growth
        ;;
    *)
        echo "unknown suite: $suite" >&2
        exit 2
        ;;
esac
