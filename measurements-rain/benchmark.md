# QMDB benchmark guide

Saved-keyset and database setup now uses a validated shared configuration. See
[snapshot reuse](snapshot-reuse.md) for preparation, required/auto/off behavior,
supported variants, and interpretation of setup provenance.

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
count. The write-size runner uses uniform random overwrites. Explicit write-type and
skew selections add `w=<type>` and `zipf=<exponent>` to benchmark names.

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
Chained cases also prepare a pending parent before each timed iteration. After
parent preparation, each iteration samples keys and generates key/value pairs
into a reusable input buffer. All of these preparations are excluded from both
Criterion timing and Gungraun collection. One timed iteration:

1. Creates a fresh batch or a child of the prepared parent.
2. Stages the prepared writes: a fixed N write calls when configured,
   otherwise 10% of the seeded key count.
3. Calls `merkleize()`.
4. Retrieves the resulting root.

It does **not** apply, commit, or sync the measured batch. Each iteration works
against the same seeded database, so this is speculative write-path latency,
not durable write latency. Random selection, benchmark key hashing, value
generation, and input-buffer allocation are outside timing; batch creation,
batch writes, merkleization, and root retrieval are inside. Database-internal
copying, encoding, and hashing remain measured. Inputs preserve write order and
duplicates, and only one batch of inputs is buffered at a time. The churned
benchmark uses the same boundary after its untimed churn setup.

Historical results collected before this timer-boundary revision include input
generation. Benchmark names are unchanged, so keep those archives separate and
record the code revision when comparing results. Rerun baselines for comparisons
with the revised boundary; old and new timings/counters are not directly comparable.

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

The Rust benchmark consumes `QMDB_MERKLEIZE_SKEW` (finite, non-negative)
and `QMDB_MERKLEIZE_WRITE_TYPE` (`overwrite`, `insert`, `delete`, or `mixed`). The
wrapper accepts `--skew`; dedicated runners select the intended workload.
Write-size runs clear both variables to preserve uniform overwrites.
The actual write-type experiment runs overwrite, insert, and delete separately:
each batch contains 100% of its selected operation type. Mixed writes are excluded
from that experiment; the mixed case in the snapshot validation archive was a
correctness check only.

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

### `apply_study.rs`: study-specific batch application

The new apply-only study compares Any unordered B-Tree with Current ordered,
using shared Merkleize configuration and validated independent scratch snapshots.
Only application is timed; preparation and Merkleization precede timing and no
commit or sync follows application. Select `QMDB_STUDY_OPERATION=apply` with the
write-size, write-type, skew and database-scale runners. Defaults continue to run
Merkleize. See [apply study instructions](experiments/apply/README.md).

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

### `qmdb_cpu`: native CPU accounting

This standalone Rust executable measures native process CPU consumption with
`libc::getrusage(RUSAGE_SELF)` and elapsed time with `Instant`. It runs separately
from Criterion and Callgrind. CPU accounting includes all process threads and
runtime housekeeping, but excludes child processes.

Select one phase and one matched B-tree/MMB variant per process:

```bash
# Prepare validated masters first, using the same key count as the CPU run.
measurements-rain/scripts/seed-db.sh --variant any_unordered_btree --count 4000000
measurements-rain/scripts/seed-db.sh --variant current_ordered --count 4000000

measurements-rain/scripts/run-cpu-telemetry.sh cpu-any-merkleize \
  --phase merkleize --variant any_unordered_btree --keys 4000000 --updates 10000
measurements-rain/scripts/run-cpu-telemetry.sh cpu-current-apply \
  --phase apply --variant current_ordered --keys 4000000 --updates 10000
```

Required options are `--phase`, `--variant`, `--keys`, and `--updates`.
Optional settings are `--samples` (10), `--write-type` (overwrite, insert, delete,
or mixed; default overwrite), and `--skew` (default 0). Inputs use RNG seed 99.
The script manages `--output`; when invoking `qmdb_cpu` directly, supply a new
output directory. `QMDB_SNAPSHOT_REUSE` defaults to `required`; `off` allows small
fresh-seed smoke runs without modifying saved masters.

Each sample opens an independent scratch seed and submits one batch. Merkleize
measures batch creation, staging precomputed writes, merkleization, root retrieval,
and speculative batch disposal. Apply prepares and merkleizes outside accounting,
then measures only application. Root validation, database cleanup and output
serialization are excluded. Neither phase commits or syncs its measured batch.
Commonware caches are retained after preparation; the OS page cache is not cleared.
Both cases use MMB, the same ordered index implementation, and eight strategy
threads; Current ordered uses 32-byte chunks. Differences represent combined
Current ordered semantics, not an isolated cryptographic cost.

