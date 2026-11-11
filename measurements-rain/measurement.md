# QMDB Measurement & Benchmark Plan

Saved-keyset and database setup uses a validated shared configuration. See
[snapshot reuse](snapshot-reuse.md) for preparation, required/auto/off behavior,
supported variants, and interpretation of setup provenance.

---

## 1. Main Goal & Core Research Question

The primary objective of this study is to measure the performance cost of QMDB's authentication machinery—specifically current-state inclusion proofs (bitmap grafting) and non-membership exclusion proofs (predecessor pointer chains in an ordered index)—under realistic state-update workloads.

### Primary Comparison: `current::ordered` vs. `any::unordered`
Following discussion with our mentor, our primary comparison across all experiments is:

$$\mathbf{current::ordered} \quad \text{vs.} \quad \mathbf{any::unordered}$$

> [!IMPORTANT]
> **Core Mentor Rationale: Maintaining Integrity while Eliminating State Authentication**
> 1. **Database Integrity is Mandatory:** Any production verifiable database or blockchain node requires tamper-evident, append-only log history and fork detection. This is guaranteed by the append-only Merkle tree (MMR/MMB) over the operations log. We are **not** proposing to remove log integrity.
> 2. **Eliminating State Authentication:** What we want to isolate and quantify is the cost of **state authentication**—the machinery required to serve light-client cryptographic proofs of current key-value state (inclusion) and non-existence (exclusion).
> 3. **Why `any::unordered` is the True Baseline:** `any::unordered` eliminates all state authentication machinery (no activity bitmap grafting, no predecessor pointer chains, zero client-facing state proofs), while preserving full cryptographic log integrity. Raw unauthenticated `store` breaks tamper-evidence and cannot run in production; thus, `any::unordered` is our mandatory production-viable baseline.

The central research question is:

> How much CPU time, latency, storage I/O, and retained state does QMDB add to
> authenticated key-value updates, and when does bitmap grafting and exclusion-proof maintenance become a
> bottleneck as the database and workload scale?

The study separates the non-negotiable cost of the operation-log Merkle tree from the
cost of tracking and authenticating current key-value state. It establishes controlled
microbenchmarks and component breakdowns before validating results against a realistic
workload, such as the QMDB-backed Avax-pEVM branch.

### Key Measurement Questions
1. **End-to-End Cryptographic Tax:** What is the overhead of `current` state authentication relative to the mandatory log-integrity baseline `any`?
2. **Component Breakdown:** How much of that overhead is attributable to:
   - Operation encoding and leaf hashing;
   - MMR / MMB operation-tree Merkleization;
   - In-memory activity bitmap updates;
   - Bitmap-chunk hashing;
   - Grafted-leaf and upper grafted-tree hashing;
   - Applying batches to disk, committing, and syncing durably?
3. **Merkle Trees:** How do Merkle Mountain Ranges (MMR) and Merkle Mountain Belts (MMB) alter CPU instruction counts, latency distributions, memory footprint, and cache behavior?
4. **Workload Sensitivity:** How does authentication overhead shift across workload dimensions:
   - Mutation type (pure insert vs. overwrite vs. deletion);
   - Batch size (micro-batches vs. multi-thousand-op batches);
   - Access skewness (uniform random vs. Zipfian power laws);
   - State scale ($1\text{M}$ to $10\text{M}+$ keys) and volume of inactive history;
   - Bitmap chunk size (32 bytes vs. 256 bytes)?
5. **Concurrency & Serialization:** Which phases can execute in parallel (speculative batch construction, hashing, Merkle tree math), and where are operations strictly serialized by QMDB's linear commit model?

---

## 2. Cryptographic Taxonomy & Evaluation Baselines

QMDB records every mutation in an append-only operation log and commits it using append-friendly Merkle families:
- **MMR (Merkle Mountain Range):** Forest of perfect Merkle trees where new leaves trigger immediate cascade merges.
- **MMB (Merkle Mountain Belt):** Postpones merges to smooth append latency, creating at most one internal node per append.

