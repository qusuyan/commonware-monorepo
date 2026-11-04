# Proposal 05 (Future): Persistent Initial Key Set & Pre-Seeded DB Snapshot Reuse via Copy-on-Write Cloning

Implementation status: the validated keyset/snapshot lifecycle is implemented for
`any_unordered_btree` and `current_ordered`. The authoritative behavior and commands
are documented in [snapshot reuse](../../snapshot-reuse.md). The original design
below is retained as proposal history; fixed millisecond/zero-copy claims are not
performance guarantees.

- **Proposal ID:** PROP-05
- **Status:** Proposed / Design Specification
- **Author:** Rain Sun
- **Target Comparison Focus:**
  - **Variant A (Controlled Baseline):** `any::unordered_btree::fixed::mmb` (Historical append log with Merkle proofs, backed by B-Tree indexer)
  - **Variant B (Full Authenticated):** `current::ordered::fixed::mmb chunk=32` (Current-state inclusion & exclusion proofs, backed by B-Tree indexer)
  *(Note: Rigorous 2-way comparison standardizing both engines on B-Tree indexing to eliminate indexer data structure as a confounding variable).*
- **Aligned Database Sizing Strategy:**
  - **Anchor Baseline ($K = 4\text{M}$ keys):** Used for all workload sweeps (write size, write type, access skew).
  - **Database Scaling Sweep ($K \in \{1\text{M}, 2\text{M}, 4\text{M}, 10\text{M}\}$ keys):** Dedicated scaling study at fixed $u = 10\text{K}$.
