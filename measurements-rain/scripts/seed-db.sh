#!/usr/bin/env bash
# Create and persist a pre-seeded master database snapshot.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo_root="$(cd "$root/.." && pwd)"

cargo bench --manifest-path "$repo_root/Cargo.toml" -p commonware-storage --features test-traits --bench qmdb_snapshot -- seed-db "$@"