### Integrity vs. Authentication: The Research Philosophy
A key insight from discussions with our mentor is the fundamental separation between **integrity** and **authentication**:

1. **Log Integrity (Non-Negotiable Invariant):**
   - Guarantees that the sequential operation log is **tamper-evident, immutable, and consensus-verifiable**.
   - Implemented via append-only Merkle Mountain Ranges/Belts (MMR/MMB) over sequential operations (`any`).
   - If an attacker tampers with a historical disk record, the Merkle root changes and consensus rejects the state.
   - **Conclusion:** We never remove the Merkle tree. Raw unauthenticated `store` is evaluated strictly as an academic reference, not a deployable alternative.

2. **State Authentication (The Measured Overhead):**
   - Provides cryptographic proofs to untrusted light clients answering:
     - *"What is the current value of key $K$?"* (Inclusion proof $\to$ requires **Activity Bitmap Grafting** in `current`).
     - *"Does key $K$ exist?"* (Exclusion proof $\to$ requires **Ordered B-Tree Index + Predecessor Chains** in `ordered`).
   - Maintaining these client-facing proofs introduces severe performance taxes:
     - **Bitmap Grafting Tax (~2.1x slowdown on overwrites):** Historical bitmap chunks are mutated from $1 \to 0$, forcing re-hashing of dirty chunks into grafted leaves.
     - **Exclusion Proof Tax (~31x slowdown on inserts):** Inserting a key requires a predecessor search in a sorted B-Tree, logging two physical records instead of one, and maintaining on-disk predecessor chains.

By comparing **`current::ordered`** against **`any::unordered`**, we measure the true end-to-end price of full state authentication over a database that maintains complete log integrity.

```text
┌────────────────────────────────────────────────────────────────────────┐
│ current_ordered        Log integrity + current inclusion + exclusion   │  ▲ Exclusion proof tax
├────────────────────────────────────────────────────────────────────────┤  │ (~31x insert slowdown)
│ current_unordered      Log integrity + current inclusion (via grafting)│  ▼
├────────────────────────────────────────────────────────────────────────┤  ▲ Bitmap grafting tax
│ any_unordered          MANDATORY BASELINE: Log integrity & history     │  │ (~2.1x overwrite slowdown)
├────────────────────────────────────────────────────────────────────────┤  ▼
│ store                  [Reference Only] Raw unauthenticated log/store  │  ◄── Breaks tamper-evident integrity
└────────────────────────────────────────────────────────────────────────┘
```

### Engineering Reality vs. Scientific Control (`any_unordered` vs. `any_unordered_btree`)
In analyzing the gap between `current::ordered` and `any::unordered`, two complementary perspectives are evaluated:
- **Engineering Reality (`current_ordered` vs. standard `any_unordered`):** In a production system that opts out of state authentication, engineers naturally adopt a high-throughput hash map index. This comparison reflects the **practical real-world speedup** achieved by shedding state authentication machinery.
- **Scientific Control (`current_ordered` vs. `any_unordered_btree`):** `current::ordered` uses an ordered B-Tree to find predecessor keys. To separate the **data structure overhead** (B-Tree vs. Hash Table) from the **cryptographic protocol overhead** (predecessor double-writes and bitmap grafting), we created the `any_unordered_btree` control variant. Comparing `current_ordered` against `any_unordered_btree` holds the index type identical on both sides, strictly isolating the cryptographic tax.

### The Mechanism of Grafting
`any` proves that a key/value appeared in history; its activity bitmap is strictly local. `current` wraps `any` to prove whether a historical operation is still active. 

To achieve single-root state proofs without dual-tree proof overhead, complete bitmap chunks are cryptographically combined with matching operation-log subtrees into **grafted leaves**:

