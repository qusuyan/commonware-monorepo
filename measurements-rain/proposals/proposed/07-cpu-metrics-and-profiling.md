# Proposal 07: Native CPU Accounting and Process Monitoring

- **Proposal ID:** PROP-07
- **Status:** Native accounting and runtime monitoring implemented; hardware counters and durable lifecycle deferred
- **Author:** Rain Sun
- **Updated:** 2026-11-04
- **Initial scope:** Add native benchmark CPU accounting with `libc`, and periodic process CPU monitoring with the existing `sysinfo` dependency.
- **Primary comparison:** `any::unordered_btree::fixed::mmb` versus `current::ordered::fixed::mmb chunk=32`, using the same ordered index implementation and matched workload settings.
- **Later scope:** Actual hardware counters through Linux `perf`, in separate native runs.

## 1. Purpose and current implementation

We want to distinguish three questions: how long a QMDB operation takes, how much CPU resource it consumes across all worker threads, and which parts of the workload account for that cost. Wall time, native CPU accounting, and Callgrind answer different parts of these questions.

The repository currently contains:

- [Criterion/shared workload harness](../../../bench/src/lib.rs): measures elapsed time using `Instant`; Gungraun collection is toggled around the shared workload's `iter()`.
- [Gungraun workload](../../../storage/src/qmdb/benches/merkleize_gungraun.rs): registers `any::unordered::fixed::mmr` and `current::ordered::fixed::mmb chunk=256`. Each seeds 10,000 keys and prepares 1,000 overwrite attempts, sampled with replacement, with no speculative ancestor. Seed sync and Commonware page-cache clearing are enabled; the OS page cache is retained.
- [Tracking configuration](../../../.github/benchmark-tracking.toml): reports `Ir`, `L1hits`, `LLhits`, `RamHits`, `TotalRW`, `EstimatedCycles`, and the workload-defined `blob_reads`. Regression gates allow a 10% increase in `EstimatedCycles` and `blob_reads` against each variant's own baseline.
- [Process collector](../../../runtime/src/process/metered.rs): uses `sysinfo` for memory and CPU every 10 seconds. CPU utilization is exported in basis points after priming; CPU sampling is independently configurable.
- [B-tree baseline](../../../storage/src/qmdb/any/unordered/fixed.rs): `BTreeDb` already exists. The [apply study](../../../storage/src/qmdb/benches/apply_study.rs) already compares the B-tree Any baseline with Current ordered, using MMB.

