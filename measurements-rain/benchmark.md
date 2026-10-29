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
For a fixed-count merkleize run, `u=N` records N write calls per batch. Legacy
names omit `u` and use 10% of the seeded key count.
The main merkleize names use `v` for the variant and `k` for the seeded key
count. The current write-size implementation performs uniform random overwrites;
it does not add write-type or skew parameters to benchmark names.

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

Setup opens and seeds a database, and may sync it or clear the page cache.
Chained cases also prepare a pending parent before each timed iteration. These
preparations are excluded from timing. One timed iteration:

1. Creates a child batch.
2. Stages uniform random overwrites: a fixed N write calls when configured,
   otherwise 10% of the seeded key count.
3. Calls `merkleize()`.
4. Retrieves the resulting root.

It does **not** apply, commit, or sync the measured batch. Each iteration works
against the same seeded database, so this is speculative write-path latency,
not durable write latency. Random selection, key hashing, value generation,
batch writes, merkleization, and root retrieval are all inside timing.

Set `QMDB_MERKLEIZE_UPDATES=N` for a direct Cargo invocation. The Rust benchmark
requires a positive `u64`; zero, malformed values, and overflow are rejected.
When unset, the count remains `num_keys / 10` and names omit `u`. Keys are sampled
with replacement, so N calls can update fewer than N distinct keys. N counts
write calls, not bytes. In a chained case, the prepared parent and timed child
each receive N calls. This setting applies only to the main Criterion merkleize
cases; the churned subfamily and Callgrind tracking retain their original counts.

The wrapper `measurements-rain/scripts/run-criterion.sh` accepts `--updates N`
and exports `QMDB_MERKLEIZE_UPDATES=N` before launching Cargo. `--updates` is a
wrapper option, not a Cargo or Criterion option. These two commands select the
same 10K-key, 1K-write, direct, synced, retained-cache workload:

```bash
# Keep metadata aligned with this branch's uniform overwrite workload.
unset QMDB_MERKLEIZE_SKEW QMDB_MERKLEIZE_WRITE_TYPE

# Direct invocation: the Rust benchmark reads the environment variable.
QMDB_MERKLEIZE_UPDATES=1000 cargo bench -p commonware-storage --features test-traits --bench qmdb -- \
  --exact 'qmdb::merkleize/v=any::unordered::fixed::mmb k=10000 u=1000 ch=false s=true cc=false'

# Wrapper invocation: translate --updates, run, and archive the results.
measurements-rain/scripts/run-criterion.sh fixed-any-10k qmdb --updates 1000 -- \
  --exact 'qmdb::merkleize/v=any::unordered::fixed::mmb k=10000 u=1000 ch=false s=true cc=false'
```

The current Rust implementation does not consume `QMDB_MERKLEIZE_SKEW` or
`QMDB_MERKLEIZE_WRITE_TYPE`. Although the local wrapper accepts `--skew` and
preserves these environment settings, they do not select skewed, insert, or mixed
workloads on this branch. Leave both variables unset for write-size experiments;
the metadata helpers can otherwise record requested settings that Rust ignores.

`cc=true` clears only Commonware's page cache after seed synchronization, once
per Criterion group of iterations. Later iterations may warm that cache. It does
not clear the OS cache. `cc=false` retains Commonware's cache. Cache-cleared
cases are available only for direct batches with a synced seed (`ch=false s=true`).

The workload defines a `blob_reads` metric, but the Criterion runner currently
does not collect it. The Gungraun runner collects it for its selected cases;
Criterion latency summaries alone do not provide this I/O counter.

The normal sweep spans `any`/`current`, index type, value type, MMR/MMB,
current chunk size, direct/chained batches, synced/unsynced seeds, and relevant
cache states. Default state is 10K keys. `full_bench` adds 100K and 1M unsynced
cases, with synced cases limited to 10K and 100K. `huge_bench` selects 10M cases,
including synced cases, and should always receive a narrow name filter.

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

`run-criterion.sh` is a wrapper around `cargo bench`. It prepares output paths,
translates the fixed-count option, records provenance, launches the selected
Criterion cases, and archives their output. It does not add timing around the
Rust workload or replace Criterion's sampling policy.