```text
grafted leaf = hash(bitmap_chunk || operation_subtree_root)
```

The upper Merkle tree hashes these grafted leaves into the canonical State Root.

---

## 3. Active Experimental Matrix & Sizing Strategy

To control execution time and disk footprint while preserving a substantial keyspace, the active experimental matrix uses a **$4\text{M}$-key baseline anchor** ($K = 4,000,000$) on dedicated runners under [`measurements-rain/experiments/`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/experiments/).

### Controlled Pairing: `any_unordered_btree` vs. `current_ordered`
All active runners compare `any_unordered_btree` with `current_ordered`. Both variants use the identical nonpartitioned ordered B-Tree index type, controlling for data structure differences (B-Tree vs. Hash Table). The remaining measured delta strictly isolates Current's record layout, predecessor tracking, authenticated bitmap activity, and Merkle grafting.

### Controlled Dimensions
All active experiments share standard defaults: fixed 32-byte keys and values, MMB, direct batches, synced seeds (`s=true`), retained Commonware page cache (`cc=false`), and 32-byte bitmap chunks for Current.

- **Database Scaling Sweep ([`db-scale`](experiments/db-scale/README.md)):**
  - Keyspaces: $K \in \{1\text{M}, 2\text{M}, 4\text{M}, 10\text{M}\}$ seeded keys.
  - Fixed batch size: $u = 2,000$ uniform overwrites.
- **Write-Size Sweep ([`writesize`](experiments/writesize/README.md)):**
  - Fixed keyspace: $K = 4\text{M}$ seeded keys.
  - Updates per batch: $u \in \{200, 500, 1000, 1500, 2000, 5000, 7000, 8000, 10000\}$ write calls (uniform overwrites).
- **Write-Type Sweep ([`write-type`](experiments/write-type/README.md)):**
  - Fixed keyspace: $K = 4\text{M}$ seeded keys; fixed batch size: $u = 2,000$.
  - Workload types: pure Overwrites, pure Inserts, and pure Deletes.
- **Access Skew Sweep ([`skew`](experiments/skew/README.md)):**
  - Fixed keyspace: $K = 4\text{M}$ seeded keys; fixed batch size: $u = 2,000$.
  - Zipf exponents: $s \in \{0.0, 0.5, 1.0, 1.5, 2.0\}$ (where $s=0.0$ is uniform random).
- **Process Repeats:** 3 independent process runs per configuration for full reported studies (`--repeats 3`).
- **Smoke Mode:** Fast validation supported via `--smoke` (10K keys, shortened Criterion warmups).

### Apply-Only Extension
The study supports measuring buffered disk apply performance in isolation via `QMDB_STUDY_OPERATION=apply` across the four experiment runners.
- Only `apply_batch` is timed (updating the index and appending log buffers).
- Write staging and Merkleization occur before timing; commit and sync are untimed.
- Each iteration executes against an isolated scratch seed. See [`experiments/apply/README.md`](experiments/apply/README.md) for execution details.

---

## 4. Configuration Comparisons & Timing Boundaries

### Alternating Execution & Statistics
Every repeat alternates execution order between A/B (`any_unordered_btree` then `current_ordered`) and B/A (`current_ordered` then `any_unordered_btree`) across repeats to mitigate thermal throttling and operating system caching bias.

Comparison summaries use `schema_version: 2` JSON:
- `configurations`: Records individual configuration parameters and Criterion means.
- `comparisons`: Stores `current_ordered_minus_any_unordered_btree`, recording:
  - `delta_mean_us`: Absolute difference in microseconds.
  - `overhead_percent`: Percentage overhead relative to `any_unordered_btree`.
- In write-type runs, intra-configuration deltas (`insert_minus_overwrite`, `delete_minus_overwrite`) are also emitted.
- All pairwise deltas are descriptive differences, reported alongside Criterion confidence intervals and sample counts.

