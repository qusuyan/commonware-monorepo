# QMDB Research & Implementation TODOs

A consolidated roadmap of pending tasks, architectural implementations, and scientific controls for the QMDB Authentication Overhead & Cryptographic Tax study.

---

## 🔒 Non-Negotiable Constraint: Log Integrity via Merkle Tree is Required

> [!IMPORTANT]
> **QMDB Integrity Invariant**
> Maintaining **cryptographic log integrity** via Merkle Mountain Ranges or Belts (MMR/MMB) over the append-only operation log is a mandatory, core requirement of QMDB. We are **not** proposing to remove the log's Merkle tree or deploy an unauthenticated store.
>
> - **The True Production Baseline is `any::unordered` (specifically `any::unordered_btree`):** It guarantees full cryptographic log integrity, historical state roots, and tamper-evident auditing.
> - **What We Are Investigating:** Strictly whether to keep or drop:
>   1. Current-state inclusion proofs via activity bitmap grafting (`current::unordered`).
>   2. Exclusion / non-membership proofs via ordered indexer and predecessor links (`current::ordered`).
> - **Role of `store`:** Raw `store` is strictly an academic hardware-limit reference, NOT an acceptable production configuration for QMDB because dropping log Merkleization breaks tamper-evidence and consensus state commitment.

---

## 🚨 Critical Control Variable: Standardizing on the B-Tree Indexer

