# QMDB Core CPU and Time Benchmark Plan

## Goal

Measure the incremental cost of QMDB current-state authentication. The primary
comparison uses the same fixed 32-byte keys, values, operations, and seeded
state:

```text
any       Merkleized operation log; local activity bitmap
current   Same operation log; bitmap chunks grafted into the authenticated root
```

Report absolute values and `current - any` / percentage overhead. Do not claim
that an end-to-end difference is bitmap hashing alone until the component
benchmarks account for it.

## Reproducible result layout

Run outputs belong in `measurements-rain/results/<run-id>/` and are ignored by
Git. Each run must contain `manifest.json`, `notes.md`, copied raw output, and
normalized summaries where applicable. Use a descriptive run ID, for example
`2026-10-14-m2-max-warm`.

The scripts intentionally require no Python. They write host, CPU, OS, Git
revision, dirty state, Rust version, command, and UTC timestamp before running.

```bash
measurements-rain/scripts/run-criterion.sh <run-id> qmdb_components
measurements-rain/scripts/run-criterion.sh <run-id> qmdb qmdb::merkleize
measurements-rain/scripts/run-part1-baseline.sh <run-prefix> merkleize
measurements-rain/scripts/run-part1-baseline.sh <run-prefix> merkleize full
measurements-rain/scripts/run-part1-baseline.sh <run-prefix> merkleize huge
measurements-rain/scripts/run-callgrind.sh <run-id>
```

`run-criterion.sh` forwards trailing arguments to Criterion. `run-callgrind.sh`
uses `just benchmark-tracking generate`; it runs directly on Linux and uses the
repository's Docker path on macOS.

Criterion output is built in an isolated `target/measurements/<run-id>/`
directory and copied into the run directory after a successful measurement.

`run-part1-baseline.sh` is the guarded launcher for a long Part 1 run. It does
not run the application-shaped Constantinople cases automatically because each
database variant is a separate positional invocation and must be labeled and
archived independently.

## Phase 1: Existing whole-system benchmarks

Run the existing `qmdb` benchmark with `RUSTFLAGS="--cfg full_bench"`. Use
unordered fixed-size variants first, then ordered fixed-size variants.

- Variants: `any` and `current`.
- Families: MMR and MMB.
- Current chunk sizes: 32 and 256 bytes.
- States: fresh, synced/warm cache, cache-cleared, and churned overwrites.
- Initial keyspaces: 100K, 1M, 2M, and 4M active keys. The 4M tier is the
  maximum initial measurement on the local personal machine.

The timed path is deliberately unchanged: staged random writes, `merkleize()`,
and root retrieval. Preserve Criterion elapsed-time estimates and the emitted
`blob_reads` counter together.

For the 4M-key tier, pass one specific Criterion name filter when running a
large-state case. This avoids accidentally launching the complete variant
matrix at once. Larger states are deferred until the initial local study has
established its storage and runtime requirements.

Then run:

- `constantinople` for 1M seeded keys and block-shaped 32K reads/32K updates;
  compare unordered and ordered fixed MMB `any`/`current` pairs and retain its
  load versus speculative-merkleization split.
- `chained_growth` to show current-state behavior with long speculative chains
  and one update per batch.
- Existing Gungraun tracking to establish the current two CPU references.

`scale` is a later startup/read study: it currently measures `any` initialization
and reads, not current-state write/grafting cost.

## Phase 2: CPU component attribution

The new `qmdb_components` target is intentionally storage-free. It uses
precomputed inputs and stable Criterion names:

```text
qmdb::component/component=<name> family=<mmr|mmb> keys=<N> updates=<N> locality=<mode> chunk=<bytes>
```

It measures:

1. operation leaf hashing for MMR and MMB;
2. in-memory operation-tree batch Merkleization for MMR and MMB;
3. activity bitmap mutation under disjoint and repeated locations;
4. non-zero grafted-leaf hashing, `hash(bitmap_chunk || operation_subtree_root)`;
5. the all-zero grafted-chunk identity path.
6. upper grafted-tree Merkleization for MMR and MMB.

The next extension adds independent `merkleize`, `apply_batch`, `commit`, and
`sync` timings on the same prepared database batch. Mirror the final component
matrix in `qmdb_gungraun` to collect
Callgrind `Ir`, `L1hits`, `LLhits`, `RamHits`, `TotalRW`, and `EstimatedCycles`.