### Timing Boundary Contract
- **Excluded from Timing:** Key sampling, random value generation, and input vector synthesis run strictly outside the timed loop (in `before_iter()`).
- **Included in Timing:** Staging updates into speculative batches, executing `batch.merkleize(db, None)`, and retrieving `merkleized.root()`.
- **Batch Discarded:** Speculative batches are discarded after root retrieval. These benchmarks measure the cost of maintaining authentication structures, not proof generation, client verification, or disk sync latency.

---

## 5. Execution & Reproducibility Guide

All benchmark and runner commands are executed from the repository root:

```bash
cd /Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo
```

### 1. Focused Matrix Runner (Fast Verification)
To execute the $u=2\text{K}$ anchor across all write types, skew endpoints ($s \in \{0, 2\}$), and scale endpoints ($K \in \{1\text{M}, 10\text{M}\}$) with 3 process repeats (48 Criterion invocations):

```bash
measurements-rain/scripts/run-focused-matrix.sh 3
```

### 2. Dedicated Dimension Sweeps
Each dimension can be executed individually:

```bash
# Write-size sweep across batch sizes:
for updates in 200 500 1000 1500 2000 5000 7000 8000 10000; do
  measurements-rain/experiments/writesize/run.sh "$updates" 3
done

# Write-type sweep (overwrites, inserts, deletes at 4M keys, 2K updates):
measurements-rain/experiments/write-type/run.sh 2000 3

# Access skewness sweep:
measurements-rain/experiments/skew/run.sh --updates 2000 --repeats 3 0 0.5 1 1.5 2

# Database scaling sweep:
measurements-rain/experiments/db-scale/run.sh --updates 2000 --repeats 3 1000000 2000000 4000000 10000000
```

### 3. Local Environment & Disk Budgeting
- **Sequential Execution:** Run configurations sequentially rather than materializing full Cartesian products simultaneously.
- **Disk Budgeting:** The raw 32-byte key + 32-byte value payload for 4M pairs is ~256 MB. Factoring in journal logs, Merkle nodes, bitmap metadata, indexes, and alignment, budget 2–5 GB of free space per active variant. Check free disk space before generating 10M keys or multi-million-key seeds.
- **Environment Isolation:** Record CPU, RAM, OS version, and compiler toolchain in `manifest.json`. Do not aggregate runs across different machines.

### 4. Archival & Results Tracking
All results are preserved under [`measurements-rain/results/<experiment>/<timestamp>/`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/results/):
- `manifest.json`: Machine hostname, git commit SHA, OS version, CPU model, and parameters.
- `commands.txt`: Verbatim CLI commands executed.
- `*.log`: Full Criterion stdout/stderr output.
- `comparison.json`: Normalized schema v2 summary.
- Machine load, cache state, and thermal events must be recorded in `notes.md`.

---

## 6. Existing Measurement & Profiling Harnesses

The monorepo provides several benchmark targets under `storage/src/qmdb/benches/`:

- **`storage/src/qmdb/benches/bench.rs` (`qmdb`):**
  - Criterion entry point grouping `apply_batch`, `chained_growth`, `generate`, `init`, and `merkleize`.
  - Configurable via environment variables (`QMDB_MERKLEIZE_UPDATES`, `QMDB_MERKLEIZE_WRITE_TYPE`, `QMDB_MERKLEIZE_SKEW`, `QMDB_MERKLEIZE_KEYS`).
- **`storage/src/qmdb/benches/components.rs` (`qmdb_components`):**
  - Isolated microbenchmarks measuring raw hashing, bitmap-chunk digest computation, grafted-leaf hashing, and B-Tree predecessor search without storage I/O.
- **`storage/src/qmdb/benches/merkleize_gungraun.rs` (`qmdb_gungraun`):**
  - Callgrind / Gungraun CPU profiling harness recording instruction counts, L1/LLC cache misses, and memory reads/writes.
