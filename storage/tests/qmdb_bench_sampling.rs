#![cfg(feature = "test-traits")]

#[allow(dead_code, unused_imports, unused_macros)]
#[path = "../src/qmdb/benches/common.rs"]
mod common;

use common::{
    Digest, WriteSampler, WriteType, parse_write_skew, write_random_updates, write_random_writes,
    write_sampled_writes,
};
use commonware_cryptography::{Hasher as _, Sha256};
use commonware_storage::{
    merkle::mmr::Family,
    qmdb::{
        Error,
        any::traits::{MerkleizedBatch, UnmerkleizedBatch},
    },
};
use commonware_utils::TestRng;
use rand::Rng as _;
use std::collections::HashSet;

#[derive(Default, Debug, PartialEq)]
struct Writes(Vec<(Digest, Option<Digest>)>);

struct Root;

impl MerkleizedBatch for Root {
    type Digest = Digest;
    fn root(&self) -> Digest {
        unreachable!("sampling tests do not merkleize")
    }
}

impl UnmerkleizedBatch<()> for Writes {
    type Family = Family;
    type K = Digest;
    type V = Digest;
    type Metadata = Digest;
    type Merkleized = Root;

    fn write(mut self, key: Digest, value: Option<Digest>) -> Self {
        self.0.push((key, value));
        self
    }

    async fn merkleize(self, _: &(), _: Option<Digest>) -> Result<Root, Error<Family>> {
        unreachable!("sampling tests do not merkleize")
    }
}

#[test]
fn parses_arbitrary_exponents_and_rejects_invalid_inputs() {
    for (value, expected) in [
        ("0", 0.0),
        ("-0", 0.0),
        ("0.75", 0.75),
        ("3.0", 3.0),
        ("1e-3", 0.001),
    ] {
        assert_eq!(parse_write_skew(value).unwrap(), expected);
    }
    for invalid in ["", "zipf", "-0.5", "NaN", "inf", "-inf", "1e309"] {
        assert!(parse_write_skew(invalid).is_err(), "{invalid}");
    }
    for invalid in [-1.0, f64::NAN, f64::INFINITY] {
        assert!(WriteSampler::new(100, invalid).is_err());
    }
    assert!(WriteSampler::new(0, 0.0).is_err());
}

#[test]
fn zero_preserves_uniform_keys_values_and_rng_consumption() {
    let sampler = WriteSampler::new(100, 0.0).unwrap();
    for write_type in [WriteType::Overwrite, WriteType::Insert, WriteType::Mixed] {
        let mut original_rng = TestRng::new(99);
        let mut sampled_rng = TestRng::new(99);
        let original =
            write_random_writes(Writes::default(), 1_000, 100, &mut original_rng, write_type);
        let sampled = write_sampled_writes(
            Writes::default(),
            1_000,
            &mut sampled_rng,
            write_type,
            &sampler,
        );
        assert_eq!(original, sampled);
        assert_eq!(original_rng.next_u64(), sampled_rng.next_u64());
    }
    let mut original_rng = TestRng::new(99);
    let mut sampled_rng = TestRng::new(99);
    assert_eq!(
        write_random_updates(Writes::default(), 1_000, 100, &mut original_rng),
        write_sampled_writes(
            Writes::default(),
            1_000,
            &mut sampled_rng,
            WriteType::Overwrite,
            &sampler
        ),
    );
    assert_eq!(original_rng.next_u64(), sampled_rng.next_u64());
}

#[test]
fn bounds_reproducibility_and_batch_rng_advancement() {
    for exponent in [0.0, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0] {
        let sampler = WriteSampler::new(100, exponent).unwrap();
        let mut first_rng = TestRng::new(99);
        let mut second_rng = TestRng::new(99);
        let first: Vec<_> = (0..1_000).map(|_| sampler.sample(&mut first_rng)).collect();
        let replay: Vec<_> = (0..1_000)
            .map(|_| sampler.sample(&mut second_rng))
            .collect();
        let next: Vec<_> = (0..1_000).map(|_| sampler.sample(&mut first_rng)).collect();
        assert_eq!(first, replay);
        assert_ne!(first, next);
        assert!(first.iter().chain(&next).all(|&index| index < 100));
        let single = WriteSampler::new(1, exponent).unwrap();
        assert!((0..100).all(|_| single.sample(&mut first_rng) == 0));
    }
}

#[test]
fn skew_concentrates_writes_on_hot_keys() {
    let mut previous_hot_count = 0;
    let mut uniform_distinct = 0;
    for exponent in [0.0, 0.5, 1.0, 1.5, 2.0, 3.0] {
        let sampler = WriteSampler::new(1_000, exponent).unwrap();
        let mut rng = TestRng::new(99);
        let samples: Vec<_> = (0..20_000).map(|_| sampler.sample(&mut rng)).collect();
        let hot_count = samples.iter().filter(|&&index| index < 10).count();
        let distinct = samples.iter().collect::<HashSet<_>>().len();
        if exponent == 0.0 {
            uniform_distinct = distinct;
        } else if exponent >= 1.5 {
            assert!(distinct < uniform_distinct);
        }
        assert!(
            hot_count > previous_hot_count,
            "exponent={exponent}, hot_count={hot_count}"
        );
        previous_hot_count = hot_count;
    }
}

#[test]
fn sampled_writes_respect_write_type_ranges_and_call_count() {
    let keys = |offset: u64| {
        (offset..offset + 32u64)
            .map(|index| Sha256::hash(&[&index.to_be_bytes()]))
            .collect::<Vec<_>>()
    };
    let seeded = keys(0);
    let new = keys(32);
    for exponent in [0.0, 0.75, 3.0] {
        let sampler = WriteSampler::new(32, exponent).unwrap();
        for write_type in [WriteType::Overwrite, WriteType::Insert, WriteType::Mixed] {
            let mut rng = TestRng::new(99);
            let writes =
                write_sampled_writes(Writes::default(), 1_000, &mut rng, write_type, &sampler);
            assert_eq!(writes.0.len(), 1_000);
            let mut overwrites = 0;
            let mut inserts = 0;
            for (key, value) in writes.0 {
                assert!(value.is_some());
                if seeded.contains(&key) {
                    overwrites += 1;
                } else if new.contains(&key) {
                    inserts += 1;
                } else {
                    panic!("key outside eligible ranges");
                }
            }
            match write_type {
                WriteType::Overwrite => assert_eq!(overwrites, 1_000),
                WriteType::Insert => assert_eq!(inserts, 1_000),
                WriteType::Mixed => assert!(overwrites > 0 && inserts > 0),
            }
        }
    }
}