- **Target Components:**
  - [`storage/src/qmdb/benches/common.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/common.rs)
  - [`storage/src/qmdb/benches/merkleize.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/merkleize.rs)
  - [`storage/src/qmdb/benches/apply_batch.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/apply_batch.rs)
  - [`measurements-rain/scripts/run-criterion.sh`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/run-criterion.sh)
  - [`measurements-rain/experiments/writesize/run.sh`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/experiments/writesize/run.sh)
  - [`measurements-rain/experiments/write-type/run.sh`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/experiments/write-type/run.sh)
  - [`measurements-rain/experiments/skew/run.sh`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/experiments/skew/run.sh)
  - [`measurements-rain/experiments/db-scale/run.sh`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/experiments/db-scale/run.sh)

---

## 1. Problem Statement & Motivation

Our study uses an **Anchor & Pivot (OFAT)** controlled experimental design comparing **`any_unordered (btree)` vs. `current_ordered`**:
- **Control Anchor:** Fixed at $K = 4\text{M}$ keys, $u = 10\text{K}$ writes, `overwrite`, and uniform access ($s = 0$).
- **The 4 Pivots:**
  1. *Write Size Sweep:* $u \in \{200, 500, 1\text{K}, 2\text{K}, 5\text{K}, 10\text{K}\}$ at fixed $K = 4\text{M}$.
  2. *Write Type Sweep:* `overwrite` vs `insert` at fixed $K = 4\text{M}, u = 10\text{K}$.
  3. *Access Skew Sweep:* Zipf $s \in \{0, 0.5, 1, 1.5, 2\}$ at fixed $K = 4\text{M}, u = 10\text{K}$.
  4. *Database Scale Sweep:* $K \in \{1\text{M}, 2\text{M}, 4\text{M}, 10\text{M}\}$ at fixed $u = 10\text{K}$.

### The Redundant Seeding Bottleneck
Even with 4M keys, seeding each benchmark from scratch:
1. Sequentially computes millions of SHA-256 digests (`Sha256(i.to_be_bytes())`).
2. Builds the Merkle Mountain Belt, appends hundreds of megabytes to journal logs, and flushes to physical flash via `fsync`.
3. Consumes **40 to 60 seconds per invocation** for 4M keys, and **2 to 3 minutes** for 10M keys.
4. Across the 4 sweeps with 3 rotated process repeats (~60 benchmark executions), **over 1.5 to 2 hours are burned in pure repetitive seeding**.

### The Opportunity: Speculative Immutability & Instant CoW Scratch Cloning
- In `merkleize.rs`, `batch.merkleize(&db, None)` is purely speculative and in-memory. The database files on disk are completely unmutated.
- For mutating workloads like `apply_batch.rs` (PROP-04), rollback methods like `db.rewind()` introduce heap allocator fragmentation and cache pollution.
- By using **APFS / reflink Copy-on-Write (CoW) Cloning**, creating an isolated scratch copy of a pre-seeded database takes **~2 to 5 milliseconds** and consumes **0 extra disk space**.

---

## 2. Architectural Design Specification

```
┌────────────────────────────────────────────────────────────────────────┐
│                        Part A: Key Set Pipeline                        │
├────────────────────────────────────────────────────────────────────────┤
│  Master Pre-Computed Key File (`keys_10M.bin`, 320 MB)                 │
│  [ Key 0 (32B) | Key 1 (32B) | ... | Key 9,999,999 (32B) ]            │
│       │                                                                │
│       ├──► Fixed Anchor Workloads: Slice first 4M keys (128 MB)        │
│       └──► DB Scale Sweep: Slice 1M (32MB), 2M (64MB), 4M, 10M (320MB) │
└──────────────────────────────────┬─────────────────────────────────────┘
                                   │
                                   ▼
┌────────────────────────────────────────────────────────────────────────┐
│           Part B: Copy-on-Write (CoW) Snapshot Reuse Pipeline          │
├────────────────────────────────────────────────────────────────────────┤
│  Immutable Master Seeds (`storage-seeds/`):                            │
│  ├── Workhorse Baseline (Fixed 4M keys, ~1.0 GB total):                │
│  │   ├── `k4000000_any_unordered_fixed_mmb/` (~400 MB)                │
│  │   └── `k4000000_current_ordered_fixed_mmb_chunk32/` (~640 MB)      │
│  └── Scaling Study Seeds (1M, 2M, 10M):                                │
│      └── `k<scale>_<variant>/` (seeded once on demand by db-scale)     │
│                                                                        │
│  Benchmark Execution Lifecycle:                                        │
│  1. Check Master Seed: If present, skip seeding. If absent, seed once. │
│  2. Instant CoW Clone: `clonefile()` master seed to scratch dir (2 ms) │
│  3. Execute Benchmark: Run `merkleize()` or `apply_batch()` on scratch │
│  4. Clean Teardown: Discard scratch clone (0 ms). Master seed pristine│
└────────────────────────────────────────────────────────────────────────┘
```

---

### Part A: Contiguous Pre-Generated Key Set (`keys_10M.bin`)

#### 1. Binary Format & Slicing Invariance
- Contiguous sequence of 32-byte digests:
  $$\text{File Size} = 10{,}000{,}000 \times 32 \text{ bytes} = 320{,}000{,}000 \text{ bytes (320 MB)}$$
- Key at index $i$ corresponds deterministically to $\text{Sha256}(i\text{.to\_be\_bytes()})$.
- **Prefix Slicing across Sizing Tiers:**
  - $K = 1\text{M}$ (db-scale): First $32\text{ MB}$ ($[0 .. 32{,}000{,}000)$).
  - $K = 2\text{M}$ (db-scale): First $64\text{ MB}$ ($[0 .. 64{,}000{,}000)$).
  - **$K = 4\text{M}$ (Workload Anchor):** First $128\text{ MB}$ ($[0 .. 128{,}000{,}000)$).
  - $K = 10\text{M}$ (db-scale): Complete $320\text{ MB}$ file.
- Guarantees 100% keyspace subset determinism: smaller databases are strictly prefixes of larger ones.

#### 2. Fast Loading & Self-Bootstrapping Generator
- In `common.rs`, implement `KeySet`:
  - Memory-maps or sequentially reads the required prefix in **$< 20\text{ ms}$** for 4M keys ($< 50\text{ ms}$ for 10M keys).
  - If `keys_10M.bin` is missing, an integrated parallel Rayon generator builds all 10M keys in **$< 0.5\text{ seconds}$**, verifies integrity, and atomically writes the file.

#### 3. Integrity Verification
- **Size Assertion:** Exactly $320{,}000{,}000$ bytes.
- **Boundary Verification:** On load, assert:
  $$\text{keys}[0] \equiv \text{Sha256}(0\text{.to\_be\_bytes()}), \quad \text{keys}[K-1] \equiv \text{Sha256}((K-1)\text{.to\_be\_bytes()})$$
- **Cryptographic Checksum:** SHA-256 digest validation of the entire key set.

---

### Part B: Instant Copy-on-Write (CoW) Directory Cloning

#### 1. Master Seed Directory Layout
```
measurements-rain/storage-seeds/
├── k4000000_any_unordered_btree_fixed_mmb/     # Workhorse Anchor (write-size, write-type, skew)
│   ├── log-journal-bench-merkleize/
│   ├── metadata-bench-merkleize/
│   └── snapshot_manifest.json
├── k4000000_current_ordered_fixed_mmb_chunk32/ # Workhorse Anchor
│   ├── log-journal-bench-merkleize/
│   ├── grafted-metadata-bench-merkleize/
│   ├── metadata-bench-merkleize/
│   └── snapshot_manifest.json
├── k1000000_any_unordered_btree_fixed_mmb/     # Scaling study
├── k1000000_current_ordered_fixed_mmb_chunk32/
├── k2000000_any_unordered_btree_fixed_mmb/     # Scaling study
├── k2000000_current_ordered_fixed_mmb_chunk32/
├── k10000000_any_unordered_btree_fixed_mmb/    # Scaling study
└── k10000000_current_ordered_fixed_mmb_chunk32/
```

#### 2. Why Instant CoW Cloning is the Scientific Gold Standard
- **Zero Latency Overhead:** APFS `clonefile(2)` (macOS) and `reflink` (Linux) duplicate directory metadata in **~2 to 5 milliseconds**.
- **Pristine Heap & Memory:** Eliminates memory fragmentation and heap layout drift inherent to undo/rollback functions like `db.rewind()`.
- **Pure CPU & Cache Profiling:** PMU counters and Callgrind instruction counts are strictly deterministic; no cache pollution from reverse log traversals.
- **Master Seed Protection:** Benchmarks (including mutating `apply_batch`, `commit`, and `sync`) execute exclusively on ephemeral scratch clones. The master seed is mounted read-only and remains immutable.

#### 3. Scratch Lifecycle & Runtime Wiring
1. Scratch directory is created at `target/measurements/scratch_<run_id>/`.
2. Master seed for `k<keys>_<variant>` is cloned into the scratch path in $< 5\text{ ms}$.
3. `tokio::Runner` is initialized with `Config::new().with_storage_directory(&scratch_dir)`.
4. Criterion executes the benchmark iterations.
5. Scratch directory is wiped on teardown.

---

### 3. Estimated Time Savings & Storage Footprint

#### Time Comparison:
- **Workload Anchor Runs ($K = 4\text{M}$):**
  - Without pre-seeding: Seeding takes **~45 seconds per run**.
  - With CoW cloning: Setup takes **~2 to 5 milliseconds** ($< 2$ seconds to open and run).
  - An isolated 12-run sweep (e.g. write-type across 3 repeats) drops from **~10 minutes to ~30 seconds** (**$> 95\%$ speedup**).
- **Scale Sweep Runs ($K = 10\text{M}$):**
  - Setup drops from **~120 seconds to ~3 milliseconds**.

#### Storage Footprint:
- **Raw Key Set (`keys_10M.bin`):** **320 MB** ($0.32\text{ GB}$).
- **Core 4M Master Seeds (Fixed Anchor):**
  - `k4000000_any_unordered`: ~400 MB
  - `k4000000_current_ordered`: ~640 MB
  - **Total for Core Sweeps:** **$\approx 1.0\text{ GB}$**
- **Scaling Master Seeds (1M, 2M, 10M):**
  - Optional; created on-demand only when running `db-scale`. Total for all scales: $\approx 3.5\text{ GB}$.
- **Scratch Clones:** **0 MB persistent storage** (APFS shares blocks; scratch directories are purged immediately).

---

## 4. Configuration & Fallback Controls

| Variable | Default Value | Description |
| :--- | :--- | :--- |
| `QMDB_SNAPSHOT_DIR` | `measurements-rain/storage-seeds` | Directory storing master pre-seeded databases. |
| `QMDB_KEYS_FILE` | `measurements-rain/data/keys_10M.bin` | Path to master binary key set file. |
| `QMDB_MERKLEIZE_KEYS` | `4000000` (4M) | Controls keyspace at runtime; overrides compile-time defaults. |
| `QMDB_FORCE_RESEED` | `0` (disabled) | Forces rebuild of master seeds. |
| `QMDB_NO_CLONE` | `0` (disabled) | Disables CoW cloning and reverts to ephemeral temp dirs. |

---

## 5. Implementation Roadmap

1. **Phase 1: Master Key Set (`storage/src/qmdb/benches/common.rs`):**
   - Implement `KeySet` with Rayon auto-generation and prefix slicing.
   - Wire `seed_db` to consume pre-loaded keys.
   - Add `.gitignore` for `data/` and `storage-seeds/`.
2. **Phase 2: CoW Clone Utility:**
   - Implement cross-platform directory cloning (`clone_directory` using APFS `clonefile` / Linux `reflink`).
3. **Phase 3: Integration in `merkleize.rs`:**
   - Detect or bootstrap master seeds for `k<keys>_<variant>`.
   - Point `tokio::Runner` to cloned scratch directory.
   - Maintain pristine master seeds on teardown.
4. **Phase 4: Validation Test Suite (`storage/tests/qmdb_snapshot_reuse.rs`):**
   - Assert root invariance across cold seed, reopened DB, and cloned scratch DB.
   - Verify clone isolation under applied writes.
5. **Phase 5: Runner Script Coordination:**
   - Coordinate with `writesize`, `write-type`, `skew`, and `db-scale` runners.
