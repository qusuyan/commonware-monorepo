# Proposal 04 (Future): End-to-End Durable Write Lifecycle Benchmarks

- **Proposal ID:** PROP-04
- **Status:** Phase 3 Implemented ([IMP-08](../implemented/08-apply-study-batch-application.md)); Phase 4 (Durable `sync`) Planned
- **Author:** Rain Sun
- **Target Components:**
  - [`storage/src/qmdb/benches/apply_study.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/apply_study.rs) (Phase 3 Implemented)
  - [`storage/src/qmdb/benches/apply_batch.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/apply_batch.rs)
  - [`storage/src/qmdb/current/db.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/current/db.rs)
  - [`storage/src/journal/`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/journal)

---

## 1. Problem Statement & Motivation

Our primary benchmarks (`merkleize.rs`) intentionally measure **Phase 2 (Speculative Cryptographic Merkleization)** in memory. 

However, writes in a production database pass through four distinct phases:

```
       PHASE 1                  PHASE 2                  PHASE 3                  PHASE 4
User / Tx Execution      Cryptographic Proof       Database Application      Physical Persistence
┌────────────────────┐   ┌─────────────────────┐   ┌─────────────────────┐   ┌─────────────────────┐
│ batch.write(k, v)  │──►│  batch.merkleize()  │──►│ db.apply_batch(b)   │──►│ db.commit() / sync()│
└────────────────────┘   └─────────────────────┘   └─────────────────────┘   └─────────────────────┘
      In RAM                    In CPU/RAM                In OS Page Cache             On Flash SSD
 (Buffer staging)         (Merkle root calc)        (Appended to files)       (NAND cells flushed)
```

In Phase 2, `batch.merkleize()` updates in-memory trees and validates the state root without touching the filesystem. The batch is subsequently discarded.

This leaves an important systems question open:
> **When batches are committed durably to SSD storage, does disk I/O latency mask the CPU overhead of bitmap grafting and exclusion proofs, or does the extra write amplification of exclusion proofs (writing 2 entries instead of 1) amplify disk bottlenecks?**

---

## 2. Design Specification

We will extend the benchmark suite to capture the complete lifecycle:

### A. Lifecycle Phase Breakdown

1. **Phase 2: Merkleize Latency ($T_{\text{merkle}}$):**
   - In-memory Merkle tree hashing and root generation.
2. **Phase 3: Apply Batch Latency ($T_{\text{apply}}$):**
   - Appending operations to the OS page cache journal files via `db.apply_batch(batch)`.
   - Ingesting activity bitmap changes and advancing the in-memory indexer.
3. **Phase 4: Commit & Sync Latency ($T_{\text{sync}}$):**
   - Issuing `fdatasync()` / `fsync()` to force dirty OS page cache pages onto physical SSD NAND flash.
   - Advancing the durable recovery boundary.

### B. Write Amplification Factor (WAF) Tracking

We will instrument the storage layer to record exact physical I/O metrics:
- Total bytes written to disk per 10,000 logical updates.
- Under `current::unordered`: Expected 1 entry write per update.
- Under `current::ordered`: Expected **2 entry writes per insert** (the new entry $E_K$ plus the updated predecessor $E'_p$).

This will quantify the physical storage bloat and disk wear caused by exclusion proof maintenance.

---

## 3. Implementation Steps

1. **Revive and Expand `apply_batch.rs`:**
   Extend `storage/src/qmdb/benches/apply_batch.rs` with the write-size, write-type, and skew parameters established in `merkleize.rs`.
2. **Introduce Durability Modes:**
   - Mode 1: Speculative in-memory only (`merkleize`).
   - Mode 2: Buffered filesystem append (`apply_batch` without sync).
   - Mode 3: Fully durable synced batch (`apply_batch` + `commit` + `sync`).
3. **Correlate with Storage Hardware:**
   Run durable benchmarks across both standard NVMe SSDs and memory-backed ramdisks to isolate kernel filesystem overhead from physical flash write limits.
