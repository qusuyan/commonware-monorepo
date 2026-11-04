# QMDB Research & Implementation TODOs

A consolidated roadmap of pending tasks, architectural implementations, and scientific controls for the QMDB Authentication Overhead & Cryptographic Tax study.

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

### Priority 1: Disentangling Indexer Overhead (PROP-01 & B-Tree Control)
- [x] **Define Ordered-Index Unordered Type Alias (`BTreeDb`):**
  - In `storage/src/qmdb/current/unordered/fixed.rs` and `storage/src/qmdb/benches/common.rs`, defined `BTreeDb` and `CurUFixBTreeDb<F>`.
- [x] **Expose Control Variant in `merkleize.rs`:**
  - Added variant selector: `current::unordered_btree::fixed::mmb chunk=32` (and MMR).
- [ ] **Build 3-Way Runner Script:**
  - Create `measurements-rain/experiments/indexer-control/run.sh`.
  - Execute write-type sweep (10K writes, 10M keys) comparing Config A, B, and C across 3 repeats.
- [ ] **Extend JSON Comparison Schema:**
  - Output explicit fields for $\Delta_{\text{structure}}$ and $\Delta_{\text{protocol}}$.

---

### Priority 2: Persistent Initial Key Set & Pre-Seeded DB Snapshot Reuse
> [!TIP]
> **Motivation & Speedup:** Seeding 10,000,000 keys currently takes minutes per run, generating identical keys sequentially via `Sha256(i)`. Because `batch.merkleize()` is speculative in memory, the on-disk seeded database remains completely intact and pristine. Saving both the raw initial key set file and the pre-seeded DB directory eliminates repetitive setup overhead and guarantees byte-level determinism.

- [ ] **Pre-Generated Binary Key Set (`keys_10M.bin`):**
  - Store a contiguous binary file of pre-computed 32-byte SHA-256 keys ($32 \times 10{,}000{,}000 = 320\text{ MB}$).
  - Verify size and integrity: exactly $320{,}000{,}000$ bytes, $10{,}000{,}000$ unique 32-byte digests.
  - Implement zero-copy `mmap` or fast sequential read in `common.rs` to load the entire 10M keyspace in $< 50\text{ ms}$ instead of running 10M hashing cycles.
- [ ] **Pre-Seeded Database Directory Reuse:**
  - Configure `commonware_runtime::tokio::Config` with a deterministic path via `.with_storage_directory(dir)` (e.g. `storage-seeds/10M_mmb_<variant>`).
  - In `MerkleizeWorkload::setup()`:
    - Check if the database partition files already exist on disk.
    - If present: Skip `seed_db(db, 10M)` entirely and simply open the database (takes $< 1\text{ second}$).
    - If absent: Run `seed_db()` once and leave files intact.
  - In `MerkleizeWorkload::teardown()`:
    - Bypass `db.destroy()` so the pre-seeded database is preserved for subsequent benchmark iterations and process repeats.

---

### Priority 3: Deletion Workloads & Activity Invalidation (Former Task 6)
- [ ] **Implement `WriteType::Delete` Support:**
  - Add `Delete` to `WriteType` enum in `storage/src/qmdb/benches/common.rs`.
  - Wire up `batch.write(k, None)` staging in `prepare_sampled_writes` / `stage_prepared_writes`.
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
- [ ] **Process Resource Telemetry (`rusage` / `sysinfo`):**
  - Instrument macro-benchmarks (10M key seeding and batch commits) with:
    - User CPU Time vs System/Kernel CPU Time.
    - Peak Resident Set Size (RSS memory in MB).
    - Disk I/O bytes read/written per second.

---

### Priority 5: Zero-Proof Unauthenticated Baseline (PROP-02, Former Task 4)
- [ ] **Integrate `qmdb::store::Db`:**
  - Wire up Commonware's raw key-value store [`qmdb::store::Db`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/store/db.rs) into the benchmark suite.
- [ ] **Implement Unmerkleized Speculative Harness:**
  - Benchmark batch staging and index lookup with zero SHA-256 leaf hashing and zero Merkle tree math.
- [ ] **Calculate Absolute Cryptographic Tax:**
  - $T_{\text{log}} = \text{Latency}(\text{any}) - \text{Latency}(\text{store})$.
  - $T_{\text{total}} = \text{Latency}(\text{current\_ordered}) - \text{Latency}(\text{store})$.

---

### Secondary Active: End-to-End Durable Write Lifecycle (PROP-04, Former Task 3)
- [ ] **Overhaul `apply_batch.rs`:**
  - Modernize `storage/src/qmdb/benches/apply_batch.rs` to support:
    - `current::unordered` and `current::ordered`.
    - `WriteSampler` (Zipfian skew and uniform access).
    - `WriteType` (Overwrites vs Inserts).
    - `prepare_sampled_writes` / `stage_prepared_writes` (timer boundary isolation).
- [ ] **Benchmark the 3 Durability Tiers:**
  - **Tier 1 (Speculative):** `merkleize()` only (in-memory).
  - **Tier 2 (Buffered Append):** `apply_batch()` (OS page cache append).
  - **Tier 3 (Durable Sync):** `apply_batch()` + `commit()` + `sync()` (`fsync` flash barrier).
- [ ] **Instrument Write Amplification Factor (WAF):**
  - Record exact physical bytes appended to disk per 10K writes.
  - Validate the 2:1 write amplification hypothesis for `current::ordered` inserts (new entry $E_K$ + predecessor update $E'_p$).
- [ ] **Create Runner:**
  - Create `measurements-rain/experiments/durable/run.sh`.

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
