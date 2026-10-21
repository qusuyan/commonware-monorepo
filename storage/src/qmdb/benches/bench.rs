//! Benchmark entry point for all QMDB benchmarks.

use criterion::criterion_main;

mod apply_batch;
mod chained_growth;
mod common;
mod generate;
mod init;
mod merkleize;

criterion_main!(
    // Each module owns a distinct QMDB lifecycle phase. A Criterion name filter can
    // therefore select one phase without running the other benchmark families.
    apply_batch::benches,
    chained_growth::benches,
    generate::benches,
    init::benches,
    merkleize::benches
);
