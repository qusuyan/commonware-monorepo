# Proposal 03: Access Skewness & The Numerically Stable Zipf Sampler

- **Proposal ID:** IMP-03
- **Status:** Implemented (Merged PR #3: `skewness`, commits `075ae4a1` & `be914770`, plus working tree numerical stabilization)
- **Author:** Rain Sun
- **Target Files:**
  - [`storage/src/qmdb/benches/common.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/common.rs)
  - [`storage/src/qmdb/benches/merkleize.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/merkleize.rs)
  - [`storage/tests/qmdb_bench_sampling.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/tests/qmdb_bench_sampling.rs)
  - [`measurements-rain/experiments/skew/`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/experiments/skew)

---

## 1. Problem Statement & Motivation

Real-world blockchain and database workloads do not touch the state space uniformly at random. A tiny fraction of "hot" accounts and contracts (such as decentralized exchange liquidity pools, stablecoin contracts, and sequencer accounts) absorb the vast majority of all transactions. This empirical phenomenon is commonly modeled as a **Zipfian (power-law) distribution**:

$$P(k) \propto \frac{1}{k^s}$$

where $s \ge 0$ is the skewness exponent ($s=0$ represents uniform random, while $s \ge 1.0$ represents heavy concentration).

In QMDB, access skew has a profound theoretical interaction with **bitmap grafting**:
- Under **uniform overwrites**, writes scatter across the entire keyspace, dirtying thousands of distinct 32-byte bitmap chunks across the historical MMR/MMB.
- Under **skewed overwrites**, writes repeatedly strike the same small subset of keys. This confines bit mutations to a handful of bitmap chunks, dramatically reducing the number of grafted leaves that must be recomputed and merged into the canonical root.

However, standard libraries (including `rand_distr::Zipf`) suffer from catastrophic floating-point cancellation or division-by-zero around $s \approx 1.0$ ($1 - s \to 0$), which is precisely the exponent of greatest interest in systems research.

---

## 2. Design & Technical Implementation

### A. The `StableZipf` Finite Rejection Sampler

To provide mathematically sound, continuous evaluation across all exponents $s \in [0, \infty)$, we implemented `StableZipf` in [`storage/src/qmdb/benches/common.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/common.rs).

`StableZipf` operates as an $O(1)$-time, $O(1)$-memory rejection sampler using continuous envelope integration:
1. When $\delta = 1.0 - s \ne 0$, the standard envelope integral $\int_1^N x^{-s} dx = \frac{N^\delta - 1}{\delta}$ is evaluated using `exp_m1(delta * ln(N))` to prevent precision loss.
2. When $\delta = 0$ ($s = 1.0$), the integral evaluates cleanly to $1.0 + \ln(N)$.
3. Inversion of the proposal CDF utilizes `ln_1p` to maintain high numerical fidelity before division by $\delta$.
4. Acceptance ratios are evaluated strictly in log-space:
   $$\ln(\alpha) = -s \cdot (\ln(\text{rank}) - \ln(\text{proposal}))$$

```rust
struct StableZipf {
    num_keys: u64,
    exponent: f64,
    delta: f64,
    proposal_mass: f64,
}

impl StableZipf {
    fn new(num_keys: u64, exponent: f64) -> Self {
        let delta = 1.0 - exponent;
        let log_keys = (num_keys as f64).ln();
        let proposal_mass = if delta == 0.0 {
            1.0 + log_keys
        } else {
            1.0 + (delta * log_keys).exp_m1() / delta
        };
        Self { num_keys, exponent, delta, proposal_mass }
    }
    
    fn sample(&self, rng: &mut TestRng) -> u64 {
        loop {
            let uniform: f64 = StandardUniform.sample(rng);
            let mass = uniform * self.proposal_mass;
            let proposal = if mass <= 1.0 {
                mass
            } else if self.delta == 0.0 {
                (mass - 1.0).exp()
            } else {
                ((self.delta * (mass - 1.0)).ln_1p() / self.delta).exp()
            };
            let rank = (proposal + 1.0).floor();
            if !rank.is_finite() || rank < 1.0 || rank > self.num_keys as f64 {
                continue;
            }
            let acceptance = if rank == 1.0 {
                1.0
            } else {
                (-self.exponent * (rank.ln() - proposal.ln())).exp()
            };
            if StandardUniform.sample(rng) < acceptance {
                return rank as u64 - 1;
            }
        }
    }
}
```

### B. Validation & Test Coverage

We created automated property and distribution tests in [`storage/tests/qmdb_bench_sampling.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/tests/qmdb_bench_sampling.rs) verifying that:
- Rank 0 is monotonically the most probable rank.
- Key ranks stay strictly within the bounds $[0, \text{num\_keys}-1]$.
- Statistical frequencies match the theoretical Zipf probability distribution within tight Chi-squared confidence bounds.

---

## 3. Empirical Results & Findings

We benchmarked 10,000 overwrites across a 10,000,000-key keyspace across 5 skewness exponents ($s \in \{0.0, 0.5, 1.0, 1.5, 2.0\}$):

| Zipf Exponent ($s$) | Access Pattern | `any_unordered` (ms) | `current_unordered` (ms) | `current_ordered` (ms) | Grafting Penalty ($Current - Any$) |
| :---: | :---: | :---: | :---: | :---: | :---: |
| **$s = 0.0$** | Uniform Random | 51.54 ms | 110.53 ms | 131.03 ms | +116.5% |
| **$s = 0.5$** | Mild Skew | 57.94 ms | 116.94 ms | 115.95 ms | +104.2% |
| **$s = 1.0$** | Classic Zipf | 37.20 ms | 64.05 ms | 74.93 ms | +73.2% |
| **$s = 1.5$** | Strong Skew | 6.33 ms | 12.01 ms | 11.99 ms | +90.5% |
| **$s = 2.0$** | Extreme Skew | **1.96 ms** | **4.23 ms** | **3.94 ms** | +116.3% |

### Key Takeaways
1. **The Locality Shield:** As skew increases from uniform ($s=0$) to extreme ($s=2.0$), absolute Merkleization latency plummets by **96%** (from 110.5 ms down to 4.2 ms for `current_unordered`).
2. **Proportional Constant:** Despite the dramatic drop in absolute latency, the **relative grafting penalty remains remarkably constant at ~73% to 116%**. Even when only a few chunks are dirty, hashing those chunks and traversing the upper tree to the root still costs approximately 2x the time of un-grafted MMR logging.