## Workload matrix

The initial experiment changes one variable at a time. Unless a subsection
explicitly says otherwise, keep the database at 4M keys and hold all other
workload, implementation, cache, and durability settings constant.

### Database size

Sweep the seeded database size across:

```text
100K, 1M, 2M, 4M keys
```

The 4M-key case is the maximum planned database size for the initial study. For this sweep, keep
the write size, skewness, and write type fixed.

### Write size

With a 4M-key database, sweep the absolute number of write calls per
speculative batch across:

```text
200, 500, 1,000, 1,500, 2,000, 5,000, 7,000, 10,000
```

These are realistic absolute batch sizes, not percentages of the database
size. The 10,000-write case is the maximum planned batch size. The benchmark
must report write calls separately from distinct keys because sampling with
replacement can select the same key more than once.

### Write skewness

With a 4M-key database and a fixed write size, sweep the Zipf exponent:

```text
0, 0.5, 1.0, 1.5, 2.0
```

Exponent `0` means uniform key selection. Positive exponents increasingly
concentrate writes on hot keys. Keep the RNG seed and generated workload
reproducible across `any` and `current`.

### Write type

With a 4M-key database and a fixed write size, compare three exclusive
100%-workload cases:

```text
100% overwrite
100% new write
100% deletion
```

Overwrites select keys already present in the seeded database. New writes use a
disjoint key range that is absent from the seeded database. Deletions select
existing keys and stage `batch.write(key, None)`. Deletion support is required
before collecting deletion results; the current benchmark implementation has
overwrite, insert, and mixed modes but does not yet provide deletion.

The existing comparison dimensions remain available for the broader study:
MMR versus MMB, ordered versus unordered indexes, current bitmap chunk sizes,
cache state, locality, and speculative versus durable operation paths. They
should be added only as controlled follow-up sweeps rather than mixed into the
initial matrix.

## Local personal-machine execution and storage

This measurement plan targets a personal local machine, not a cloud or
multi-host experiment. Run configurations sequentially instead of
materializing the full Cartesian product.

- Keep only the database for the active configuration and the required result
  artifacts on disk.
- Measure and record committed on-disk database size after initialization.
- Clean up temporary database directories after each successful configuration.
- Perform a free-space check before every 4M-key run and budget
  approximately 2--5 GB of available space per active database variant.
- Preserve manifests, summaries, and notes while removing large temporary
  database files after successful runs.
- Record CPU, RAM, operating system, compiler/toolchain, and storage-device
  information. Do not combine results from different machines without keeping
  those environments separate.

The raw 32-byte key plus 32-byte value payload for 4M pairs is approximately
256 MB before QMDB journal, Merkle, bitmap, index, alignment, and temporary
working-space overhead. The 2--5 GB budget is a conservative execution
budget, not an assertion that the committed logical database will occupy the
full range.

## Reproducibility and acceptance criteria

Perform three independent process runs per configuration, preserve the default
Criterion sample policy, warm once before measurement, and note cache state,
thermal events, and competing work in `notes.md`.

For every `any`/`current` comparison, use identical seeded keys, values, RNG
seeds, and initial database state. The default timed operation must be stated
explicitly as write staging, `merkleize()`, and root retrieval. Runs that test
`apply`, `commit`, or `sync` must be labeled as durable-path measurements.

Every result should report, where available:

- latency and throughput;
- committed storage growth and bytes written;
- write-call count and distinct-key count;
- database size, write size, skew exponent, write type, cache state, and
  durability mode.

Before collecting the full matrix, verify that:

- all requested write sizes are accepted;
- all requested Zipf exponents are accepted;
- a 4M-key seed completes within the local disk budget;
- overwrite, new-write, and deletion cases target the intended key population;
- each run records its exact parameters in `manifest.json` and `notes.md`.

The milestone is complete when:

- all existing baseline runs have manifests and normalized summaries;
- component names compile and pass benchmark-name validation;
- `current - any` CPU/time overhead is reported alongside component totals;
- any residual is labeled as index, batch, allocation, or runtime work rather
  than attributed to bitmap hashing;
- the report states that committed QMDB mutation application is serialized,
  while speculative and internal computation may parallelize.