Native user/kernel CPU accounting is implemented in `commonware-bench::cpu`.
The `qmdb_cpu` executable and `run-cpu-telemetry.sh` capture one independent batch
per sample for either merkleize or apply. Periodic `sysinfo::Process::cpu_usage()`
monitoring is also implemented. See the [benchmark guide](../../benchmark.md#qmdb_cpu-native-cpu-accounting)
for runnable commands and output files. Historical design requirements below
explain the measurement contract; hardware counters and durable lifecycle work
remain deferred.

The current Callgrind pair changes index structure, Merkle family, and authentication semantics together. Preserve it as a regression baseline; add explicitly named matched cases for the research comparison. Sharing an index implementation reduces one confound but does not make index access patterns, allocations, operation counts, or storage reads identical. Report the difference as the combined cost of Current ordered semantics under matched settings, rather than attributing it exclusively to cryptographic hashing or exclusion proofs.

## 2. Metrics and interpretation

### A. Existing Callgrind measurements

Retain Callgrind/Gungraun as a separate instrumentation mode:

- `Ir`: executed instructions under instrumentation.
- `L1hits`, `LLhits`, `RamHits`: modeled accesses satisfied at the first cache level, last cache level, or beyond those levels. These include instruction and data accesses.
- `TotalRW`: instruction fetches plus data reads and writes; it is an access count, not bytes transferred.
- `EstimatedCycles`: the Gungraun 0.19.4 model `L1hits + 5 * LLhits + 35 * RamHits`. This is not a hardware cycle measurement and must not be used as the denominator for hardware IPC.
- `blob_reads`: runtime blob-read call count, not physical disk reads.

Instruction counts are useful for repeatable comparisons, but exact zero variance is not guaranteed, especially with async execution and worker pools. Additional raw read/write and cache-miss events may be exposed later; they are not currently selected tracking outputs.

### B. Native benchmark accounting with `libc` (first priority)

Take cumulative snapshots using `libc::getrusage(libc::RUSAGE_SELF, ...)` before and after the measured workload. On Linux this includes all threads in the current process, including Tokio and Rayon workers. It excludes separate child processes. Do not use thread-only accounting for async work that may migrate between threads.

Collect interval deltas:

- `user_cpu_ns`: `ru_utime`, converted from the OS time representation to nanoseconds.
- `system_cpu_ns`: `ru_stime`, converted in the same way.
- `voluntary_context_switches`: `ru_nvcsw`.
- `involuntary_context_switches`: `ru_nivcsw`.
- `minor_page_faults`: `ru_minflt`, faults not requiring I/O on Linux.
- `major_page_faults`: `ru_majflt`, faults requiring I/O on Linux.

Record `wall_ns` with `std::time::Instant`, plus the number of measured batches and submitted write attempts. Compute:

```text
total_cpu_ns       = user_cpu_ns + system_cpu_ns
cpu_ns_per_batch   = total_cpu_ns / measured_batches
cpu_ns_per_write   = total_cpu_ns / submitted_write_attempts
average_cores_used = total_cpu_ns / wall_ns
cpu_usage_percent  = 100 * average_cores_used
```

Use floating-point division for derived values and reject zero denominators. Also report user and system time separately per batch. Submitted writes may contain duplicate keys; do not label them as distinct updated keys or persisted operations.

For example, 400 ms of total CPU time over 100 ms of wall time means four cores used on average, or 400% CPU utilization. More than 100% is valid; do not divide by the machine's logical CPU count unless reporting a separately named host-capacity percentage.

CPU time is resource consumption, not an instruction count. It still depends on processor frequency, cache behavior, and the execution environment. Kernel CPU time captures kernel work charged to this process; it does not include all background writeback, interrupt, or device activity. Wall time minus summed CPU time is not a valid I/O-wait estimate for a multithreaded process.

### C. Periodic monitoring with `sysinfo` (second priority)

Extend the existing runtime collector rather than adding a second sampler:

- Refresh the current PID with `ProcessRefreshKind::nothing().with_memory().with_cpu()`.
- Publish `process.cpu_usage()` as a process CPU percentage gauge, preserving the convention that one fully occupied core is 100%.
- Keep RSS and virtual memory in bytes.
- Optionally expose `process.accumulated_cpu_time()` as cumulative CPU milliseconds, with the unit explicit in the exported metric name/help. This is total CPU time and does not replace the user/kernel split from `getrusage`.

CPU percentage requires a previous refresh and a sufficiently long interval. Prime the collector, omit the first CPU sample, and respect `sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`. Retain the existing 10-second cadence initially. A missing process or unavailable measurement must not be emitted as a fabricated zero.

The implemented integer gauge is `process_cpu_usage_basis_points`: 10,000 means 100%, rounded to the nearest 0.01 percentage point. It is registered only after a valid second refresh and removed when a reading is unavailable. `tokio::Config::with_process_cpu_monitoring(bool)` defaults to enabled. The native and Gungraun runners disable CPU sampling while retaining memory sampling, so the new monitoring does not add polling to the existing instruction-count baseline. The optional cumulative `sysinfo` CPU-time metric remains deferred.

This sampler answers how busy a running process is over time. It does not isolate individual short QMDB operations, and a 10-second sample may include setup, teardown, or several phases. Use the `libc` snapshots for benchmark attribution. Polling and metric serialization themselves consume resources; make CPU monitoring independently configurable and record whether it was enabled during benchmark runs.

### D. Supporting memory measurements

RSS is useful context, although it is not a CPU metric. Use `sysinfo` for periodic current RSS and virtual memory. If reporting `ru_maxrss`, normalize its platform-specific units to bytes: Linux reports KiB and macOS reports bytes. It is a process-lifetime high-water mark, not interval memory consumption. Do not subtract before/after peaks and call the result memory allocated by a batch. Separate-process runs are needed for comparable per-run peak measurements.

### E. Later hardware counters

On Linux, add actual `cycles`, `instructions`, cache references/misses, and branches/branch misses through the `perf` CLI first. No new Rust PMU dependency is needed for this later phase. Report cycles and instructions per submitted write, `IPC = instructions / cycles`, and miss counts alongside miss rates. Event availability and meanings depend on the host CPU; record unsupported events and counter multiplexing rather than substituting zeros.

A whole-process `perf stat` invocation includes setup unless the runner explicitly controls counter enable/disable boundaries. Label whole-run diagnostics accordingly; precise phase counters require later boundary integration. Keep CPU affinity, user/kernel event scope, and inherited worker-thread coverage explicit. Higher IPC alone is not proof of a faster or cheaper implementation.

## 3. Measurement boundaries and sampling

Run native accounting separately from Callgrind. CPU time measured under Valgrind includes instrumentation overhead and is not native QMDB CPU cost. Do not automatically toggle Callgrind inside the native accounting helper.

For each phase, finish setup and prepare inputs before taking the start snapshot. Await all work belonging to the operation before the end snapshot. Serialize metrics, validate results, and destroy databases afterward.

### Speculative merkleization

Measure batch creation, staging precomputed writes, `merkleize`, and root retrieval. Exclude key/value generation, database opening/seeding/sync, snapshot preparation, and cache preparation. This phase includes any lookups, allocations, or storage reads performed by the operation; it is not a hashing-only benchmark. Preserve the existing workload's ownership and destruction semantics and document them rather than claiming that every deallocation is excluded.

### Apply only

Use the prepared-batch boundary in `apply_study.rs`: construct and merkleize the batch before accounting starts; measure `apply_batch` only. Keep root correctness checks after accounting stops. Do not describe apply-only results as durable writes; commit and sync are outside this phase. The upstream `apply_batch.rs` benchmarks remain separate cases.

### Durable lifecycle (later workload)

Deferred: a separately labeled workload may measure staging, merkleization, application, commit, and sync together. It must prepare inputs beforehand, retain the correct database state between operations, and include the full durability barrier in the measured window. Do not infer lifecycle latency or CPU cost by adding independently sampled phase results with different states or cache conditions.

### Precision and overhead

Short operations may be smaller than the OS CPU-accounting resolution. Nanosecond output units do not imply nanosecond measurement precision. Prefer a native benchmark runner that executes enough preprepared operations per window to obtain meaningful CPU-time deltas and amortize snapshot overhead.

For repeatable speculative batches, keep the seed unchanged and specify whether caches are retained or cleared between windows. For apply-only windows, prepare independent ready-to-apply database/batch pairs outside the window, or use a documented evolving-state workload. If those pairs require excessive memory, increase work per batch or report the precision limitation; do not move setup into the measured phase merely to lengthen it.

A single window must contain only its declared workload. If preparation must occur between iterations, either use separate snapshots per iteration and sum their deltas, acknowledging their overhead and quantization, or redesign the window. Do not wrap Criterion's entire sampling loop and call it operation-only CPU time, because `before_iter`, setup, and teardown are excluded from its existing wall timer.

Record an empty-window overhead distribution. Do not blindly subtract it or assert a universal microsecond threshold. Repeat matched windows and report sample count and variability. Native runs should execute one benchmark case per process where practical, with no concurrent benchmark cases. Process-wide accounting still includes runtime housekeeping and other threads in that process. Disable optional periodic CPU sampling for primary accounting runs where supported, and measure its overhead in separate runs.

The `getrusage` snapshots and `Instant` reads cannot be simultaneous. Use a fixed documented order around the same workload, keep snapshots adjacent to the wall timer, and quantify the boundary overhead. Do not promise cycle-exact alignment.

## 4. Implementation plan

### A. Shared native accounting in `commonware-bench`

Add a small Unix CPU-accounting module, for example `bench/src/cpu.rs`, with explicit fallible start/finish snapshots and a result carrying durations and counter deltas. Keep it independent of QMDB and Gungraun. Add a target-specific `libc.workspace = true` dependency to `bench/Cargo.toml`; the workspace already declares `libc`. Follow the crate's stability annotations for any public surface.

Implementation requirements:

- Check every `getrusage` return value and return `std::io::Error::last_os_error()` on failure.
- If using `MaybeUninit`, call `assume_init` only after successful initialization; document unsafe blocks with `// SAFETY:`.
- Convert each complete `timeval` to a checked integer duration before subtraction. Handle microsecond borrowing correctly and reject negative or overflowing deltas.
- Use checked conversions for signed OS counters. Unsupported fields/platforms must be explicit; do not cast negative values to large unsigned values or silently clamp invalid deltas.
- Return metrics explicitly from `finish`; do not perform I/O or silently discard errors in `Drop`.
- Platform-gate the implementation, initially validating Linux and macOS. Preserve builds for other targets with an explicit unsupported result or feature gating, not synthetic measurements.
- Do not add placeholder `instructions = 0` or `cycles = 0`; this module does not measure them.

### B. QMDB native runner and output

Reuse shared workload preparation and phase boundaries. Add a dedicated native accounting mode/runner rather than changing Criterion's latency estimator or routing these measurements through the Callgrind-only metric emitter.

The implemented `measurements-rain/scripts/run-cpu-telemetry.sh <run-id>` invokes
`qmdb_cpu`, records a manifest, and archives raw samples plus summaries under
`measurements-rain/results/<run-id>/cpu/`. Require `--phase merkleize|apply`,
`--variant any_unordered_btree|current_ordered`, `--keys`, and `--updates`.
Optional flags are `--samples` (10), `--write-type` (overwrite), and `--skew` (0).
The script owns `--output`; direct executable invocations require it. Both output
and run directories must be new. Snapshot reuse defaults to `required`.

V1 uses one batch per sample, with independent scratch seeds and identical seed-99
inputs. It retains Commonware caches after preparation and does not clear the OS
cache. Zero CPU deltas and CPU deltas within 10 times paired empty-window overhead
carry precision warnings. No overhead is subtracted. Summaries preserve raw
samples and report mean, median, and sample standard deviation; the latter is
unavailable for a single sample.

Each raw sample must include:

- Source (`libc_getrusage`), phase, window/sample index, measured batch count, and submitted write count.
- Wall, user, system, and total CPU time with explicit units; context-switch and page-fault deltas.
- Variant, Merkle family, chunk bytes, seed/keyset identity, key count, write type/skew, worker count, chaining, seed sync, Commonware cache policy, and OS-cache policy.
- Host/OS/architecture, CPU model, Rust version, Git revision/dirty state, and periodic monitoring settings in the manifest.

Keep raw integer durations and counters. Derive percentages and normalized values in summaries, preserving numerator and denominator. Emit unsupported values as unavailable with a reason. Do not feed nondeterministic native timing values into the existing Callgrind regression gates without a separately justified statistical policy.

### C. Runtime monitoring

Extend `runtime/src/process/metered.rs` with the CPU refresh and gauges described above, using the existing `sysinfo.workspace` dependency. Keep monitoring independent of benchmark accounting and follow runtime configuration conventions for enabling/disabling CPU sampling. No new async runtime dependency is required.

### D. Matched research comparison

Use the existing Any `BTreeDb` baseline and Current ordered with MMB and 32-byte chunks for the initial research pair. Match partitioning, key/value sizes, translator, cache capacity, worker count, write sequence, and initial database state where applicable. Record unavoidable semantic/state differences.

Add named Gungraun research cases separately if needed; do not silently replace the existing regression variants or compare new configurations against their old baselines. Keep native, Callgrind, and later hardware outputs distinct.

## 5. Validation and acceptance criteria

1. **Accounting correctness:** Unit-test time conversion across second boundaries, checked deltas, normalization, zero denominators, unavailable values, and platform-specific RSS units. Exercise the error path without reading uninitialized memory.
2. **Boundary correctness:** Use a recording/mock snapshot source to verify setup/input preparation precedes start, the intended workload completes before finish, and serialization/teardown occur afterward. Verify failure handling does not emit a successful partial sample.
3. **Native smoke runs:** Execute empty, CPU-active, and waiting windows on Linux and macOS. Observe nonnegative deltas and meaningful CPU accumulation for sufficiently long CPU-active runs. Treat timing expectations as diagnostics with broad tolerances, not fixed microsecond CI assertions.
4. **Worker coverage:** Verify process accounting captures work performed by worker threads. Do not require a particular speedup or utilization on a shared CI host. Cross-check longer isolated Linux runs against external process accounting using matching scopes.
5. **Monitoring behavior:** Verify first-sample handling, fractional percentages, values above 100%, missing-process handling, exported units, cadence, and the CPU-monitoring configuration. Compare native runs with sampling enabled and disabled to characterize overhead.
6. **State and scaling:** Compare matched cases using identical prepared writes and equivalent initial/cache settings. Sweep write counts, but do not require exactly linear instruction or CPU-time growth: duplicate writes, tree shape, batching, and parallelism can change scaling. Fresh and reopened seeds need not produce identical counters unless their full relevant state is equivalent.
7. **Isolation and provenance:** Confirm native accounting does not enable Callgrind, outputs identify all measurement sources and units, and manifests make every reported comparison reproducible. Keep operation correctness checks outside measured windows.

Implementation checks should use the repository's focused `just test` and `just clippy` commands for changed crates. Add focused tests with the implementation; this document does not claim that a new profiler test target already exists. Run dependency and public-API checks where required by repository guidance.

## 6. Delivery order and expected conclusions

1. Implement `libc` user/system CPU time, wall time, normalized CPU cost, and average cores used; collect context switches and page faults from the same snapshots.
2. Add periodic `sysinfo` CPU monitoring and characterize its overhead, retaining current memory metrics.
3. Run the matched B-tree/MMB comparison across merkleize and apply-only phases; add the durable lifecycle as a separate workload.
4. Add Linux hardware counters when native CPU results warrant deeper investigation.

These measurements should establish whether latency improvements reduce total CPU consumption or use more parallelism, how user and kernel costs differ across phases, and whether scheduling or page faults coincide with anomalous samples. Cache/branch explanations require later hardware evidence; native CPU time alone cannot identify those causes.

## 7. Implementation validation (2026-11-04)

Validated locally on macOS/Apple Silicon:

- Shared accounting conversion, overflow, syscall-failure, normalization, and worker-thread smoke tests.
- Runtime priming, fractional/over-100% CPU samples, unavailable readings, memory-only collection, and monitoring disablement tests.
- Recording-source tests for both phase boundaries and accounting failure propagation.
- Native smoke runs for both variants and phases, checking matching prepared/applied roots, stable per-sample roots, unchanged saved-master file contents, units, summaries, and provenance.
- Launcher fixtures for capture, invalid arguments, and existing-destination rejection; a real Cargo/script launch also verified Cargo's trailing `--bench` marker handling.
- BETA runtime stability build. The benchmark crate passes Clippy directly; runtime and CPU target Clippy checks pass with the installed toolchain's unrelated `chunks_exact_to_as_chunks` and `map_or_identity` baseline lints allowed on the command line only.

Linux execution and io_uring checks remain pending. The local installation lacks
`just`, nextest, Miri, cargo-udeps, and the WASM target standard library; focused
checks use Cargo directly. The WASM build was attempted and failed because the
target is not installed. No unavailable validation is counted as passing.

## References

- [Existing benchmark guide](../../benchmark.md#qmdb_gungraun-callgrind-cpu-counters)
- [CPU work attribution plan](../../measurement.md#2-attribute-cpu-work)
- [Linux getrusage documentation](https://www.man7.org/linux/man-pages/man2/getrusage.2.html)
- [sysinfo Process documentation](https://docs.rs/sysinfo/latest/sysinfo/struct.Process.html) (check APIs against the repository's pinned version during implementation)
- [Linux perf stat documentation](https://man7.org/linux/man-pages/man1/perf-stat.1.html)