> [!IMPORTANT]
> **Eliminating the Indexer Confounding Variable**
>
> In our current benchmarks, comparing `current::ordered` against `current::unordered` conflates two independent factors:
> 1. **The Cryptographic Exclusion Proof Protocol:** Querying lexicographical predecessors, reading $E_p$, appending 2 physical log records ($E_K$ and $E'_p$), and tracking `old_next_key_id`.
> 2. **The In-Memory Indexer Data Structure:** `current::unordered` uses a `HashMap` ($O(1)$ amortized lookups, high cache locality), whereas `current::ordered` uses a sorted `BTreeMap` / partitioned array ($O(\log N)$ binary searches, array element shifts on insertion, higher cache misses).
>
> **Mandatory Rule for Fair Attribution:**
> - To isolate the true cryptographic tax, **we must ensure they all use the B-Tree (ordered) indexing data structure** as a controlled baseline.
> - Because Commonware's `crate::index::partitioned::ordered::Index` implements the `UnorderedIndex` trait, we can configure `current::unordered` to use the exact same ordered B-Tree indexer.
> - This gives us the complete 3-way control decomposition:
>   - **Config A:** `current::unordered` with `HashMap` (Production unordered baseline)
>   - **Config B (Control):** `current::unordered` with `B-Tree` (Unordered logic on ordered index)
>   - **Config C:** `current::ordered` with `B-Tree` (Full exclusion proofs)
>   - **Pure Indexer Overhead:** $\Delta_{\text{structure}} = \text{Latency}(B) - \text{Latency}(A)$
>   - **Pure Exclusion Protocol Overhead:** $\Delta_{\text{protocol}} = \text{Latency}(C) - \text{Latency}(B)$

---

## ⚡ Active & Prioritized Tasks

### Priority 0: In-Depth Latency Breakdown for Insert vs. Overwrite (Abnormal Insert Latency Investigation)

> [!IMPORTANT]
> **Observation & Motivation**
> In our write-type benchmark measurements ($K = 4\text{M}$ keys, $u = 2,000$ operations), `any::unordered_btree` demonstrated an abnormally fast latency for **insert** ($1.15\text{ ms}$) compared to **overwrite** ($9.90\text{ ms}$) and **delete** ($8.03\text{ ms}$) — an ~8.6x speedup.
> In contrast, `current::ordered` insert latency remained high ($11.96\text{ ms}$ vs $13.68\text{ ms}$ overwrite, only 12.6% faster).
>
> Following mentor discussion, we are conducting an in-depth internal latency breakdown to instrument, isolate, and quantify the micro-stages driving this performance gap.

#### Identified Root Causes in QMDB Architecture
1. **Zero Journal Reads on Insert (`gather_existing_locations`):**
   - For `any::unordered_btree` inserts, none of the keys exist in the snapshot (`db.snapshot.get(key)` returns `None`).
   - Consequently, `gather_existing_locations` produces an empty list and skips `read_ops` completely (0 scattered log reads vs 2,000 scattered disk/cache reads for overwrites).
2. **Compaction Floor Scan Bypass & `user_steps` Anomaly (`any/batch.rs`):**
   - In `merkleize_with_floor_scan`, `total_steps = user_steps + 1`.
   - `user_steps += 1` is incremented inside `emit()` when resolving existing keys (overwrites/deletes).
   - In the `creates` loop (lines 2145–2170), `user_steps` is **NOT incremented**! Thus, `user_steps = 0` and `total_steps = 1` regardless of batch size ($u = 2,000$).
   - As a result, the compaction floor only advances **1 single step** instead of scanning, reading, and moving 2,001 active operations from the bottom of the log to the tip. This cuts the resulting Merkle tree size roughly in half (~2,001 leaves for insert vs ~4,001 leaves for overwrite).
3. **Bitmap Invalidation & Predecessor Rewriting in `current::ordered`:**
   - In `current::ordered`, every insert must execute `find_prev_key` in the ordered index and append an updated predecessor entry ($E'_p$) pointing `next_key` to $K$.
   - This invalidates $E_p$'s old location, flipping bits to 0 in historical activity chunks, dirtying historical chunks, and forcing full upper-tree Merkle grafting.

#### Planned Action Items
- [ ] **Stage-Level Micro-Instrumentation in `merkleize`:**
  - Instrument high-resolution nanosecond timers (`std::time::Instant` or lightweight trace spans) across 4 discrete stages in `merkleize()`:
    - **Stage 1 (Snapshot Resolution):** Key lookups in index / snapshot (`gather_existing_locations` / `find_prev_key`).
    - **Stage 2 (Historical Journal I/O):** Reading and deserializing historical operations from log (`read_ops`).
    - **Stage 3 (Compaction & Floor Raising):** Inactivity floor scan, classification, and moving active entries to tip.
    - **Stage 4 (Merkle Tree Construction & Hashing):** Building Merkle trees, activity bitmap chunk overlays, grafting, and computing state roots.
- [ ] **Breakdown Benchmark Harness & Data Collection:**
  - Create micro-benchmark harness or runner script to export granular sub-stage latencies (mean, p50, p99) for:
    - `any::unordered_btree` (Insert vs. Overwrite vs. Delete)
    - `current::ordered` (Insert vs. Overwrite vs. Delete)
    - `current::unordered_btree` (Control variant)
- [ ] **Upstream Design Clarification & Experimental Verification (`user_steps`):**
  - Discuss with mentor whether omitting `user_steps += 1` in `creates` is intentional design (inserts naturally advance the log tip without amortized compaction debt) or an oversight.
  - Benchmark an experimental variant with `user_steps += 1` in `creates` to measure insert latency when compaction is charged equally.
- [ ] **Stacked Latency Breakdown Visualizations:**
  - Generate stacked bar charts decomposing total `merkleize` time into the 4 constituent stages for all write types.

---

### Priority 1: Disentangling Indexer Overhead (PROP-01 & B-Tree Control)
- [x] **Define Ordered-Index Unordered Type Alias (`BTreeDb`):**
  - In `storage/src/qmdb/current/unordered/fixed.rs` and `storage/src/qmdb/benches/common.rs`, defined `BTreeDb` and `CurUFixBTreeDb<F>`.
- [x] **Expose Control Variant in `merkleize.rs`:**
  - Added variant selector: `current::unordered_btree::fixed::mmb chunk=32` (and MMR).
- [ ] **Build 3-Way Runner Script:**
  - Create `measurements-rain/experiments/indexer-control/run.sh`.
  - Execute the indexer-control sweep at the current 4M-key, 2K-write anchor across 3 repeats.
- [ ] **Extend JSON Comparison Schema:**
  - Output explicit fields for $\Delta_{\text{structure}}$ and $\Delta_{\text{protocol}}$.

---

### Priority 2: Persistent Initial Key Set & Pre-Seeded DB Snapshot Reuse

- [x] Canonical keysets with whole-file checksum sidecars and process-local caching.
- [x] Shared Merkleization configuration and schema-2 validated saved databases.
- [x] Immutable masters, isolated scratch copies, and verified reopen before timing.
- [x] Required/auto/off policies and setup provenance archival.
- Snapshot support is limited to `any_unordered_btree` and `current_ordered`.
- Full-scale keysets/databases are prepared explicitly; implementation tests use small datasets.
- See [snapshot reuse](snapshot-reuse.md) for commands and validation coverage.

---

### Priority 3: Deletion Workloads & Activity Invalidation (Former Task 6)
- [x] **Implement `WriteType::Delete` Support:**
  - `Delete` is wired through prepared writes, Criterion selection, the pairwise
    write-type runner, comparison output, and regression tests.
- [ ] **Measure Pure Bitmap Invalidation Costs:**
  - Benchmark the cost of flipping bitmap bits from `1` to `0` without appending new active entries.
  - Observe dirty chunk re-hashing behavior under uniform vs. Zipfian skewed deletions.
- [ ] **Evaluate Recovery & Floor-Raising:**
  - Test pruning and compaction boundary updates (`inactivity_floor_loc`) caused by sustained delete streams.

---

### Priority 4: CPU Consumption & Low-Level Profiling (PROP-07, Former Task 5)
- [x] **B-Tree Standardized Baseline Alignment (Strict 2-Way Comparison):**
  - Standardized both `current::ordered` and `any::unordered` on the B-Tree indexer (`any::unordered_btree::fixed::mmb`) to eliminate data structure bias from instruction counting.
  - Defined `any::unordered::fixed::BTreeDb` and registered variant selector `"any::unordered_btree::fixed::mmb"` in `merkleize.rs` and `common.rs`.
  - Focus strictly on the 2-way comparison: `any+unordered_btree` vs. `current+ordered`.
- [ ] **Callgrind Instruction Counting (Deterministic Profiling):**
  - Parameterize and run `measurements-rain/scripts/run-callgrind.sh` across:
    - `any::unordered_btree` (B-Tree Baseline)
    - `current::ordered` (B-Tree Full Authenticated)
  - Record exact Instruction Reads ($Ir$), Data Reads ($Dr$), and Data Writes ($Dw$).
- [ ] **Hardware Performance Counters (`perf-event` on Linux):**
  - Add optional PMU telemetry tracking:
    - `CPU_CYCLES` and `INSTRUCTIONS` (to calculate Instructions Per Cycle - IPC).
    - `CACHE_MISSES` (L1-dcache and LLC misses to prove B-Tree memory stalls).
    - `BRANCH_MISSES` (branch misprediction rates in binary search loops).
- [x] **Native CPU accounting (`libc`):**
  - Shared `commonware-bench::cpu` start/finish API records user/kernel CPU time,
    context switches and page faults across all process threads.
  - `qmdb_cpu` measures one independent merkleize/apply batch per sample;
    `run-cpu-telemetry.sh` captures raw samples, summaries, overhead and provenance.
  - Native accounting remains separate from Criterion and Callgrind regression gates.
- [x] **Process CPU monitoring (`sysinfo`):**
  - Runtime samples CPU alongside memory every 10 seconds; utilization is exported
    in basis points (10,000 = one core), after priming.
  - CPU monitoring is configurable and disabled in native CPU benchmark runs.
- [ ] **Additional resource studies:**
  - Per-run peak RSS with correctly normalized platform units and isolated processes.
  - Disk I/O bytes read/written per second.
  - Linux execution validation and actual hardware counters.

---

### Priority 5: Overhaul `apply_batch.rs` & Phase 3 Batch Application (IMP-08 / PROP-04)

> [!IMPORTANT]
> **Bridging Speculative Crypto (`merkleize`) to Real Storage Persistence (`apply_study`)**
>
> While upstream `storage/src/qmdb/benches/apply_batch.rs` remains untouched upstream code (65K keys toy scale), we implemented a dedicated Phase 3 benchmark harness—[`apply_study.rs`](../storage/src/qmdb/benches/apply_study.rs) (IMP-08)—that brings full scientific control parity, pre-seeded snapshot scratch isolation, and workload dimension controls to batch application.

- [x] **Phase 3 Batch Application Harness (`apply_study.rs` / IMP-08):**
  - **Standardized B-Tree Pair:** Compares `any::unordered_btree::fixed::mmb` vs. `current::ordered::fixed::mmb chunk=32`.
  - **Scale Scaling:** Dynamically configured via `QMDB_MERKLEIZE_KEYS` (defaulting to the $4\text{M}$ keys anchor) and `QMDB_MERKLEIZE_UPDATES`.
  - **Decoupled Setup via Snapshots:** Pre-seeded master databases cloned into per-iteration scratch directories. Master checksums verified, and setup/teardown strictly excluded from timing.
  - **Workload Controls:** Full support for `overwrite`, `insert`, `delete`, and `mixed` write types alongside Zipfian skew ($s \in [0.0, 2.0]$).
  - **Matrix Automation:** Seamlessly toggled across all existing experiment runners (`writesize`, `write-type`, `skew`, `db-scale`) via `export QMDB_STUDY_OPERATION=apply`.
  - **Verification:** Covered by deterministic tests (`qmdb_apply_study.rs`) and Python fixtures (`test-apply-study.py`).
- [ ] **Optional Reference: `qmdb::store::Db` Hardware Limit (Zero Cryptography):**
  - Wire up Commonware's raw key-value store [`qmdb::store::Db`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/store/db.rs) strictly as an academic reference point.
  - Time `store::Db::apply_batch` with raw `Changeset<K, V>` to isolate the absolute hardware ceiling when all Merkle integrity is absent.
- [ ] **Phase 4: Durable Sync Tier & Write Amplification Factor (WAF):**
  - Benchmark the 3 Durability Tiers:
    - **Tier 1 (Speculative):** `merkleize()` only (in-memory math).
    - **Tier 2 (Buffered Append):** `apply_batch()` (log buffer append + index update — measured by `apply_study`).
    - **Tier 3 (Durable Sync):** `apply_batch()` + `commit()` + `sync()` (`fsync` flash barrier).
  - Instrument physical bytes written to log files to validate the 2:1 write amplification hypothesis for `current::ordered` inserts (new entry $E_K$ + predecessor update $E'_p$).
- [ ] **Dedicated Durable Experiment Runner:**
  - Create `measurements-rain/experiments/durable/run.sh` to run durable sync persistence benchmarks across `store`, `any_unordered`, and `current_ordered`.

---

## ⏸️ On Hold (Not Urgent / Optional Follow-Up)

> [!NOTE]
> The following items are deferred for later phases as they are either read-path evaluations or secondary parameter sweeps that do not block the primary write-path attribution study.

### [ON HOLD] Task 2: Client-Side Proof Generation & Verification Benchmarks (PROP-03)
- **Why on hold:** Focus is currently on the server/validator write-path bottleneck (`merkleize` & `apply_batch`). Read-path light client proofs can be evaluated after write-path attribution is finalized.
- [ ] Register `[[bench]] name = "qmdb_proofs"` in `storage/Cargo.toml`.
- [ ] Benchmark server proof generation: `key_value_proof(key)` vs `exclusion_proof(key)`.
- [ ] Benchmark client verification: `verify_key_value_proof` vs `verify_exclusion_proof`.
- [ ] Record serialized wire footprint (`proof.encode_size()`).
- [ ] Create `measurements-rain/experiments/client-proofs/run.sh`.

### [ON HOLD] Task 7: Database Footprint Scaling
- **Why on hold:** 10M keys is already established as the primary standard scale for our paper. Scaling down to 100K/1M or up to 50M keys is a follow-up sensitivity study.
- [ ] Run sweeps across database sizes: $N \in \{100\text{K}, 1\text{M}, 10\text{M}, 50\text{M}\}$ keys to observe how cache miss frequency scales with tree depth.

### [ON HOLD] Task 8: Chunk Size Variation
- **Why on hold:** 32-byte chunking is the standard baseline across all experiments. Evaluating 16-byte or 64-byte chunking is a secondary tuning optimization.
- [ ] Compare `current::*` performance with alternate chunk sizes ($N = 16$, $N = 32$, $N = 64$ bytes) to evaluate dirty bitmap hashing frequency vs proof payload size.
