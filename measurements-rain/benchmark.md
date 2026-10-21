# QMDB benchmark guide

This document describes the QMDB benchmark programs currently in this checkout:
what they prepare, what they time, and how to run them. It is an implementation
guide, not a record of completed experiments.

Run commands from the repository root:

```bash
cd /Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo
```

## Structure and conventions

The `qmdb` Cargo target (`storage/src/qmdb/benches/bench.rs`) is the Criterion
entry point. It groups five independent lifecycle phases: `apply_batch`,
`chained_growth`, `generate`, `init`, and `merkleize`. Each uses ten
Criterion samples. Criterion calibrates iterations per sample, so ten samples
are not ten independent process runs. A string following `--` filters which
Criterion names execute.

Names encode configuration. `variant` identifies the QMDB representation;
`keys`/`elements` is pre-existing state; `updates`/`operations` is mutation
count; `ch` signals a pending parent; `s` signals a synced seed; and `cc`
signals a deliberately cleared Commonware page cache.

`any` and `current` are representations, not generic modes. `any`
authenticates the operation log and keeps its activity bitmap local. `current`
also authenticates current state by grafting bitmap chunks into the root. The
same benchmarks vary index order, value representation, MMR/MMB, cache and
parent state, key count, and—in `current`—32- or 256-byte chunks.

Run the complete default Criterion target:

```bash
cargo bench -p commonware-storage --features test-traits --bench qmdb
```

## The five `qmdb` Criterion families

### `merkleize.rs`: speculative batch merkleization

Setup opens and seeds a database, and may sync it, clear the page cache, or
create a pending parent. Setup is excluded from timing. One timed iteration:

1. Creates a child batch.
2. Stages random updates equal to 10% of the seeded key count.
3. Calls `merkleize()`.
4. Retrieves the resulting root.

It does **not** apply, commit, or sync that batch. The workload also records
`blob_reads` during timing.

The normal sweep spans `any`/`current`, index type, value type, MMR/MMB,
current chunk size, direct/chained batches, synced/unsynced seeds, and relevant
cache states. Default state is 10K keys. `full_bench` adds 100K/1M cases;
`huge_bench` selects 10M cases and should always receive a narrow name filter.

The churned subfamily is `any` only. Setup applies 50 overwrite batches to
leave inactive operations above the inactivity floor and then uses a smaller
cache. Timing remains updates, merkleization, and root retrieval.

```bash
cargo bench -p commonware-storage --features test-traits --bench qmdb -- qmdb::merkleize
RUSTFLAGS='--cfg full_bench' cargo bench -p commonware-storage --features test-traits --bench qmdb -- qmdb::merkleize
RUSTFLAGS='--cfg huge_bench' cargo bench -p commonware-storage --features test-traits --bench qmdb -- 'current::unordered::fixed::mmr chunk=32'
```

### `apply_batch.rs`: installing authenticated batches

Setup opens and seeds a 65,536-key database, writes 16,384 random updates, and
merkleizes the batch. Only `apply_batch()` is timed. Cases cover direct apply,
ordered versus unordered indexes, committed and uncommitted ancestors, mixed
ancestor chains, multiple uncommitted ancestors, and immutable variants.
Ancestor cases distinguish installation work from reconnecting to pending
history.

```bash
cargo bench -p commonware-storage --features test-traits --bench qmdb -- qmdb::apply_batch
```

### `chained_growth.rs`: deep speculative current-state chains

Setup seeds a `current` database and prebuilds a chain of one-update parent
batches. Timing grows further batches on that chain, merkleizes and applies
them. Small overlays force state resolution to traverse older layers.

```bash
cargo bench -p commonware-storage --features test-traits --bench qmdb -- qmdb::chained_growth
```

### `generate.rs`: write-heavy construction

The timer starts before initialization. Each iteration creates a database,
populates it, performs random updates/deletes, periodically merkleizes, applies,
commits, prunes, and syncs it. Destruction is outside timing. It covers
fixed- and variable-value keyed variants and keyless MMR/MMB variants; keyless
cases measure append-only log growth without an index or bitmap.

```bash
cargo bench -p commonware-storage --features test-traits --bench qmdb -- qmdb::generate
```

### `init.rs`: startup and replay

The harness first populates and syncs each on-disk database once, outside the
timed region. Each timed iteration reopens that database and rebuilds
initialization/snapshot state at a selected location-to-key cache size. The
stored database is destroyed after the family finishes.

```bash
cargo bench -p commonware-storage --features test-traits --bench qmdb -- qmdb::init
```

## Other benchmark targets

### `qmdb_components`: storage-free CPU components

