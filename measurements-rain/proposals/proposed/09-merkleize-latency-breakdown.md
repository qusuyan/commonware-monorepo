# Proposal 09: In-Depth `merkleize()` Latency Breakdown & Stage Attribution

- **Proposal ID:** PROP-09
- **Status:** Proposed
- **Author:** Rain Sun
- **Created:** 2026-11-11
- **Target Files:**
  - [`storage/src/qmdb/any/batch.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/any/batch.rs)
  - [`storage/src/qmdb/current/batch.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/current/batch.rs)
  - [`storage/src/qmdb/benches/breakdown.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/breakdown.rs) *(new)*
  - [`storage/Cargo.toml`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/Cargo.toml)
  - [`measurements-rain/experiments/breakdown/run.sh`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/experiments/breakdown/run.sh) *(new)*
  - [`measurements-rain/experiments/breakdown/plot.py`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/experiments/breakdown/plot.py) *(new)*

---

## 1. Problem Statement & Motivation

In the controlled write-type experiment ([`write-type/20261104T192747Z-u2000-r1`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/results/write-type/20261104T192747Z-u2000-r1/summary.csv); $K = 4\text{M}$ seeded keys, $u = 2,000$ write calls per batch, fixed 32-byte keys/values, MMB), we observed an unexpected **~8.6x latency gap** between **Insert (new write)** and **Overwrite/Delete** in `any::unordered_btree`, while `current::ordered` showed almost no speedup on inserts:

| Configuration | Overwrite (ms) | Delete (ms) | Insert (ms) | Insert vs. Overwrite |
| :--- | :---: | :---: | :---: | :---: |
| `any::unordered_btree::fixed::mmb` | $9.90$ | $8.03$ | **$1.15$** | **$-88.4\%$ ($\sim 8.6\times$ faster)** |
| `current::ordered::fixed::mmb chunk=32` | $13.68$ | $12.92$ | $11.96$ | $-12.6\%$ ($\sim 1.1\times$ faster) |

### Why Existing Tooling Cannot Explain This Gap Empirically

1. **`merkleize.rs` (Criterion):** Times `new_batch() -> write() -> merkleize() -> root()` as a single opaque wall-clock block.
2. **`qmdb_cpu` (`run-cpu-telemetry.sh`):** Captures process-wide `getrusage` counters (`user_cpu_ns`, `system_cpu_ns`, context switches, page faults) across the entire batch, without internal phase attribution.
3. **`components.rs` (`qmdb_components`):** Benchmarks isolated, synthetic in-memory CPU primitives (1,024-item leaf hashing, `Mem` tree construction, bitmap toggles, grafted hashing). It deliberately omits B-Tree snapshot probes, journal disk/page-cache reads, compaction floor raising, and ordered predecessor resolution.

We need an **empirical, end-to-end stage breakdown** inside actual `merkleize()` executions against the 4M-key database snapshots to measure exactly how many microseconds and what percentage of total latency each internal phase consumes.

---

## 2. Hypothesized Root Causes to Quantify

Code inspection of [`any/batch.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/any/batch.rs) and [`current/batch.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/current/batch.rs) points to three structural asymmetries that this breakdown will measure directly:

1. **Zero Historical Journal Reads (`gather_existing_locations` & `read_ops`):**
   - On `Insert`, none of the $2,000$ fresh keys exist in `db.snapshot`. `gather_existing_locations` returns an empty `Vec`, so `read_ops(&locations, &[], &db.log)` returns immediately with **0 journal reads** and **0 deserializations**.
   - On `Overwrite` / `Delete`, all $2,000$ keys exist at scattered log positions across $4\text{M}$ entries, forcing unsorted batched journal reads across page-cache/disk sections.
2. **Inactivity Floor-Raise Bypass (`user_steps = 0` in `creates`):**
   - In `merkleize_with_floor_scan` (`Unordered` and `Ordered`), `emit()` increments `user_steps += 1` for existing-key updates/deletes, whereas the `creates` loop does **not** increment `user_steps`.
   - In `finish()`, `total_steps = user_steps + 1`. Thus, pure inserts execute `total_steps = 1` (advancing the floor by at most 1 active op for the previous commit), whereas pure overwrites execute `total_steps = 2001`—forcing 2,001 candidate bitmap scans, log reads (`read_ops_sharded`), classifications, and op moves to the tip (roughly doubling the final Merkleized batch from ~2,001 to ~4,001 leaves).
3. **`current::ordered` Exclusion-Proof & Grafting Tax:**
   - Every insert in `current::ordered` executes ordered predecessor resolution (`find_prev_key` / `find_next_key`), reads historical predecessor records, and rewrites $K_{\text{prev}}$'s `next_key`.
   - Rewriting $K_{\text{prev}}$ generates ~2x operations (~4,000 ops), increments `user_steps` for predecessor updates, flips historical bitmap bits $1 \to 0$, dirties historical 32-byte chunks, and forces `compute_current_layer` to re-hash and re-graft the upper tree.

---

## 3. Proposed Technical Design

### A. Feature-Gated Stage Instrumentation (`MerkleizeBreakdown`)

To guarantee zero overhead in production and untouched Criterion macro-benchmarks, instrumentation will be gated behind a Cargo feature flag (e.g., `bench-breakdown` in `storage/Cargo.toml`) or thread-local scoped collector.

#### Measured Stages & Exact Code Boundaries

We decompose `merkleize()` into 9 mutually exclusive timing buckets (plus `unaccounted_ns` as an instrumentation sanity check) and 5 structural counters:

```rust
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct MerkleizeBreakdown {
    // --- Wall-clock stage durations (nanoseconds) ---
    pub index_probe_ns: u64,          // gather_existing_locations + ordered next/prev snapshot probes
    pub journal_read_ns: u64,         // read_ops for existing keys (and ordered prev/next candidates)
    pub op_gen_ns: u64,               // emit / creates / ordered predecessor rewrites / diff prep
    pub floor_scan_ns: u64,           // fill_candidates bitmap scan inside finish()
    pub floor_read_ns: u64,           // read_ops_sharded for floor-raise candidates
    pub floor_classify_move_ns: u64,  // parallel candidate classification + move-to-tip loop
    pub ops_tree_merkleize_ns: u64,   // db.log.merkleize() + diff_sort / diff_merge awaits
    pub current_overlay_ns: u64,      // current::build_chunk_overlay (0 for any)
    pub current_graft_ns: u64,        // read_graft_inputs + graft_chunk_digests + upper tree + compute_db_root (0 for any)
    pub total_ns: u64,                // end-to-end merkleize() wall time

    // --- Structural operation counters (per batch) ---
    pub existing_locs_read: u64,      // locations.len() passed to read_ops
    pub floor_steps_requested: u64,   // total_steps in finish()
    pub floor_candidates_read: u64,   // read_candidates.len() across floor-raise rounds
    pub floor_ops_moved: u64,         // moved count in finish()
    pub final_ops_len: u64,           // total ops passed to db.log.merkleize()
    pub dirty_chunks_grafted: u64,    // chunk_indices_to_update.len() (0 for any)
}
```

#### Instrumentation Mapping in Source

1. **In `storage/src/qmdb/any/batch.rs`:**
   - **Unordered (`merkleize_with_floor_scan`, ~lines 2015–2182)** and **Ordered (`merkleize_with_floor_scan`, ~lines 2231–2606)**:
     - Wrap `gather_existing_locations` (and ordered index successor/predecessor lookups) $\rightarrow$ `index_probe_ns`.
     - Wrap `read_ops` $\rightarrow$ `journal_read_ns`, recording `existing_locs_read`.
     - Wrap `emit` / `creates` / predecessor rewrite loops $\rightarrow$ `op_gen_ns`.
   - **Shared `finish()` (~lines 1014–1330):**
     - Record `floor_steps_requested = total_steps`.
     - Inside the `while moved < total_steps` floor-raise loop:
       - Wrap `fill_candidates(...)` $\rightarrow$ `floor_scan_ns`.
       - Wrap `read_ops_sharded(...)` $\rightarrow$ `floor_read_ns`, accumulating `floor_candidates_read`.
       - Wrap candidate classification + apply-to-tip loop $\rightarrow$ `floor_classify_move_ns`, recording `floor_ops_moved`.
     - Wrap `db.log.merkleize(...)` and final `diff_sort` / `diff_merge` awaits $\rightarrow$ `ops_tree_merkleize_ns`, recording `final_ops_len`.
     - Note: `diff_sort` and `diff_merge` are spawned asynchronously onto the Rayon pool (`db.strategy().spawn(...)`) to overlap with I/O and hashing. Measuring the join/await point separately or inside `ops_tree_merkleize_ns` (plus noting overlapped CPU time if needed) ensures wall-clock buckets remain strictly additive and sum to `total_ns`.

2. **In `storage/src/qmdb/current/batch.rs`:**
   - **`compute_current_layer` (~lines 750–906):**
     - `inner.merkleize_with_floor_scan(...)` populates the `any`-layer timings and counters above.
     - Wrap `build_chunk_overlay(...)` $\rightarrow$ `current_overlay_ns`.
     - Wrap `read_graft_inputs(...)`, `graft_chunk_digests`, `grafted_batch.merkleize(...)`, and `compute_db_root(...)` $\rightarrow$ `current_graft_ns`, recording `dirty_chunks_grafted = chunk_indices_to_update.len() as u64`.

---

### B. Breakdown Benchmark Executable (`qmdb_breakdown`)

Create a standalone binary/benchmark target [`storage/src/qmdb/benches/breakdown.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/breakdown.rs) (following the pattern of [`cpu.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/cpu.rs) rather than Criterion, so every iteration emits exact per-stage distributions without Criterion loop black-boxing):

- **Arguments:**
  `--variant <any_unordered_btree|current_ordered|current_unordered_btree> --keys 4000000 --updates 2000 --write-type <overwrite|insert|delete> --samples 20 [--warmup 3] [--skew 0] --output <DIR>`
- **Snapshot Reuse:**
  Uses [`snapshots::prepare`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/snapshots.rs#L422) (APFS `clonefile` CoW) against the pre-seeded $4\text{M}$-key masters in `measurements-rain/storage-seeds/` (`s=true`, MMB, fixed 32-byte keys/values, warm retained page cache).
- **Timing Scope:**
  Identical to `merkleize.rs`: input generation precedes timing; `batch = db.new_batch()` + `stage_prepared_writes` + `batch.merkleize(db, None)` + `merkleized.root()` are executed inside the measured window.
- **Outputs (`<DIR>`):**
  - `samples.ndjson`: One JSON record per sample containing all 9 stage durations (ns), `unaccounted_ns`, and all 6 structural counters.
  - `summary.json` / `summary.csv`: Per-stage mean, median, p95, min, max, standard deviation, and `% of total_ns`.

---

### C. Controlled `user_steps` Verification Experiment

To isolate how much of `any::unordered_btree`'s $8.6\times$ insert speedup comes from **zero journal reads** vs. the **floor-raise bypass (`user_steps = 0`)**:

1. Run the standard breakdown (`creates` does *not* increment `user_steps`) to establish the baseline split across the stages.
2. Add an experimental diagnostic toggle (e.g., `QMDB_BREAKDOWN_CHARGE_CREATE_STEPS=1` gated behind `bench-breakdown`) that sets `user_steps += 1` in the `creates` loop of `merkleize_with_floor_scan`.
3. Re-run `any_unordered_btree` with `WriteType::Insert` ($K=4\text{M}, u=2,000$). Comparing `charge=0` vs `charge=1` cleanly partitions the insert savings into:
   - **Savings from skipping initial key reads (`read_ops`)**: difference between `Overwrite` and `Insert (charge=1)`.
   - **Savings from skipping compaction floor-raise work (`floor_scan + floor_read + floor_move + 2x tree`)**: difference between `Insert (charge=1)` and `Insert (charge=0)`.

*(Note: Consult upstream/mentor intent on whether omitting `user_steps += 1` on `creates` is deliberate—because inserts append net-new live keys to the log rather than creating inactive/dead entries that increase the stale-to-active ratio—before considering any functional change to QMDB itself.)*

---

### D. Experiment Runner & Stacked Bar Visualization

Create [`measurements-rain/experiments/breakdown/`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/experiments/breakdown):

1. **`run.sh`:**
   - Runs `qmdb_breakdown` (built with `--features test-traits,bench-breakdown`) across the matrix:
     - Variants: `any_unordered_btree`, `current_ordered` (and `current_unordered_btree` once seed/control is wired).
     - Write types: `overwrite`, `insert`, `delete`.
   - Archives manifests, `command.txt`, raw `samples.ndjson`, and aggregated `summary.csv` under `measurements-rain/results/breakdown/<run-id>/`.
2. **`plot.py`:**
   - Renders a **stacked bar chart** (`breakdown-merkleize.png` / `.pdf`) comparing configurations and write types side by side, grouping the 9 sub-stages into clear visual bands:
     - **Index Probe** (`index_probe_ns`)
     - **Historical Journal Reads** (`journal_read_ns`)
     - **Op Gen & Predecessor Maintenance** (`op_gen_ns`)
     - **Compaction Floor Raise** (`floor_scan_ns + floor_read_ns + floor_classify_move_ns`)
     - **Ops Merkle Tree Build** (`ops_tree_merkleize_ns`)
     - **Current Layer Bitmap & Grafting** (`current_overlay_ns + current_graft_ns`)
   - Overlays structural counter annotations (e.g., `ops_read`, `moved`, `leaves`, `dirty_chunks`).

---

## 4. Validation & Correctness Guardrails

1. **Additive Closure Check:** Assert in unit/integration tests (`storage/tests/qmdb_breakdown.rs`) that `sum(stage_ns) <= total_ns` and `unaccounted_ns / total_ns < 5%` across both `any` and `current`.
2. **Counter Invariants:** Verify deterministically on small seeds ($K = 1,000, u = 100$):
   - `any` pure `Insert`: `existing_locs_read == 0`, `floor_steps_requested == 1`, `floor_ops_moved <= 1`, `dirty_chunks_grafted == 0`.
   - `any` pure `Overwrite`: `existing_locs_read == unique_keys_written`, `floor_steps_requested == unique_keys_written + 1`, `dirty_chunks_grafted == 0`.
   - `current::ordered` pure `Insert`: `dirty_chunks_grafted > 0` (from predecessor updates invalidating historical locations).
3. **Zero Upstream/Baseline Regression:** When `bench-breakdown` is disabled, `cargo test` and `qmdb` Criterion benchmarks compile with zero extra branches or timer calls.

---

## 5. Execution Checklist

- [ ] **Step 1:** Add `bench-breakdown` feature and `MerkleizeBreakdown` collector struct (`storage/Cargo.toml`, `storage/src/qmdb/mod.rs`).
- [ ] **Step 2:** Instrument `any/batch.rs` (`Unordered` + `Ordered` + `finish`) and `current/batch.rs` (`compute_current_layer`).
- [ ] **Step 3:** Add unit/integration verification test `storage/tests/qmdb_breakdown.rs`.
- [ ] **Step 4:** Implement `storage/src/qmdb/benches/breakdown.rs` reusing `snapshots::prepare`.
- [ ] **Step 5:** Add `measurements-rain/experiments/breakdown/{README.md,run.sh,plot.py}` and run the $K=4\text{M}, u=2,000$ sweep.
- [ ] **Step 6:** Run the `user_steps` attribution check (`charge=0` vs `charge=1`) and document empirical findings in `measurements-rain/todos.md`.
