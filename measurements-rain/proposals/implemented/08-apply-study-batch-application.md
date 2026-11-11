# Proposal 08: Apply Study: Controlled Batch Application, Scratch Snapshot Isolation, and Workload Dimension Parity

- **Proposal ID:** IMP-08
- **Status:** Implemented
- **Author:** Rain Sun
- **Target Files:**
  - [`storage/src/qmdb/benches/apply_study.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/apply_study.rs)
  - [`storage/src/qmdb/benches/bench.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/bench.rs)
  - [`storage/tests/qmdb_apply_study.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/tests/qmdb_apply_study.rs)
  - [`measurements-rain/experiments/apply/README.md`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/experiments/apply/README.md)
  - [`measurements-rain/scripts/experiment-configs.sh`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/experiment-configs.sh)
  - [`measurements-rain/scripts/compare-experiment.jq`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/compare-experiment.jq)
  - [`measurements-rain/scripts/summarize-isolated-suite.py`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/summarize-isolated-suite.py)
  - [`measurements-rain/scripts/tests/test-apply-study.py`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/tests/test-apply-study.py)
  - [`measurements-rain/scripts/tests/test-apply-benchmark.py`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/tests/test-apply-benchmark.py)

---

## 1. Problem Statement & Motivation

Our primary benchmark harness ([`merkleize.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/merkleize.rs)) isolates **Phase 2 (Speculative Cryptographic Merkleization)**: it builds an in-memory batch, computes the cryptographic state root, and discards the batch without applying it to the database or persisting changes to disk.

However, in production storage engines, writes pass through four distinct phases:

```text
       PHASE 1                  PHASE 2                  PHASE 3                  PHASE 4
User / Tx Execution      Cryptographic Proof       Database Application      Physical Persistence
┌────────────────────┐   ┌─────────────────────┐   ┌─────────────────────┐   ┌─────────────────────┐
│ batch.write(k, v)  │──►│  batch.merkleize()  │──►│ db.apply_batch(b)   │──►│ db.commit() / sync()│
└────────────────────┘   └─────────────────────┘   └─────────────────────┘   └─────────────────────┘
      In RAM                    In CPU/RAM                In OS Page Cache             On Flash SSD
 (Buffer staging)         (Merkle root calc)        (Appended to files)       (NAND cells flushed)
```

To evaluate the true storage tax of authentication structures, we need to benchmark **Phase 3 (`apply_batch`)**: ingesting authenticated batches, updating in-memory indexers, mutating activity bitmap chunks, and appending records to journal log buffers.