`storage/src/qmdb/benches/components.rs` constructs inputs before timing, then
isolates operation-leaf hashing, MMR/MMB operation-tree merkleization, bitmap
mutation under two locality patterns, grafted-leaf hashing, the all-zero chunk
identity path, and upper grafted-tree merkleization. It is for CPU attribution,
not end-to-end database latency.

```bash
cargo bench -p commonware-storage --bench qmdb_components
```

### `constantinople`: application-shaped block pipeline

`storage/src/qmdb/benches/constantinople.rs` is a standalone harness rather
than Criterion. It seeds one selected MMB database and times staged reads,
staged writes, merkleization, and root retrieval; it prints load and
merkleization time separately. Defaults: 1M keys, 32,768 reads, 32,768 updates,
and eight strategy-pool threads. It requires a database kind and otherwise does
nothing, preventing an accidental large run from `cargo bench --benches`.

```bash
cargo bench -p commonware-storage --bench constantinople -- any::unordered::fixed::mmb
```

After the database kind, optional positional arguments are `depth`, `iters`,
`keys`, `reads`, `read_chunks`, `updates`, `threads`, and `page_cache`
(4KiB pages).

### `scale`: reusable on-disk scale study

`storage/src/qmdb/benches/scale.rs` avoids Criterion resampling for multi-GB
databases. `generate` creates and syncs a database through random updates and
deletes (Zipfian by default; `0` selects uniform). `init` reopens it once and
reports initialization/replay. `get` runs cold and warm point-read passes;
`get_many` does the same through batched reads. `destroy` removes the supplied
directory.

`generate` can use substantial time and disk. `destroy` is destructive: use a
dedicated disposable directory.

```bash
# Create a database at a disposable path.
cargo bench -p commonware-storage --features test-traits --bench scale -- generate /tmp/qmdb-scale 1000000 500000 0 unordered

# Reopen: init-cache entries, total initialization tasks, index type.
cargo bench -p commonware-storage --features test-traits --bench scale -- init /tmp/qmdb-scale 0 1 unordered

# Point reads at one and four readers (cold then warm at each level).
cargo bench -p commonware-storage --features test-traits --bench scale -- get /tmp/qmdb-scale 1000000 100000 1,4 unordered

# Batched reads, 128 keys per get_many call.
cargo bench -p commonware-storage --features test-traits --bench scale -- get_many /tmp/qmdb-scale 1000000 100000 1,4 128 unordered

# DESTRUCTIVE: delete this database directory.
cargo bench -p commonware-storage --features test-traits --bench scale -- destroy /tmp/qmdb-scale
```

### `qmdb_gungraun`: Callgrind CPU counters

`storage/src/qmdb/benches/merkleize_gungraun.rs` reuses selected merkleize
workloads under Gungraun/Callgrind. It reports deterministic instruction,
simulated cache, memory read/write, and estimated-cycle counters rather than
wall-clock latency. The current tracking configuration selects two
representative variants and retains the shared `blob_reads` metric.

```bash
measurements-rain/scripts/run-callgrind.sh <run-id>
```

This launches `just benchmark-tracking generate`, writes a run manifest, and
copies `benchmark-tracking-results/` to
`measurements-rain/results/<run-id>/callgrind/`.

## Reproducible result capture

Use the measurement scripts when results must be retained. They write host,
CPU, OS, Git revision and dirty state, Rust version, UTC timestamp, and command
to `manifest.json`; preserve Criterion artifacts; and produce summaries.

```bash
# Archive one filtered Criterion run.
measurements-rain/scripts/run-criterion.sh <run-id> qmdb qmdb::merkleize

# Archive guarded Part 1 tiers: standard, full, or huge.
measurements-rain/scripts/run-part1-baseline.sh <run-prefix> merkleize
measurements-rain/scripts/run-part1-baseline.sh <run-prefix> merkleize full
measurements-rain/scripts/run-part1-baseline.sh <run-prefix> merkleize huge
```

The generic runner builds in `target/measurements/<run-id>/`, then stores
artifacts under `measurements-rain/results/<run-id>/`:

- `manifest.json`: environment and command provenance;
- `criterion/`: raw Criterion output;
- `summary.csv` and `summary.json`: normalized observations when parsed;
- `callgrind/`: archived CPU-counter output; and
- `notes.md`: cache state, thermal events, or other operator notes.

The Part 1 helper selects the `merkleize` suite. Its tiers use no extra flag,
`--cfg full_bench`, and `--cfg huge_bench`. It intentionally does not launch
Constantinople automatically because each configuration should be separately
labeled and archived.

## Interpreting output

Criterion measures only the timed regions described above. Callgrind reports
simulated CPU counters, not elapsed time. Keep results from different CPU
models separate, record cache and background-load conditions, and use
independent process runs before making broad performance claims.
