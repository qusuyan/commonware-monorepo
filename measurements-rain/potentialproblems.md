# Potential Problems and Architectural Hazards in Apply Study (`apply_study.rs`)

This document details identified performance traps, operational hazards, test-coverage gaps, and methodological nuances resulting from the code review of [`storage/src/qmdb/benches/apply_study.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/apply_study.rs), its test suites, and supporting benchmark tooling.

---

## 1. High-Impact Performance & Operational Hazards

### Hazard 1.1: Criterion Iteration Scaling & Wall-Clock Runtime Explosion
- **Affected File:** [`storage/src/qmdb/benches/apply_study.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/apply_study.rs#L86-L143)
- **Affected Runners:**
  - [`measurements-rain/experiments/writesize/run.sh`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/experiments/writesize/run.sh)
  - [`measurements-rain/experiments/write-type/run.sh`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/experiments/write-type/run.sh)
  - [`measurements-rain/experiments/skew/run.sh`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/experiments/skew/run.sh)
  - [`measurements-rain/experiments/db-scale/run.sh`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/experiments/db-scale/run.sh)

#### Mechanism
In [`merkleize.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/merkleize.rs#L935-L979), database cloning and opening happen **once per `iter_custom` invocation**, while the loop over `0..iters` simply generates writes and calls `merkleize()` in memory.

In contrast, [`apply_study.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/apply_study.rs#L88-L141) mutates database state during `apply_batch()`. To guarantee that every iteration starts from the identical baseline state, snapshot preparation, runtime startup, database opening, input preparation, Merkleization, and scratch directory cleanup were placed **inside** the `for _ in 0..iters` loop:

```rust
b.iter_custom(|iters| {
    let mut total = Duration::ZERO;
    for _ in 0..iters {
        let prepared = snapshots::prepare(variant, keys, mode).expect("prepare validated seed");
        // ... generate writes, stage, and merkleize ...
        let elapsed = tokio::Runner::new(...).start(|ctx| async move {
            // ... open DB, check database, apply_prepared ...
        });
        total += elapsed; // Only apply_batch duration is returned
        drop(prepared);
    }
    total
});
```

#### Problem
Criterion scales its iteration count (`iters`) based **strictly on the returned `total` `Duration`**, which measures only `db.apply_batch(batch).await` (~0.5 ms to 5 ms). Criterion is entirely unaware of the heavy untimed setup per iteration:
1. Copy-on-Write clone of a 4M/10M key master snapshot (~0.5–2 seconds).
2. Starting and tearing down a Tokio runtime.
3. Opening the database and validating bounds (~1–5 seconds).
4. Merkleizing the batch outside timing (~0.5–2 seconds).

With default Criterion settings (3 seconds warm-up time, 5 seconds measurement time across 10 samples), Criterion will evaluate sub-millisecond apply latencies and request **hundreds of iterations per sample** (thousands of iterations total). At 5–15 seconds of untimed work per iteration, a single benchmark run can take **multiple hours or days** and appear hung.

> [!IMPORTANT]
> While [`test-apply-benchmark.py`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/tests/test-apply-benchmark.py#L64-L65) works around this by passing `--warm-up-time 0.05 --measurement-time 0.05`, the primary experiment runners (`writesize/run.sh`, `write-type/run.sh`, `skew/run.sh`, `db-scale/run.sh`) run Criterion with default timers and sample sizes, leaving them exposed to runtime explosion.

#### Recommended Mitigation
- When `QMDB_STUDY_OPERATION=apply`, configure Criterion with strict upper bounds on warmup and measurement times (e.g. `--warm-up-time 0.5 --measurement-time 1.0`) or pass explicit iteration caps.
- Alternatively, support batching scratch environments or recycling read-only snapshots if CoW unmounts can be expedited.

---

### Hazard 1.2: Double Database Initialization Per Iteration
- **Affected File:** [`storage/src/qmdb/benches/snapshots.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/snapshots.rs#L435) & [`storage/src/qmdb/benches/apply_study.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/apply_study.rs#L106-L137)

#### Mechanism
On every iteration, [`snapshots::prepare_at`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/snapshots.rs#L435) validates the cloned scratch directory by calling:

```rust
verify_open(scratch.path(), &manifest, None)?;
```

`verify_open` spawns a `tokio::Runner`, initializes the database, verifies bounds and roots, and then immediately shuts down and drops the database handle.

Immediately after `prepare` returns to [`apply_study.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/apply_study.rs#L106-L137), `bench_apply_study` spawns a **second** `tokio::Runner`, initializes the database from the same scratch directory a second time, and runs:

```rust
snapshots::check_database(&db, manifest).unwrap();
```

#### Problem
The scratch database is fully opened, bounds-checked, and closed **twice per iteration** (and in `ReuseMode::Off` / fresh mode, it is populated, verified, and opened a third time). While excluded from the timed `apply_batch` metric, this doubles the heaviest disk I/O and journal recovery overhead in the untimed phase.

#### Recommended Mitigation
- Add a lightweight `prepare_unopened` function or a verification bypass flag for benchmark scratch clones where `master` was already validated by manifest checksums prior to cloning.

---

### Hazard 1.3: Redundant Memory Allocation Inside Iteration Loop
- **Affected File:** [`storage/src/qmdb/benches/apply_study.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/apply_study.rs#L94-L101)

#### Mechanism
Lines 94–101 recreate the input vector and sample the exact same keys on every iteration:

```rust
let mut writes = Vec::new();
prepare_sampled_writes(
    &mut writes,
    updates,
    &mut TestRng::new(99),
    write_type.unwrap_or(WriteType::Overwrite),
    &sampler,
);
```

#### Problem
Because the RNG seed (`TestRng::new(99)`) is fixed, every iteration generates the identical write operations. Reallocating the `Vec` and regenerating identical elements on every iteration adds unnecessary untimed heap churn and CPU cycles.

#### Recommended Mitigation
- Prepare `writes` once before entering the `for _ in 0..iters` loop, or allocate `writes` outside the loop and reuse the buffer.

---

## 2. Test Coverage & Verification Gaps

### Hazard 2.1: Asymmetric Root Determinism Verification for `CurrentOrdered`
- **Affected File:** [`storage/tests/qmdb_apply_study.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/tests/qmdb_apply_study.rs#L79-L118)

#### Mechanism
In [`apply_study_preserves_values_duplicates_and_engine_roots`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/tests/qmdb_apply_study.rs#L79-L118):
- `Any` (`any::unordered_btree`) is executed twice under independent contexts (`any_ctx` and `repeat_ctx`), asserting `assert_eq!(first, check(any, kind, skew).await)`.
- `Current` (`current::ordered`) is executed only once per `(kind, skew)` combination.

#### Problem
While `Current` checks that `db.root() == expected_batch_root` and verifies all key-value pairs via `db.get`, it does not assert that two completely separate runs of `Current` produce identical Merkle roots when given identical inputs. Any non-determinism specific to `Current`'s activity bitmap or ordered index across separate process instances would not be caught by this assertion.

#### Recommended Mitigation
- Add a second execution of `Current` in `repeat_current_ctx` and assert `assert_eq!(first_current, second_current)`.

---

### Hazard 2.2: Mocked Merkleization in `timing_probe`
- **Affected File:** [`storage/tests/qmdb_apply_study.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/tests/qmdb_apply_study.rs#L121-L278)

#### Mechanism
In the `timing_probe` module:
- `Batch::merkleize()` returns `self.root` directly without performing cryptographic hashing.
- `Db::apply_batch()` checks `assert_eq!(batch.root, self.root)` and sleeps 15 ms.

#### Problem
`timing_probe` tests that [`apply_prepared`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/apply_study.rs#L26-L40) isolates the call to `apply_batch`, but it does not test:
- The actual Criterion `b.iter_custom` closure in `bench_apply_study`.
- Real snapshot cloning, runtime scoping, or directory deletion behavior.

#### Recommended Mitigation
- Rely on the integration tests in [`test-apply-benchmark.py`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/tests/test-apply-benchmark.py) to cover end-to-end benchmark loops, and document `timing_probe` as a targeted unit test for `apply_prepared` boundaries only.

---

### Hazard 2.3: `mixed` Mutation Type Rejected by Summary Pipeline
- **Affected Files:**
  - [`storage/src/qmdb/benches/apply_study.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/apply_study.rs#L58)
  - [`measurements-rain/scripts/compare-experiment.jq`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/compare-experiment.jq#L66-L85)

#### Mechanism
`apply_study.rs` accepts `QMDB_MERKLEIZE_WRITE_TYPE=mixed`, and [`summarize-criterion.sh`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/summarize-criterion.sh#L51) correctly parses `w=mixed`. However, [`compare-experiment.jq`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/compare-experiment.jq#L66-L85) expects strictly 3 write types (`overwrite`, `insert`, `delete`) when analyzing `write-type` experiments.

#### Problem
Attempting to run a standard comparative suite with `mixed` writes causes `compare-experiment.jq` to abort with `expected three configurations per write type`.

#### Recommended Mitigation
- Document that `mixed` is supported for standalone microbenchmarks and ad-hoc experiments, but is excluded from the standard comparative matrix suite.

---

### Hazard 2.4: Sandbox Shebang Restriction on `test-apply-study.py`
- **Affected File:** [`measurements-rain/scripts/tests/test-apply-study.py`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/tests/test-apply-study.py#L97-L111)

#### Mechanism
`test_launchers_with_fixture_cargo` writes a stub `cargo` executable with `#!/usr/bin/env python3` into a temporary directory created under system temp (`/var/folders/...` on macOS).

#### Problem
In environments with strict OS process isolation or sandbox policies (e.g. macOS sandbox restrictions), executing a shebang binary pointing to `/usr/bin/env` from a temporary directory triggers `Operation not permitted (error 126)`.

#### Recommended Mitigation
- Invoke the fixture via explicit `sys.executable` or execute the script directly rather than relying on shebang path resolution in temporary directories.

---

## 3. Methodological & Durability Invariants

### Invariant 3.1: Buffered Latency vs. Durable Write Latency
- **Timing Scope:** `apply_batch`
- **Durability Tier:** `buffered`

`apply_study.rs` measures the CPU and in-memory time to update indexes, graft bitmap chunks, and append records into OS page-cache log buffers. It does **not** invoke `commit()` or `sync()` (`fsync` flash barriers).
- **Do not** equate `apply_study` results to durable transaction latency.
- **Do not** sum `merkleize` duration and `apply_study` duration to claim an end-to-end transaction cost without accounting for `fsync` barriers and pipeline concurrency.

### Invariant 3.2: Warm Page Cache Bias
- In `apply_study.rs`, `cc=false` is enforced: the Commonware page cache is intentionally retained between `merkleize` and `apply_batch`.
- Because inputs are fixed (`TestRng::new(99)`), repeated iterations access the identical hot pages in cache. Real-world continuous batch application with varied keys will experience cold page misses and journal boundary rotations that this microbenchmark explicitly excludes.

---

## Summary Matrix

| ID | Issue | Severity | Scope | Status |
| :--- | :--- | :--- | :--- | :--- |
| **1.1** | Criterion iteration scaling explosion at 4M/10M keys | **High** | Experiment Runners / Benchmarks | **Resolved**: Clamped sample window (`sample_size=10`, `warm_up_time=20ms`, `measurement_time=10ms`), added `iters <= 2` guard in `apply_study.rs`, and forwarded `criterion_study_flags` across all 4 runners |
| **1.2** | Double database open per iteration | **Medium** | Untimed Benchmark Setup | **Resolved**: Added `snapshots::prepare_scratch` to eliminate redundant `verify_open` on snapshot clones |
| **1.3** | Redundant write generation in `for 0..iters` | **Low** | Benchmark Setup | **Resolved**: Hoisted deterministic write vector preparation outside `iter_custom` |
| **2.1** | Missing second determinism run for `Current` | **Low** | Test Suite (`qmdb_apply_study.rs`) | **Resolved**: Added `repeat_current_ctx` cross-run root determinism check |
| **2.2** | Mocked Merkleization in `timing_probe` | **Low** | Unit Test (`timing_probe`) | Architectural limitation; covered end-to-end by `test-apply-benchmark.py` |
| **2.3** | `mixed` write type rejected by `compare-experiment.jq` | **Low** | Tooling / Reporting | Design constraint (microbenchmarks only; excluded from 3-type comparative suite) |
| **2.4** | Temporary shebang failure in strict sandbox | **Low** | Fixture Test (`test-apply-study.py`) | **Resolved**: Used explicit `sys.executable` in fixture stub shebang so tests run cleanly in sandboxed environments |
