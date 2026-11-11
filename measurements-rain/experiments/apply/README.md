# Apply-only QMDB study

This study compares **Any unordered B-Tree** (`any::unordered_btree::fixed::mmb`)
against **Current ordered** (`current::ordered::fixed::mmb chunk=32`). Both retain
an authenticated operation log. Their difference measures combined Current ordered
design overhead, including index updates, bitmap changes and authentication-state
installation; it is not an isolated hashing cost.

## Measurement contract

Every iteration:

1. Validate the master and clone it into an independent scratch directory.
2. Open the scratch DB using the same descriptor as Merkleize: 4 KiB pages,
   16,384 cache pages, 2 MiB write buffer, eight strategy threads and MMB.
3. Prepare inputs using seed 99, stage the writes and Merkleize, outside timing.
4. Time only `db.apply_batch(batch).await`.
5. Check the applied root against the prepared root, then drop the DB and remove
   scratch outside timing. The master remains unchanged.

**No commit or sync follows measured application.** The seed is synced during
preparation, outside measurement. The result is buffered batch application latency,
not durable write latency or end-to-end latency. No durability extension is planned
without a separate discussion.

The seed, RNG and inputs reset for every iteration. Duplicate keys are retained in
input generation; write calls can exceed distinct mutations. Independent scratch
seeds keep inserts new and deletions applicable. Commonware caches are retained
following opening and Merkleization; the OS cache is not cleared. This deliberately
measures application immediately following preparation of that same batch.

Snapshot checks, copying and opening can make total execution much slower than the
reported apply duration, especially at 4M/10M keys. Do not interpret short reported
latencies as short total benchmark runtime.

## Running

Prepare the two masters at each desired key count, using the existing snapshot CLI:

```bash
measurements-rain/scripts/generate-keys.sh --count 10000000
measurements-rain/scripts/seed-db.sh --variant any_unordered_btree --count 4000000
measurements-rain/scripts/seed-db.sh --variant current_ordered --count 4000000
```

Select the apply phase explicitly; existing Merkleize runner defaults are preserved:

```bash
export QMDB_STUDY_OPERATION=apply
measurements-rain/experiments/writesize/run.sh 200 3
measurements-rain/experiments/write-type/run.sh 2000 3
measurements-rain/experiments/skew/run.sh --updates 2000 --repeats 3
measurements-rain/experiments/db-scale/run.sh --updates 2000 --repeats 3
```

Write-size, write-type and skew use 4M keys by default, overridable with
`QMDB_MERKLEIZE_KEYS`. Database scaling defaults to 1M, 2M, 4M and 10M keys;
prepare both masters at each size first. All four apply runners alternate the
B-Tree pair's execution order across repeats. Write-type covers overwrite, insert,
and delete; the benchmark also accepts mixed writes for targeted checks.

The benchmark uses existing `QMDB_MERKLEIZE_KEYS`, `QMDB_MERKLEIZE_UPDATES`,
`QMDB_MERKLEIZE_WRITE_TYPE` and `QMDB_MERKLEIZE_SKEW` workload settings.
`QMDB_SNAPSHOT_REUSE` defaults to `required`; `off` creates a fresh seed and
reopens it for each iteration, also outside timing.

## Results and validation

Results go under `measurements-rain/results/apply/`. Names begin with
`qmdb::apply_study/`; normalized rows and comparisons record operation
`apply_study`, timing scope `apply_batch`, durability `buffered`, and iteration
policy `independent_snapshot_fixed_inputs`. Merkleize rows retain their own phase.
Comparisons reject mixed phases, durability endpoints and mismatched engines.
Suite summaries preserve the B-Tree identity and reject mixed operations.

The original `apply_batch.rs` microbenchmarks and Merkleize timed code are unchanged.
Existing write-size plotting tools for historical three-engine archives are not
apply-study renderers; use the phase-aware comparison JSON or suite summaries.
Do not add separately collected stage averages to claim end-to-end latency.

Focused validation (no full-scale performance run):

```bash
cargo test -p commonware-storage --features test-traits --test qmdb_apply_study --test qmdb_prepared_writes
python3 measurements-rain/scripts/tests/test-apply-study.py
python3 measurements-rain/scripts/tests/test-apply-benchmark.py SNAPSHOT_EXECUTABLE QMDB_EXECUTABLE
```

The actual-binary smoke test covers both engines, four write types, uniform and
skewed inputs, repeated reuse, unchanged masters, tiny Criterion observations and
continued selection of the independent Merkleize benchmark. Timing smoke results
are validation data, not study findings.
