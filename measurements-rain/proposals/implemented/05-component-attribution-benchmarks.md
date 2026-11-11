# Proposal 05: Micro-Component Attribution Benchmarks (`qmdb_components`)

- **Proposal ID:** IMP-05
- **Status:** Implemented (Working tree changes in `storage/src/qmdb/benches/components.rs` and `storage/Cargo.toml`)
- **Author:** Rain Sun
- **Target Files:**
  - [`storage/src/qmdb/benches/components.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/components.rs)
  - [`storage/Cargo.toml`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/Cargo.toml)

---

## 1. Problem Statement & Motivation

While macro-benchmarks (`merkleize.rs`) report the end-to-end wall-clock latency of a batch, `merkleize()` is an aggregate function that performs multiple overlapping operations:
1. Encoding operations and computing leaf digests.
2. Building the operation log Merkle Mountain Range/Belt (MMR/MMB).
3. Mutating bits in the activity bitmap.
4. Materializing dirty chunk overlays.
5. Hashing complete bitmap chunks with operation subtree roots to form grafted leaves.
6. Merkleizing the upper tree over all grafted leaves.

Without decomposing these operations into **isolated microbenchmarks**, it is impossible to determine which specific sub-routine dominates CPU time or to assess whether optimizing a particular component (e.g., using SIMD for bitmap manipulation or hardware-accelerated SHA-256) would yield meaningful end-to-end gains.

---

## 2. Design & Technical Implementation

We created a standalone micro-component benchmark suite in [`storage/src/qmdb/benches/components.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/components.rs) registered under `Cargo.toml`:

```toml
[[bench]]
name = "qmdb_components"
harness = false
path = "src/qmdb/benches/components.rs"
```

The suite deliberately eliminates all database, disk, and in-memory index I/O to evaluate CPU subroutines in pure memory.

### The 5 Isolated Component Benchmarks

1. **Leaf Hashing (`bench_leaf_hashing`):**
   - Measures hashing 1,024 64-byte payload pairs into Merkle leaf digests using `merkle::hasher::Hasher::<F>::leaf_digest`.
   - Compares **MMR** versus **MMB** leaf position encoding overhead.
2. **Operation Tree Construction (`bench_operation_tree`):**
   - Measures building an in-memory Merkle tree (`Mem::<F, Digest>::new()`), adding 1,024 pre-computed leaves, and computing the peak hashes.
   - Isolates intermediate tree node allocation and hashing from operation payload serialization.
3. **Bitmap Mutation (`bench_bitmap_mutation`):**
   - Measures toggling 1,024 bits in `commonware_utils::bitmap::Prunable<32>`.
   - Compares **disjoint mutations** (bits scattered across multiple chunks) against **repeated mutations** (bits hitting the same 32-byte chunk).
4. **Grafted Leaf Hashing (`bench_grafted_leaf_hashing`):**
   - Measures combining a 32-byte bitmap chunk with a 32-byte operation subtree root:
     $$\text{grafted\_leaf} = \text{hash}(\text{bitmap\_chunk} \parallel \text{ops\_root})$$
   - Also benchmarks the zero-chunk identity bypass (`bench_zero_chunk_identity`).
5. **Upper Grafted Tree Merkleization (`bench_upper_grafted_tree`):**
   - Measures constructing and Merkleizing the upper MMR/MMB tree strictly over the set of grafted leaves to compute the canonical state root.

---

## 3. Analytical Utility

This component attribution harness provides the mathematical baseline for our hardware counter and CPU cycle profiling:
- **Dissecting the Overwrite Tax:** Comparing `bench_grafted_leaf_hashing` + `bench_upper_grafted_tree` against end-to-end `current_unordered_minus_any_unordered` determines exactly how much of the grafting penalty is attributable to raw SHA-256 compression calls versus memory copy overhead in chunk overlay generation.
- **MMR vs. MMB Smoothing:** Comparing `bench_operation_tree::<mmr>` against `bench_operation_tree::<mmb>` quantifies the exact CPU reduction achieved by MMB's delayed tree-merge strategy.
