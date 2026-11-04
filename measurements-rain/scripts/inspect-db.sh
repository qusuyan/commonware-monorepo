#!/usr/bin/env bash
# Inspect and verify an existing pre-seeded database snapshot.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo_root="$(cd "$root/.." && pwd)"

cargo bench --manifest-path "$repo_root/Cargo.toml" -p commonware-storage --features test-traits --bench qmdb_snapshot -- inspect-db "$@"