The run directory contains `manifest.json`, `command.txt`, `output.log`, and a
`cpu/` directory with `configuration.json`, `samples.jsonl`, `overhead.jsonl`, and
`summary.json`. Existing destinations are rejected, including partial prior runs.
Raw samples include wall/user/kernel CPU nanoseconds, context switches, page
faults, snapshot provenance and workload settings. Summaries report mean, median,
and sample standard deviation (null for one sample), plus CPU cost per batch/write
attempt, average cores, and CPU percentage. Duplicate keys remain write attempts.

Nanosecond units do not imply nanosecond resolution. Zero CPU deltas are preserved
and flagged; samples within 10 times their paired empty-window CPU overhead are
also flagged. Empty-window overhead is recorded, never subtracted. Increase work
per batch when precision is insufficient. Native results do not participate in
Callgrind regression gates. Linux and macOS are supported; validation status is
recorded in [PROP-07](proposals/proposed/07-cpu-metrics-and-profiling.md).

The runtime also exposes `runtime_process_cpu_usage_basis_points` through its
existing `sysinfo` sampler after two CPU refreshes, at the existing 10-second
cadence. Divide by 100 for percent or by 10,000 for average cores; values above
10,000 are valid. Fractional utilization is rounded to the nearest basis point.
Unavailable readings remove the gauge until re-primed. Disable CPU monitoring
with `tokio::Config::with_process_cpu_monitoring(false)`; memory sampling remains
enabled. The native runner and existing Gungraun runner do this automatically, keeping
CPU polling out of their measured intervals.

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
# Keep write-size runs uniform overwrites.
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

For the active controlled measurement pair, run the reduced end-to-end sweep
when the full matrix is too expensive:

```bash
measurements-rain/scripts/run-focused-matrix.sh 3
```

It runs the 2K anchor, overwrite/insert/delete, Zipf 0 and 2, and 1M/10M
database endpoints using `any_unordered_btree` versus `current_ordered`.

## Interpreting output

Criterion measures only the timed regions described above. Callgrind reports
simulated CPU counters, not elapsed time. Keep results from different CPU
models separate, record cache and background-load conditions, and use
independent process runs before making broad performance claims.

## Active configuration comparisons

All active dedicated repeats run `any_unordered_btree` and `current_ordered`.
Both use fixed 32-byte keys and values, MMB, the same nonpartitioned ordered
B-Tree index type, direct batches, a synced seed, and retained Commonware cache.
Current uses 32-byte chunks. Configuration order alternates across repeats.

Their comparison JSON uses `schema_version: 2`, explicit configuration IDs under
`configurations`, and `current_ordered_minus_any_unordered_btree` under
`comparisons`.

Each comparison records `delta_mean_us` and `overhead_percent`, relative to the
baseline named after `_minus_`. Differences are descriptive, not
confidence intervals for paired differences. Absolute observations retain
Criterion means, confidence intervals, and sample counts.

Write-type repeats use the same pair across pure overwrite, insert, and delete
batches. The pair controls the index implementation, but its difference still
contains the Current record and authentication design, not an isolated bitmap cost.

Timing includes batch creation, prepared-write staging, speculative
Merkleization, and root retrieval; input generation is excluded. Historical
runs before the timer-boundary revision include input generation. The measured batch is discarded: these experiments measure
maintaining proof-supporting structures, not proof generation, verification,
durable writes, or total authentication overhead. Any retains its authenticated
operation log and local bitmap. Ordering differences include all ordered-design
work, not exclusively exclusion-proof hashing.

The active dedicated workload runs use 4M seeded keys (`k=4000000`), with 200, 500,
1K, 1.5K, 2K, 5K, 7K, 8K, or 10K write calls for the write-size sweep. The
write-type sweep uses 2K write calls. Skew uses 2K writes for the full study and
configurable exponents at fixed 4M keys, with a 10K-key smoke mode. The dedicated
database scaling sweep evaluates 1M, 2M, 4M, and 10M keys at 2K writes.
Arbitrary key counts can be selected at runtime via `QMDB_MERKLEIZE_KEYS`. See their READMEs for
commands. Writesize plots and aggregation accept the active pair and legacy
configuration sets, but a single aggregate cannot mix them. No Rust benchmark timing changes are
required to select Current + ordered.