The wrapper requires Cargo/Rust, Git, Bash, `jq`, and these sibling helpers:

- `write-manifest.sh`: records host, CPU, OS, Git revision and dirty state, Rust
  version, UTC timestamp, command, and requested workload settings;
- `summarize-criterion.sh`: converts raw Criterion estimates into CSV and JSON;
- `validate-skew.sh`: used by the metadata helpers even for the default uniform
  setting.

Include these helpers with the wrapper when sharing the runner. The direct
Cargo invocation above works without them.

Wrapper syntax:

```text
measurements-rain/scripts/run-criterion.sh <run-id> <bench-target> [--updates N] -- [Criterion arguments...]
```

Put wrapper options before the standalone `--`; arguments after it are passed
to Criterion. `--updates` is accepted only for the `qmdb` target. The wrapper
clears an inherited `QMDB_MERKLEIZE_UPDATES`, so use its `--updates` option rather
than setting that variable outside the wrapper. Omitting `--updates` selects
the original 10% workload in the main merkleize cases. Select those cases with
`qmdb::merkleize/v=`; a broader `qmdb::merkleize` filter also includes churned
cases, which ignore the fixed-count setting.

```bash
# This branch does not implement skew or write-type overrides.
unset QMDB_MERKLEIZE_SKEW QMDB_MERKLEIZE_WRITE_TYPE

# Archive the main merkleize matrix at the default 10K-key tier and 10% count.
measurements-rain/scripts/run-criterion.sh baseline-standard qmdb -- 'qmdb::merkleize/v='

# Keep 1K write calls per batch while enabling the full database-size tier.
RUSTFLAGS='--cfg full_bench' measurements-rain/scripts/run-criterion.sh fixed-full-u1000 qmdb \
  --updates 1000 -- 'qmdb::merkleize/v='

# Select one 10M-key case with 100K write calls and retained page cache.
RUSTFLAGS='--cfg huge_bench' measurements-rain/scripts/run-criterion.sh fixed-any-10m-u100k qmdb \
  --updates 100000 -- --exact 'qmdb::merkleize/v=any::unordered::fixed::mmb k=10000000 u=100000 ch=false s=true cc=false'
```

Database-size tiers are compile-time settings supplied through `RUSTFLAGS`;
cache, seed-sync, parent, and database variants are selected through Criterion
name filters. Use `--exact` with the complete name to select a single case.

By default, the wrapper builds in `target/measurements/<run-id>/`. An inherited
`CARGO_TARGET_DIR` overrides that path. It stores these artifacts under
`measurements-rain/results/<run-id>/`:

- `command.txt`: the Cargo command and relevant environment settings;
- `manifest.json`: environment and command provenance;
- `output.log`: combined standard output and errors;
- `criterion/`: raw Criterion output;
- `summary.csv` and `summary.json`: normalized observations when parsed;
- `notes.md`: cache state, thermal events, or other operator notes.

Fixed-count runs record `fixed_updates_per_batch` in `manifest.json`. The
normalizer reads `u=N` from each main merkleize benchmark name into
`write_calls_per_batch`; names without `u` use `k / 10`. Choose a descriptive,
unique run ID yourself; this wrapper does not append the write count to it.

Use a fresh result directory and a fresh or cleared Criterion output directory.
The wrapper neither rejects an existing run ID nor clears existing Criterion
reports. It copies every report in the target directory, so reusing directories
can mix old measurements into a new archive. A successful Cargo exit alone does
not establish that the intended cases ran; check that the summary contains the
expected benchmark names and row count.

The optional local `run-part1-baseline.sh` helper selects tiers and delegates to
this wrapper. Fixed-count invocations append `-uN` to that helper's run ID and
exclude churned cases. It is not required for the commands above. Callgrind
capture uses the separate `run-callgrind.sh` script described earlier.

## Interpreting output

Criterion measures only the timed regions described above. Callgrind reports
simulated CPU counters, not elapsed time. Keep results from different CPU
models separate, record cache and background-load conditions, and use
independent process runs before making broad performance claims.
