# Proposal 03 (Future): Client-Side Proof Generation & Verification Benchmarks

- **Proposal ID:** PROP-03
- **Status:** Proposed / Design Specification
- **Author:** Rain Sun
- **Target Components:**
  - [`storage/src/qmdb/current/unordered/db.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/current/unordered/db.rs)
  - [`storage/src/qmdb/current/ordered/db.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/current/ordered/db.rs)
  - [`storage/src/qmdb/benches/`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches)

---

## 1. Problem Statement & Motivation

All benchmarks executed thus far measure the **write path maintenance cost**:
- The latency experienced by block proposers, validators, or database writers who must compute roots (`merkleize()`) and update activity bitmaps.

However, in blockchain architectures, authenticated storage exists primarily to serve **light clients, bridging protocols, and remote verifiers** via cryptographic proofs. 

A comprehensive evaluation of the "Cryptographic Tax" must assess the **read path and serving costs**:
1. How long does a full node take to **generate** an inclusion proof vs. an exclusion proof?
2. How long does a light client or smart contract take to **verify** the proof?
3. What is the **wire payload size** in bytes that must be transmitted over p2p networks or stored in blockchain receipts?

---

## 2. Design Specification

### A. Proof Generation Comparison

We will implement dedicated Criterion read benchmarks targeting:
1. **Current-State Inclusion Proofs (`key_value_proof`):**
   - Implemented in [`current/unordered/db.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/current/unordered/db.rs#L91):
   - Generates an MMR audit path for the historical operation leaf, combined with the activity bit proof from the grafted layer.
2. **Exclusion Proofs (`exclusion_proof`):**
   - Implemented in [`current/ordered/db.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/current/ordered/db.rs#L225):
   - Locates predecessor $K_{\text{prev}}$ in the ordered index.
   - Generates the inclusion proof for $E_{\text{prev}}$, demonstrating that $E_{\text{prev}}.\text{next\_key} > K_{\text{target}}$, thereby mathematically proving that $K_{\text{target}}$ does not exist.

### B. Verification Latency & Complexity

We will benchmark client verification in isolation without database dependencies:
1. **Inclusion Verification:**
   - [`verify_key_value_proof`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/current/unordered/db.rs#L58): Hashes $O(\log N)$ audit peaks against the canonical root.
2. **Exclusion Verification:**
   - [`verify_exclusion_proof`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/current/ordered/db.rs#L144): Verifies the predecessor's inclusion proof, checks key bounds ($K_{\text{prev}} < K < K_{\text{next}}$), and confirms the predecessor's active status.

### C. Proof Wire Size (Bytes)

We will record serialized byte lengths:
$$\text{Size}(\text{Proof}) = |\text{audit\_path}| \times 32\text{ bytes} + |\text{payload}| + |\text{bitmap\_chunk\_proof}|$$

---

## 3. Anticipated Research Impact

This experiment will reveal whether the **31x write-path penalty of exclusion proofs** is balanced by cheaper verification, or whether systems should abandon in-database exclusion proofs altogether in favor of:
- Offloading negative proofs to off-chain zero-knowledge provers.
- Appending "tombstone" entries and proving tombstone inclusion rather than key absence.
