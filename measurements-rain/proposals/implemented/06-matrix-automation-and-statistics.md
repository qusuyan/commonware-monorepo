# Proposal 06: Full-Matrix Automation, Rotated Repeats & Statistics

- **Proposal ID:** IMP-06
- **Status:** Implemented (Working tree changes across `measurements-rain/scripts/`)
- **Author:** Rain Sun
- **Target Files:**
  - [`measurements-rain/scripts/run-full-matrix.sh`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/run-full-matrix.sh)
  - [`measurements-rain/scripts/compare-experiment.jq`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/compare-experiment.jq)
  - [`measurements-rain/scripts/summarize-isolated-suite.py`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/summarize-isolated-suite.py)
  - [`measurements-rain/scripts/render-experiment-figures.py`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/render-experiment-figures.py)

---

## 1. Problem Statement & Motivation

Benchmarking complex database systems is susceptible to environmental artifacts:
- **Thermal Throttling & CPU Turbo Boost:** Running configurations in fixed order (e.g. always `any`, then `current_unordered`, then `current_ordered`) systematically biases the later configurations as CPU cores heat up and clock speeds throttle.
- **Cache Warmup & OS Page Cache Pollution:** The first configuration executed in a run often incurs initial cold-page page faults that later configurations avoid.
- **Sample Non-Independence:** Criterion's internal 10 samples are executed in a tight loop within a single process. They do not constitute independent statistical process runs.

To ensure academic-grade experimental rigor, we required an automated, reproducible harness enforcing strict process separation, execution order rotation, and standardized statistical reporting.

---

## 2. Design & Technical Implementation

### A. The 12-Step Matrix Suite (`run-full-matrix.sh`)

We constructed an orchestrator script ([`run-full-matrix.sh`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/run-full-matrix.sh)) that automates the execution of the entire multidimensional benchmark space:
- **Steps 1–9 (Write Size Sweep):** 200, 500, 1000, 1500, 2000, 5000, 7000, 8000, and 10000 updates on a 4M-key database.
- **Step 10 (Write Type Sweep):** Separate overwrite, insert, and delete cases at 2,000 updates.
- **Step 11 (Skew Sweep):** Zipf exponents $s \in \{0.0, 0.5, 1.0, 1.5, 2.0\}$ at 2,000 updates.
- **Step 12 (Database Scale Sweep):** 1M, 2M, 4M, and 10M keys at 2,000 uniform overwrites.

Key safeguards implemented in the runner:
1. **Source Code Hash Verification:** SHA-256 hashes of all benchmark source files are recorded at suite startup and re-verified before every step to ensure the codebase was not modified during long-running tests.
2. **Disk Space Invariant:** Asserts at least 20 GiB of free SSD space before each step.
3. **Resumability:** `--resume-from-step <1-12>` enables resuming an interrupted sweep without discarding completed steps.

### B. Rotated Execution Order Across Independent Process Repeats

Every experiment step runs **3 independent operating system process repeats**
($r=1, 2, 3$). All active studies use the controlled pair
$A = \text{any\_unordered\_btree}$ and $B = \text{current\_ordered}$:

$$\text{Repeat 1: } A \to B$$
$$\text{Repeat 2: } B \to A$$
$$\text{Repeat 3: } A \to B$$

Both variants use the same nonpartitioned ordered B-Tree index type. The
write-type experiment also rotates overwrite, insert, and delete workload order
independently across three repeats. Alternation reduces order bias; it does not
eliminate thermal drift, cache effects, or measurement noise.

### C. Standardized Schema v2 Comparisons (`compare-experiment.jq`)

We established a standardized JSON comparison schema calculating the descriptive
`current_ordered_minus_any_unordered_btree` delta. This controls the index type;
the delta still includes record-layout and authentication differences.

### D. Automated Figure Rendering (`render-experiment-figures.py`)

Python rendering scripts compile raw CSV data into publication-quality vector PDF and PNG figures located in [`measurements-rain/results/figures/`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/results/figures):
- `qmdb-configuration-uniform.pdf`: Latency scaling across batch sizes.
- `write-type-effect.pdf`: The insert vs. overwrite dual asymmetry.
- `skew-sensitivity.pdf`: Performance response to Zipfian skewness.
- `timer-boundary-latency.pdf`: Before/after comparison of timer boundary isolation.