- **`storage/src/qmdb/benches/apply_study.rs`:**
  - Dedicated harness for measuring `apply_batch` latency across variants without commit overhead.
- **`storage/src/qmdb/benches/constantinople.rs`:**
  - Block-shaped harness (default 1M keys, 32K reads, 32K updates), separating staged-load and speculative Merkleization.
- **`storage/src/qmdb/benches/chained_growth.rs`:**
  - Evaluates long speculative batch chains and sparse updates.
- **`storage/src/qmdb/benches/scale.rs`:**
  - Multi-GB database builder evaluating cold/warm `get` and `get_many` performance at configurable concurrency.
- **`storage/src/qmdb/metrics.rs`:**
  - Prometheus metrics tracking apply, commit, sync, prune, and storage boundaries.

---

## 7. Metrics to Record

For every evaluated configuration, report:
- **Wall-Clock Latency:** Criterion median, mean, p95, and p99 per batch.
- **Throughput:** Operations per second and batches per second.
- **CPU & Cache:** Total instructions, cycles / estimated cycles, L1 data misses, and LLC cache misses (via Callgrind/Gungraun).
- **Storage & I/O:** Physical log bytes appended, blob read/write operations, page cache hit rate, and `commit`/`sync` latency.
- **State Footprint:** Operation-log length, inactivity floor position, retained active operations, and pruned bitmap chunks.
- **Workload Parameters:** Keyspace size ($K$), batch size ($u$), write mix (insert/overwrite/delete), access skew exponent ($s$), Merkle family (MMR/MMB), index type, and bitmap chunk size ($N$).

---

## 8. Phased Research Roadmap & Follow-Ups

```
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                              PHASED MEASUREMENT ROADMAP                                │
└────────────────────────────────────────────────────────────────────────────────────────┘
  Phase 1: Speculative CPU & Time Matrix (ACTIVE)
  ├── 4M-key anchor with dedicated runners (writesize, write-type, skew, db-scale)
  └── Controlled A/B comparison: any_unordered_btree vs. current_ordered
       │
       ▼
  Phase 2: CPU Attribution & Component Profiling
  ├── Callgrind / Gungraun instruction & cache profiling
  └── Isolated microbenchmarks: raw leaf hashing, bitmap chunking, regrafting
       │
       ▼
  Phase 3: Durable Write Lifecycle & I/O Amplification
  ├── Measure 3 tiers: Speculative Merkleize -> Buffered Apply -> fsync Commit
  └── Quantify Write Amplification Factor (WAF) for Current ordered predecessor writes
       │
       ▼
  Phase 4: Client Verification & Real-World Validation
  ├── Measure proof generation & verification times (any membership vs. current state)
  └── Macro-benchmark validation against Avax-pEVM transaction trace workloads
```

1. **Phase 1: Controlled Whole-System Speculative Baselines (Active):** Complete the 4-dimension matrix (`writesize`, `write-type`, `skew`, `db-scale`) comparing `any_unordered_btree` and `current_ordered`.
2. **Phase 2: Component Attribution & Instruction Profiling:** Profile isolated hashing costs (`qmdb_components`) and map Callgrind CPU counters to the end-to-end gap.
3. **Phase 3: Durable Write Lifecycle & Write Amplification:** Extend beyond speculative Merkleization to quantify disk I/O across the 3 durability tiers:
   - Tier 1: Speculative `merkleize()` (in RAM)
   - Tier 2: Buffered `apply_batch()` (log append buffer + memory index)
   - Tier 3: Durable `commit()` + `sync()` (physical `fsync` flash barrier)
   - Validate the 2:1 write amplification hypothesis for ordered inserts.
4. **Phase 4: Light-Client Proofs & Application-Level Validation:**
   - Measure proof generation and client-side proof verification latency (`any` historical proof vs. `current` grafted proof).
   - Replay real-world transaction execution traces on the QMDB Avax-pEVM integration branch.
