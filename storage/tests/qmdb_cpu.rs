#![cfg(feature = "test-traits")]

#[allow(dead_code, unused_imports, unused_macros)]
#[path = "../src/qmdb/benches/common.rs"]
mod common;
#[path = "../src/qmdb/benches/cpu_workload.rs"]
#[allow(dead_code)]
mod cpu_workload;

use common::Digest;
use commonware_cryptography::{Hasher as _, Sha256};
use commonware_runtime::{Runner as _, deterministic};
use commonware_storage::{
    merkle::{Location, mmb},
    qmdb::{
        Error,
        any::traits::{ApplyBatchResult, BatchableDb, DbAny, MerkleizedBatch, UnmerkleizedBatch},
    },
};
use commonware_utils::sync::Mutex;
use cpu_workload::{Phase, Recorder};
use std::{io, ops::Range, sync::Arc};

#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<&'static str>>>);
impl Events {
    fn push(&self, event: &'static str) {
        self.0.lock().push(event);
    }
}
struct Db(Events);
struct Batch(Events);
struct Ready(Events);

impl UnmerkleizedBatch<Db> for Batch {
    type Family = mmb::Family;
    type K = Digest;
    type V = Digest;
    type Metadata = Digest;
    type Merkleized = Ready;
    fn write(self, _: Digest, _: Option<Digest>) -> Self {
        self.0.push("write");
        self
    }
    async fn merkleize(self, _: &Db, _: Option<Digest>) -> Result<Ready, Error<mmb::Family>> {
        self.0.push("merkleize");
        Ok(Ready(self.0))
    }
}
impl MerkleizedBatch for Ready {
    type Digest = Digest;
    fn root(&self) -> Digest {
        self.0.push("batch_root");
        Sha256::hash(&[b"root"])
    }
}
impl Drop for Ready {
    fn drop(&mut self) {
        self.0.push("drop_batch");
    }
}
impl BatchableDb for Db {
    type Family = mmb::Family;
    type K = Digest;
    type V = Digest;
    type Merkleized = Ready;
    type Batch = Batch;
    fn new_batch(&self) -> Batch {
        self.0.push("new_batch");
        Batch(self.0.clone())
    }
    async fn apply_batch(self, batch: Ready) -> ApplyBatchResult<Self> {
        self.0.push("apply");
        drop(batch);
        Ok((self, Location::new(0)..Location::new(0)))
    }
}
impl DbAny<mmb::Family> for Db {
    type Key = Digest;
    type Value = Digest;
    type Digest = Digest;
    async fn get(&self, _: &Digest) -> Result<Option<Digest>, Error<mmb::Family>> {
        unreachable!()
    }
    async fn get_many(&self, _: &[&Digest]) -> Result<Vec<Option<Digest>>, Error<mmb::Family>> {
        unreachable!()
    }
    fn root(&self) -> Digest {
        self.0.push("check_root");
        Sha256::hash(&[b"root"])
    }
    fn bounds(&self) -> Range<Location<mmb::Family>> {
        unreachable!()
    }
    async fn get_metadata(&self) -> Result<Option<Digest>, Error<mmb::Family>> {
        unreachable!()
    }
    async fn prune(self, _: Location<mmb::Family>) -> Result<Self, Error<mmb::Family>> {
        unreachable!()
    }
    async fn start_sync(
        self,
    ) -> Result<(Self, commonware_runtime::Handle<()>), Error<mmb::Family>> {
        unreachable!()
    }
    async fn commit(self) -> Result<Self, Error<mmb::Family>> {
        unreachable!()
    }
    async fn sync(self) -> Result<Self, Error<mmb::Family>> {
        unreachable!()
    }
    async fn destroy(self) -> Result<(), Error<mmb::Family>> {
        unreachable!()
    }
    fn inactivity_floor_loc(&self) -> Location<mmb::Family> {
        unreachable!()
    }
    fn sync_boundary(&self) -> Location<mmb::Family> {
        unreachable!()
    }
}

struct Recording {
    events: Events,
    fail_finish: bool,
}
impl Recorder for Recording {
    type Window = ();
    type Output = ();
    fn start(&mut self) -> io::Result<()> {
        self.events.push("start");
        Ok(())
    }
    fn finish(&mut self, _: ()) -> io::Result<()> {
        self.events.push("finish");
        if self.fail_finish {
            Err(io::Error::other("injected accounting failure"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn boundaries_and_accounting_failure() {
    deterministic::Runner::default().start(|_| async move {
        let root = Sha256::hash(&[b"root"]);
        for (phase, expected) in [
            (
                Phase::Merkleize,
                vec![
                    "setup",
                    "start",
                    "new_batch",
                    "write",
                    "merkleize",
                    "batch_root",
                    "drop_batch",
                    "finish",
                    "serialize",
                    "teardown",
                ],
            ),
            (
                Phase::Apply,
                vec![
                    "setup",
                    "new_batch",
                    "write",
                    "merkleize",
                    "batch_root",
                    "start",
                    "apply",
                    "drop_batch",
                    "finish",
                    "check_root",
                    "serialize",
                    "teardown",
                ],
            ),
        ] {
            let events = Events::default();
            events.push("setup");
            let mut recorder = Recording {
                events: events.clone(),
                fail_finish: false,
            };
            let result = cpu_workload::measure(
                Db(events.clone()),
                &[(root, Some(root))],
                phase,
                &mut recorder,
            )
            .await;
            assert!(result.is_ok());
            events.push("serialize");
            drop(result);
            events.push("teardown");
            assert_eq!(*events.0.lock(), expected);
            let mut recorder = Recording {
                events: events.clone(),
                fail_finish: true,
            };
            assert!(
                cpu_workload::measure(Db(events), &[], phase, &mut recorder)
                    .await
                    .is_err()
            );
        }
    });
}
