#!/usr/bin/env bash
# Create and persist a pre-seeded master database snapshot.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo_root="$(cd "$root/.." && pwd)"

cargo build --release --manifest-path "$repo_root/Cargo.toml" -p commonware-storage \
    --features test-traits --bench qmdb_snapshot
snapshot_bin="$(find "$repo_root/target/release/deps" -maxdepth 1 -type f -name 'qmdb_snapshot-*' -perm -111 -print0 \
    | xargs -0 ls -t | head -n 1)"
[ -n "$snapshot_bin" ] || { echo "qmdb_snapshot executable was not built" >&2; exit 1; }
exec "$snapshot_bin" seed-db "$@"
