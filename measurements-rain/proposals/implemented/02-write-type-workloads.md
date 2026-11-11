# Proposal 02: Write-Type Workloads & The Dual Asymmetry

- **Proposal ID:** IMP-02
- **Status:** Implemented (Merged PR #2: `overwriten-newwrite`, commits `3a77fc41` & `497d2c6e`)
- **Author:** Rain Sun
- **Target Files:**
  - [`storage/src/qmdb/benches/common.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/common.rs)
  - [`storage/src/qmdb/benches/merkleize.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/merkleize.rs)
  - [`measurements-rain/experiments/write-type/`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/experiments/write-type)

---

## 1. Problem Statement & Motivation

QMDB's default benchmarks evaluated write performance using only random updates on existing keys. However, the architectural mechanisms of `current::unordered` and `current::ordered` operate on fundamentally different internal paths when processing an **Insert (brand new key)** versus an **Overwrite (mutating an existing key)**:

1. **`current::unordered` Mechanics:**
   - **Insert:** Appends an entry to the log and sets the corresponding bit to `1` in the newest bitmap chunk. No historical chunks are dirtied ($O(1)$).
   - **Overwrite:** Queries the indexer for the old entry location (`old_loc`), marks the old bit as `0` in a historical chunk, and sets the new bit to `1`. Every historical chunk modified becomes a **dirty chunk**, requiring chunk overlay construction and Merkle re-grafting.
2. **`current::ordered` Mechanics:**
   - **Insert:** Because QMDB must support **exclusion proofs** ($P_{\text{excl}} = K_{\text{prev}} < K < K_{\text{next}}$), inserting key $K$ requires searching the B-Tree for its lexicographical predecessor $E_p$, **appending two entries to the log** (the new entry $E_K$ and an updated predecessor $E'_p$ whose `next_key` points to $K$), and inserting into sorted partitions.
   - **Overwrite:** Updates the existing key's value. The predecessor link does not change; only a single entry is appended.

Without isolating write types, benchmarks could not evaluate whether the system's bottleneck stemmed from Merkle grafting or from ordered predecessor tracking.

---

## 2. Design & Technical Implementation

### A. The `WriteType` Domain Model

We introduced `WriteType` in [`storage/src/qmdb/benches/common.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/common.rs):
```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteType {
    /// Mutates existing keys pre-seeded into the database [0, num_keys)
    Overwrite,
    /// Inserts brand new keys not present in the pre-seeded keyspace [num_keys, 2 * num_keys)
    Insert,
    /// Alternating 50/50 mix of inserts and overwrites
    Mixed,
}
```

### B. Partitioned Key Generation Logic

To guarantee that an insert never collides with a pre-seeded key, we partitioned the 64-bit integer index before hashing it into a 32-byte cryptographic key:
```rust
let insert = match write_type {
    WriteType::Overwrite => false,
    WriteType::Insert => true,
    WriteType::Mixed => rng.next_u32().is_multiple_of(2),
};

// If inserting, offset by num_keys to guarantee the key does NOT exist in the index:
let idx = if insert {
    sampled + num_keys
} else {
    sampled
};
let key = hash_key(idx);
```

Controlled via `QMDB_MERKLEIZE_WRITE_TYPE=overwrite|insert|mixed`.

---

## 3. Empirical Results: The "Dual Asymmetry"

Running 10,000 writes on a 10,000,000-key database across 3 independent process groups revealed a profound **Dual Asymmetry**:

```
=== EMPIRICAL LATENCY BREAKDOWN (u = 10,000, keys = 10M) ===

       Configuration       Overwrites (ms)      Inserts (ms)        Insert vs Overwrite
    ───────────────────────────────────────────────────────────────────────────────────
    any::unordered            57.42 ms             3.71 ms           -93.5% (Inserts 15x faster)
    current::unordered       122.25 ms             4.38 ms           -96.4% (Inserts 28x faster)
    current::ordered         156.03 ms           138.66 ms           -11.1% (Comparable latency)
```

Comparing the configurations directly exposes the two distinct cryptographic costs:

### 1. The Exclusion Proof Tax on Inserts
When executing **Inserts**:
- `current::unordered` takes **4.38 ms**.
- `current::ordered` takes **138.66 ms**.
- **Overhead:** $+134.28\text{ ms}$ (**+3,132.8% overhead / 31.3x slowdown**).
- **Cause:** Searching the ordered B-tree, shifting sorted arrays, and writing double entries to update predecessor chains.

### 2. The Bitmap Grafting Tax on Overwrites
When executing **Overwrites**:
- `any::unordered` takes **57.42 ms**.
- `current::unordered` takes **122.25 ms**.
- **Overhead:** $+64.83\text{ ms}$ (**+115.3% overhead / 2.1x slowdown**).
- **Cause:** Overwriting existing keys inactivates historical bits, forcing dirty chunk overlay recomputation and grafted leaf hashing.

```
┌────────────────────────────────────────────────────────────────────────┐
│                        THE DUAL ASYMMETRY                              │
│                                                                        │
│   In current::unordered:   INSERTS are 28x FASTER than OVERWRITES      │
│   (Grafting only triggers when historical bits are turned off)         │
│                                                                        │
│   In current::ordered:     INSERTS are 31x SLOWER than in unordered   │
│   (Maintaining ordered predecessor links penalizes key creation)       │
└────────────────────────────────────────────────────────────────────────┘
```

This finding is a cornerstone of our research: **the optimal cryptographic architecture depends entirely on whether a workload is insert-heavy (e.g. minting tokens, new accounts) or overwrite-heavy (e.g. updating AMM pool balances, DEX state).**
