//! Phase boundaries shared by the native runner and boundary tests.

use crate::common::{Digest, stage_prepared_writes};
use commonware_bench::cpu::{Measurement, Sample};
use commonware_storage::{
    merkle::mmb,
    qmdb::any::traits::{DbAny, MerkleizedBatch, UnmerkleizedBatch},
};
use std::{hint::black_box, io};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Merkleize,
    Apply,
}

impl Phase {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Merkleize => "merkleize",
            Self::Apply => "apply",
        }
    }
}

pub trait Recorder {
    type Window;
    type Output;
    fn start(&mut self) -> io::Result<Self::Window>;
    fn finish(&mut self, window: Self::Window) -> io::Result<Self::Output>;
}

pub struct Native;
impl Recorder for Native {
    type Window = Measurement;
    type Output = Sample;
    fn start(&mut self) -> io::Result<Measurement> {
        Measurement::start()
    }
    fn finish(&mut self, window: Measurement) -> io::Result<Sample> {
        window.finish()
    }
}

fn storage_error(error: impl std::fmt::Debug) -> io::Error {
    io::Error::other(format!("{error:?}"))
}

pub async fn measure<C, R>(
    db: C,
    writes: &[(Digest, Option<Digest>)],
    phase: Phase,
    recorder: &mut R,
) -> io::Result<(C, R::Output, Digest)>
where
    C: DbAny<mmb::Family, Key = Digest, Value = Digest, Digest = Digest>,
    C::Merkleized: MerkleizedBatch<Digest = Digest>,
    R: Recorder,
{
    match phase {
        Phase::Merkleize => {
            let window = recorder.start()?;
            let batch = stage_prepared_writes(db.new_batch(), writes);
            let batch = batch.merkleize(&db, None).await.map_err(storage_error)?;
            let root = black_box(batch.root());
            // Match MerkleizeWorkload::iter(): discard speculative state before returning.
            drop(batch);
            let sample = recorder.finish(window)?;
            Ok((db, sample, root))
        }
        Phase::Apply => {
            let batch = stage_prepared_writes(db.new_batch(), writes);
            let batch = batch.merkleize(&db, None).await.map_err(storage_error)?;
            let expected = batch.root();
            let window = recorder.start()?;
            let (db, _) = db.apply_batch(batch).await.map_err(storage_error)?;
            let sample = recorder.finish(window)?;
            if db.root() != expected {
                return Err(io::Error::other("applied root differs from prepared root"));
            }
            black_box(&db);
            Ok((db, sample, expected))
        }
    }
}
