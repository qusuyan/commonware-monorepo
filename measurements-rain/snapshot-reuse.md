# Saved keysets and Merkleization databases

Snapshot reuse supports `any_unordered_btree` and `current_ordered`, fixed 32-byte
keys/values, MMB, and 32-byte Current chunks. It applies only to synced-seed
Merkleization cases (`s=true`). Other variants and unsynced cases retain fresh
setup. The write-type runner compares standard `any_unordered` with
`current_ordered`: Any reuses saved keys but builds its database fresh; Current
requires a saved database by default. The additional `any_unordered_btree`
snapshot support does not change that selected comparison.

## Prepare once

Run these commands from the repository root. These are full-study preparation
commands, not implementation smoke tests:

```bash
measurements-rain/scripts/generate-keys.sh --count 10000000
measurements-rain/scripts/seed-db.sh --variant any_unordered_btree --count 4000000
measurements-rain/scripts/seed-db.sh --variant current_ordered --count 4000000
```

The key file lives at `measurements-rain/data/keys_10M.bin`, with a `.bin.json`
sidecar. Masters live under `measurements-rain/storage-seeds/k<count>_<variant>/`.
Repeated preparation validates and reuses compatible artifacts. It never replaces
an incompatible destination. Choose a new output path or explicitly remove an old
artifact yourself. A custom `--out` is useful for inspection/tests; automatic
benchmark lookup uses the default master paths only.

A missing default key file is generated during seed preparation. Explicit
`--keys-file` inputs must already exist and contain enough keys. A legacy key
file without a sidecar is checked against every canonical `SHA256(index)` key
before a sidecar is published. Subsequent process loads check the full-file
checksum; unchanged validated keysets are cached within a process. Corrupted or
insufficient files fail with an error. Do not edit key files or their sidecars.

## Run the supported pair

Use the existing runner with an exact variant filter, for example:

```bash
measurements-rain/scripts/run-criterion.sh pair-any-u10000 qmdb \
  --keys 4000000 --updates 10000 -- \
  --exact 'qmdb::merkleize/v=any::unordered_btree::fixed::mmb k=4000000 u=10000 ch=false s=true cc=false'
measurements-rain/scripts/run-criterion.sh pair-current-u10000 qmdb \
  --keys 4000000 --updates 10000 -- \
  --exact 'qmdb::merkleize/v=current::ordered::fixed::mmb chunk=32 k=4000000 u=10000 ch=false s=true cc=false'
```

Snapshot reuse does not select benchmark variants. For the write-size/skew
runners and the pairwise write-type runner, first prepare `current_ordered` at
the requested key count. Standard unordered variants still seed fresh.

Batch size, overwrite/insert/delete/mixed selection, and Zipf exponent do not change the
initial master. The 4M masters can be shared across workload sweeps and process
repeats. Database scaling requires separate masters for 1M, 2M, 4M, and 10M keys
for each supported variant. All sizes use prefixes of the same canonical keyset.

## Reuse modes and retention

`QMDB_SNAPSHOT_REUSE` accepts:

- `required` (default): fail if the supported synced-seed case has no compatible
  master; print the preparation command.
- `auto` or `1`: reuse when present; seed fresh only if the master is absent.
- `off` or `0`: seed fresh, ignoring masters.

Invalid values fail. An invalid existing master always fails in `required` and
`auto`; it never silently falls back to an empty or freshly seeded database.
For a small implementation smoke run, explicitly set `QMDB_SNAPSHOT_REUSE=off`
and select a small key count. Full-study preparation is never automatic during
validation.

Each Criterion measurement group receives an isolated scratch copy. Its speculative
batches are discarded and never applied to the master. The scratch database is
removed after database/runtime handles close. Saved keys and masters remain for
subsequent invocations. Failed preparation cleans its staging directory; an
abruptly killed process can leave a lock or scratch directory. Remove a stale
`.lock` only after confirming its recorded process is no longer running.

Inspection uses the same validation contract and opens only a scratch copy:

```bash
measurements-rain/scripts/inspect-db.sh \
  --db-path measurements-rain/storage-seeds/k4000000_current_ordered_fixed_mmb_chunk32
```

## Configuration and provenance

Creation and benchmarking share one configuration builder: `bench-merkleize`
partitions, 10M items per blob, 4 KiB pages, 16,384 cache pages, 2 MiB write buffers,
eight strategy threads, `EightCap`, and the existing initialization settings.
Storage settings and execution/cache settings are recorded separately in a
canonical descriptor with a SHA-256 fingerprint. Unrelated benchmarks retain their
own defaults. Fresh and reused supported synced-seed cases both close and reopen
before measurement with newly constructed caches; `cc=true` additionally clears
the Commonware cache. The OS cache is not cleared.

Manifest schema 2 records the configuration, variant, seeded key count, keyset
prefix digest, value generator and seed, root, operation bounds, and storage file
inventory/checksums. Operation bounds describe log positions, not live-key count.
Legacy snapshot manifests are rejected and must be recreated. Inspection rejects
missing/truncated/changed files, incompatible configurations, and root mismatches
without initializing new storage in the master directory.

Setup emits JSON records containing `qmdb_setup: true`, the reuse mode, source,
snapshot path, configuration fingerprint, initial root, keyset prefix digest,
clone method, and clone duration. Runners retain these in logs; the summarizer
also writes `setup-provenance.ndjson`. It is possible to have several setup records
for one benchmark because Criterion requests several measurement groups.

Historical benchmark names are retained. Compare results together with their
setup provenance: historical seed/cache behavior and the new reopened-state
behavior are not guaranteed to have identical latency. Fresh and reused paths
must produce identical logical state and prepared-batch roots. CoW availability
is platform/filesystem dependent; a normal recursive-copy fallback is supported
and timed outside the benchmark.

## Focused validation

```bash
cargo test -p commonware-storage --features test-traits \
  --test qmdb_keyset --test qmdb_snapshot_reuse --test qmdb_persistence
cargo build -p commonware-storage --features test-traits --bench qmdb_snapshot
python3 measurements-rain/scripts/tests/test-snapshot-cli.py <qmdb_snapshot-executable>
python3 measurements-rain/scripts/tests/test-snapshot-benchmark.py <qmdb_snapshot-executable> <qmdb-executable>
```

Tests use small datasets and cover real producer/consumer builders, file
corruption, manifest mismatch, repeated reopen/reuse, blob boundaries, concurrent
scratch allocation, and unchanged masters. No full-scale study is required.

## Full-scale validation

The `exercise-db --db-path DIR --updates 10000` CLI command reports the initial
root and deterministic speculative roots for uniform overwrites, uniform inserts,
mixed writes, and Zipf-2 overwrites. It uses a scratch copy and does not apply the
batches to the master. Compare this output from independently created seeds and
repeated processes to verify logical equivalence.

`measurements-rain/scripts/validate-large-snapshots.py RUN_DIR SNAPSHOT_EXECUTABLE
QMDB_EXECUTABLE` performs the explicit 4M/10M validation using release executables.
It retains the 10M keyset and four default masters, validates every generated key,
checks idempotent preparation and master checksums, compares independent fresh and
reused batch roots, and exercises actual Criterion callbacks and short timings.
Each command has saved stdout/stderr and resource statistics. Free disk below
20 GiB or process-tree RSS above 18 GiB stops the run; no existing artifact is
deleted. Only freshly created comparison databases are removed.