### Limitations of Upstream `apply_batch.rs`
The upstream repository contained an existing benchmark file ([`storage/src/qmdb/benches/apply_batch.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/apply_batch.rs)), but it suffered from critical architectural limitations that prevented its use for our research:
1. **Hardcoded Toy Scale:** Hardcoded to $K = 65,536$ keys and $u = 16,384$ updates, preventing evaluation at our standard $4\text{M}$ and $10\text{M}$ research anchors.
2. **Missing Variants & Confounded Baselines:** Omitted `current::ordered` entirely. The only variants tested were `any::unordered` and `any::ordered`, conflating data-structure index choices without evaluating current-state bitmap grafting or exclusion proofs.
3. **In-Loop Seeding & Destruction:** Seeded and destroyed the database inside the timed Criterion iteration loop (`open_db -> seed_db -> apply_batch -> db.destroy`). At $4\text{M}$ keys, seeding takes 30–60 seconds per iteration, causing Criterion runs to stall or timeout.
4. **Uniform Overwrites Only:** Lacked support for insert or delete workloads, or Zipfian access skew.
5. **Upstream Contamination:** Directly modifying `apply_batch.rs` would disrupt upstream CI/CD regression gates and create unnecessary merge conflicts.

---

## 2. Solution: Dedicated `apply_study.rs` Benchmark Harness

Rather than modifying upstream files, we implemented a dedicated, isolated benchmark harness: [`storage/src/qmdb/benches/apply_study.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/apply_study.rs).

This harness brings all of our scientific controls, snapshot reuse systems, and workload dimensions to Phase 3 batch application.

### The Measurement Contract & Timing Boundaries

Every iteration strictly enforces:

```text
┌────────────────────────────────────────────────────────────────────────┐
│                        OUTSIDE TIMED ITERATION                         │
├────────────────────────────────────────────────────────────────────────┤
│ 1. Validate master database via SHA-256 manifest sidecar               │
│ 2. Clone master to an isolated temporary scratch directory             │
│ 3. Open scratch database with production descriptor (4 KiB pages,      │
│    16,384 cache pages, 2 MiB write buffer, 8 strategy threads)         │
│ 4. Generate workload writes (sampled with RNG seed 99)                 │
│ 5. Stage writes into batch: stage_prepared_writes(db.new_batch())      │
│ 6. Cryptographically Merkleize batch: batch.merkleize(&db, None)       │
├────────────────────────────────────────────────────────────────────────┤
│                          TIMED REGION ONLY                             │
├────────────────────────────────────────────────────────────────────────┤
│ let start = Instant::now();                                            │
│ let (db, _) = db.apply_batch(batch).await.unwrap();                    │
│ let elapsed = start.elapsed();                                         │
├────────────────────────────────────────────────────────────────────────┤
│                        OUTSIDE TIMED ITERATION                         │
├────────────────────────────────────────────────────────────────────────┤
│ 7. Assert cryptographic state root: assert_eq!(db.root(), expected)   │
│ 8. Drop database handle and unmount/remove scratch directory           │
└────────────────────────────────────────────────────────────────────────┘
```

> [!IMPORTANT]
> **Durability Boundary Specification**
> - **Included in Timing:** In-memory index ingestion, bitmap chunk updates, and appending serialized operation records to OS page-cache log buffers.
> - **Excluded from Timing:** Batch Merkleization (Phase 2), random input synthesis, and `commit()` / `sync()` (`fsync` flash barriers).
> - **Durability Tier:** Buffered batch application latency, not durable sync latency. Master databases remain completely immutable.

---

## 3. Experimental Controls & Workload Dimension Parity

`apply_study.rs` integrates directly with all existing research controls:

### A. Controlled B-Tree Baseline Alignment
To eliminate the index data-structure confounding variable, both engines are standardized on the B-Tree indexer:
- **Baseline:** `any::unordered_btree::fixed::mmb` (`AnyUFixBTreeDb`)
- **Full Authentication:** `current::ordered::fixed::mmb chunk=32` (`CurOFixDb`)

This isolates the true cost of maintaining the ordered predecessor index and activity bitmap grafting during disk application.

### B. Full Workload Matrix Support
`apply_study.rs` shares environment configuration parity with `merkleize.rs`:
- **Keyspace Scale (`QMDB_MERKLEIZE_KEYS`):** Defaults to $4\text{M}$ keys; scales up to $10\text{M}$ keys.
- **Batch Size (`QMDB_MERKLEIZE_UPDATES`):** Configurable batch sizes ($u \in [200 \dots 10,000]$).
- **Mutation Types (`QMDB_MERKLEIZE_WRITE_TYPE`):** Supports `overwrite`, `insert`, `delete`, and `mixed` via `prepare_sampled_writes`.
- **Access Skewness (`QMDB_MERKLEIZE_SKEW`):** Supports uniform ($s=0.0$) and Zipfian power laws ($s \in \{0.5, 1.0, 1.5, 2.0\}$) via `WriteSampler` / `StableZipf`.
- **Snapshot Reuse (`QMDB_SNAPSHOT_REUSE`):** Integrates with `snapshots::prepare(variant, keys, mode)` (`required`, `auto`, `off`).

---

## 4. Automation & Tooling Integration

### Unified Runner Switch (`QMDB_STUDY_OPERATION=apply`)
We updated [`measurements-rain/scripts/experiment-configs.sh`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/experiment-configs.sh) to support global operation switching. By setting:

```bash
export QMDB_STUDY_OPERATION=apply
```

researchers can execute the four primary experiment runners without modifying script code:
- `measurements-rain/experiments/writesize/run.sh`
- `measurements-rain/experiments/write-type/run.sh`
- `measurements-rain/experiments/skew/run.sh`
- `measurements-rain/experiments/db-scale/run.sh`

### Strict Phase Isolation in Reporting
To prevent invalid apples-to-oranges comparisons, [`compare-experiment.jq`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/compare-experiment.jq) and [`summarize-isolated-suite.py`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/summarize-isolated-suite.py) were updated:
- Results are tagged with:
  ```json
  {
    "operation": "apply_study",
    "timing_scope": "apply_batch",
    "durability": "buffered",
    "iteration_policy": "independent_snapshot_fixed_inputs"
  }
  ```
- Comparison scripts strictly reject inputs with mixed operations (`merkleize` vs `apply_study`) or mixed durability scopes.

---

## 5. Verification & Testing

The implementation includes comprehensive verification suites:

1. **Rust Integration Test ([`storage/tests/qmdb_apply_study.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/tests/qmdb_apply_study.rs)):**
   - Validates deterministic state root reproduction across independent runs.
   - Tests `overwrite`, `insert`, `delete`, and `mixed` mutations under both uniform and Zipfian skew ($s=2.0$).
   - Verifies key-value retrieval matches expected contents and validates timer boundaries using artificial delay probes.
2. **Python Fixture Tests ([`measurements-rain/scripts/tests/test-apply-study.py`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/tests/test-apply-study.py)):**
   - Verifies comparison boundary invariants, phase rejection logic, and summary parsers.
3. **End-to-End Benchmark Smoke Test ([`measurements-rain/scripts/tests/test-apply-benchmark.py`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/tests/test-apply-benchmark.py)):**
   - Validates live Criterion execution against pre-seeded snapshots across both engines.

---

## 6. Summary of Architectural Impact

| Aspect | Upstream `apply_batch.rs` | Modern `apply_study.rs` (IMP-08) |
| :--- | :--- | :--- |
| **Compared Variants** | `any::unordered` vs. `any::ordered` | `any::unordered_btree` vs. `current::ordered` |
| **Scientific Control** | None (different indexing data structures) | Standardized B-Tree indexer on both sides |
| **Evaluated Scale** | Toy ($K = 65,536$) | Production research scale ($K = 4\text{M}, 10\text{M}$) |
| **Workload Types** | Uniform overwrites only | Overwrites, Inserts, Deletes, Mixed |
| **Temporal Skew** | Uniform random only ($s = 0.0$) | Full Zipfian distribution sweep ($s \in [0.0, 2.0]$) |
| **Setup Cost / Timing** | Seeding inside timed loop (unviable at 4M) | Decoupled pre-seeded snapshot scratch clone |
| **Correctness Gate** | Basic application | Full post-application cryptographic root assertion |
| **Runner Integration** | Standalone Criterion binary only | Unified matrix runner (`QMDB_STUDY_OPERATION=apply`) |
