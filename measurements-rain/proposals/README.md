# QMDB Proposals & Architecture Documentation Suite

This directory contains the formal documentation of all research proposals, design decisions, code modifications, and experimental methodologies developed for the **QMDB (Quick Merkle Database) Authentication Overhead Study** in [`commonware-monorepo`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo).

---

## 1. Project Purpose & Research Framework

The central research question guiding this investigation is:

> **What is the exact "cryptographic tax" of authenticated storage in append-only blockchain databases, and how does each individual cryptographic guarantee impact the write and read paths?**

Traditional blockchain storage engines (such as Ethereum's Merkle Patricia Trie or Jellyfish Merkle Tree) authenticate data by continuously reorganizing state into authenticated key-value search trees. While this natively supports both **inclusion proofs** (proving key $K$ has value $V$) and **exclusion proofs** (proving key $K$ does not exist), it incurs severe random disk I/O penalties and CPU tree-rebalancing costs.

QMDB fundamentally re-architects storage around an **append-only operation log** paired with append-friendly Merkle Mountain Ranges and Merkle Mountain Belts (**MMR** / **MMB**). 

> [!IMPORTANT]
> **Log Integrity is a Mandatory Invariant**
> A non-negotiable requirement of QMDB is maintaining **cryptographic log integrity**: every state transition must be committed into an authenticated, tamper-evident append-only Merkle log (`any`). We are **not** proposing to remove the operation log's Merkle tree.
> 
> Instead, our research isolates the overhead of the two optional, specialized authentication layers built *on top* of the Merkle log:
> 1. **Current-State Inclusion Proofs (`current::unordered`):** Tracking live records via an activity bitmap and grafting dirty bitmap chunks into the state root.
> 2. **Exclusion / Non-Membership Proofs (`current::ordered`):** Proving non-existence of a key via ordered B-tree indexing, predecessor span splitting, and `next_key` pointer links.

Our study isolates the performance cost of each mechanism across this hierarchy:

```
┌────────────────────────────────────────────────────────────────────────┐
│ current::ordered       (Log Integrity + Current Inclusion + Exclusion) │  ▲
├────────────────────────────────────────────────────────────────────────┤  │ Exclusion Proof Tax (~31x insert slowdown)
│ current::unordered     (Log Integrity + Current State via Grafting)    │  ▼
├────────────────────────────────────────────────────────────────────────┤  ▲
│ any::unordered (btree) (MANDATORY BASELINE: Log Integrity & History)   │  │ Bitmap Grafting Tax (~2.1x overwrite slowdown)
├────────────────────────────────────────────────────────────────────────┤  ▼
│ raw store / contiguous [Reference Only: Unauthenticated, breaks integrity]
└────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Directory Structure

The proposals are cleanly partitioned into two directories:

```
measurements-rain/proposals/
├── README.md                      # This directory guide and research overview
├── implemented/                   # Completed proposals, code modifications, and empirical results
│   ├── 01-write-size-decoupling.md
│   ├── 02-write-type-workloads.md
│   ├── 03-access-skewness-zipf.md
│   ├── 04-timer-boundary-isolation.md
│   ├── 05-component-attribution-benchmarks.md
│   ├── 06-matrix-automation-and-statistics.md
│   ├── 07-btree-indexer-control.md
│   └── 08-apply-study-batch-application.md
└── proposed/                      # Planned architectural proposals for ongoing and future study
    ├── 03-client-proof-benchmarks.md
    ├── 04-durable-write-lifecycle.md
    ├── 05-persistent-keyset-and-db-snapshot-reuse.md
    └── 06-pure-deletion-workloads-activity-invalidation.md
```

---

## 3. Proposal Index

### Implemented Proposals ([`implemented/`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/proposals/implemented))

| ID | Title | Core Focus | Primary Code Locations | Status |
| :--- | :--- | :--- | :--- | :---: |
| **IMP-01** | [Write-Size Decoupling](implemented/01-write-size-decoupling.md) | Decoupling batch mutation count from database size (`u=200..10K`) | [`common.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/common.rs), [`merkleize.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/merkleize.rs) | **Merged (PR #1)** |
| **IMP-02** | [Write-Type Workloads](implemented/02-write-type-workloads.md) | Isolating Overwrites vs. Inserts & exposing the Dual Asymmetry | [`common.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/common.rs), [`write-type/`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/experiments/write-type) | **Merged (PR #2)** |
| **IMP-03** | [Access Skewness & Zipf](implemented/03-access-skewness-zipf.md) | Modeling hot-key temporal locality with a numerically stable sampler | [`common.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/common.rs), [`skew/`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/experiments/skew) | **Merged (PR #3)** |
| **IMP-04** | [Timer Boundary Isolation](implemented/04-timer-boundary-isolation.md) | Moving random input generation outside Criterion timed iteration | [`merkleize.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/merkleize.rs), [`qmdb_prepared_writes.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/tests/qmdb_prepared_writes.rs) | **Implemented** |
| **IMP-05** | [Component Attribution](implemented/05-component-attribution-benchmarks.md) | Microbenchmarks isolating leaf hashing, tree ops, & bitmap mutation | [`components.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/components.rs), [`Cargo.toml`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/Cargo.toml) | **Implemented** |
| **IMP-06** | [Matrix Automation & Stats](implemented/06-matrix-automation-and-statistics.md) | 12-step matrix runner, rotated repeats, & automated figures | [`run-full-matrix.sh`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/run-full-matrix.sh), [`scripts/`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts) | **Implemented** |
| **IMP-07** | [B-Tree Indexer Control](implemented/07-btree-indexer-control.md) | Standardizing on B-Tree indexer as an experimental control | [`fixed.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/current/unordered/fixed.rs), [`merkleize.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/merkleize.rs) | **Implemented** |
| **IMP-08** | [Apply Study & Batch Application](implemented/08-apply-study-batch-application.md) | Phase 3 batch application, snapshot scratch isolation, and workload dimension parity | [`apply_study.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/apply_study.rs), [`experiments/apply/`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/experiments/apply) | **Implemented** |

---

### Proposals retained under `proposed/` ([`proposed/`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/proposals/proposed))

This directory includes future designs as well as implemented changes that remain
there while review is pending. The status column is authoritative.

| ID | Title | Core Focus | Target Components | Status |
| :--- | :--- | :--- | :--- | :---: |
| **PROP-03** | [Client Proof Benchmarks](proposed/03-client-proof-benchmarks.md) | Read-path: proof generation, verification time, & wire byte size | [`current/ordered/db.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/current/ordered/db.rs), [`current/unordered/db.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/current/unordered/db.rs) | **Planned** |
| **PROP-04** | [Durable Write Lifecycle](proposed/04-durable-write-lifecycle.md) | Disk persistence: `apply_batch` $\rightarrow$ `commit` $\rightarrow$ `sync` | [`apply_batch.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/apply_batch.rs) | **Planned** |
| **PROP-05** | [Persistent Key Set & DB Snapshot Reuse](proposed/05-persistent-keyset-and-db-snapshot-reuse.md) | Pre-computed key files and pre-seeded snapshot caching (`current::ordered` vs `any::unordered_btree`) | [`common.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/common.rs), [`merkleize.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/merkleize.rs) | **Implemented / Review** |
| **PROP-06** | [Overwrite, Insert, Delete](proposed/06-pure-deletion-workloads-activity-invalidation.md) | Pure workload comparison using `any_unordered_btree` versus `current_ordered` | [`common.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/common.rs), [`write-type/`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/experiments/write-type) | **Implemented / Review** |
| **PROP-07** | [CPU Metrics & Profiling](proposed/07-cpu-metrics-and-profiling.md) | Native `libc` user/kernel CPU accounting for merkleize/apply, plus periodic `sysinfo` CPU monitoring; hardware counters deferred | [`cpu.rs`](../../storage/src/qmdb/benches/cpu.rs), [`run-cpu-telemetry.sh`](../scripts/run-cpu-telemetry.sh) | **Native accounting implemented; Linux validation pending** |
| **PROP-09** | [Merkleize Latency Breakdown](proposed/09-merkleize-latency-breakdown.md) | Sub-phase wall-clock attribution inside `merkleize()` explaining insert vs. overwrite asymmetry | [`any/batch.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/any/batch.rs), [`current/batch.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/current/batch.rs) | **Proposed** |

---

## 4. Key Empirical Takeaways from Current Findings

The following are historical findings from the earlier 11-step, 10M-key/10K-write
matrix. They should not be combined with the current 4M-key/2K-write study:

1. **The Exclusion Proof Insert Penalty:** Maintaining an ordered index and successor pointers for exclusion proofs adds a **~31.3x slowdown** (3,132% overhead) to batch Merkleization under inserts (`current_ordered` = 138.66 ms vs. `current_unordered` = 4.38 ms for 10K writes).
2. **The Bitmap Grafting Overwrite Penalty:** Authenticating active records via bitmap chunk grafting adds a **~2.1x slowdown** (115% overhead) under uniform overwrites (`current_unordered` = 122.25 ms vs. `any_unordered` = 57.42 ms for 10K writes).
3. **The Locality Shield:** When access is heavily skewed (Zipf $s = 2.0$), overwrites cluster into fewer bitmap chunks. Batch latency drops from **110.5 ms to 4.2 ms** (a 96% reduction in latency) because fewer grafted leaves are dirty and upper trees require minimal re-hashing.
