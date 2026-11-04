#!/usr/bin/env bash
# Generate deterministic binary keyset file using Rayon parallelization.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo_root="$(cd "$root/.." && pwd)"

cargo bench --manifest-path "$repo_root/Cargo.toml" -p commonware-storage --features test-traits --bench qmdb_snapshot -- generate-keys "$@"
