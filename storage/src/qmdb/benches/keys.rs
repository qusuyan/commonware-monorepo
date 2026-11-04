//! Deterministic keysets with checked persistence and process-local caching.
#![allow(dead_code)]
use super::artifact_io::{self, CreationLock, invalid};
use commonware_cryptography::{DigestOf, Hasher as _, Sha256};
use commonware_utils::sync::Mutex;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;
use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::SystemTime,
};
pub type Digest = DigestOf<Sha256>;
pub const KEY_SIZE: usize = 32;
pub const DEFAULT_KEY_COUNT: usize = 10_000_000;
pub const DEFAULT_KEYS_FILENAME: &str = "keys_10M.bin";
pub const ALGORITHM: &str = "sha256-u64-be-v1";
pub fn checksum(bytes: &[u8]) -> String {
    format!("{:?}", Sha256::hash(&[bytes]))
}
pub fn prefix_digest(keys: &[Digest]) -> String {
    let mut hasher = Sha256::default();
    for key in keys {
        hasher.update(key.as_ref());
    }
    format!("{:?}", hasher.finalize().1)
}
#[derive(Clone, Debug)]
pub struct KeySet {
    keys: Arc<[Digest]>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    algorithm: String,
    count: usize,
    checksum: String,
}
#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: SystemTime,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}
fn stamp(path: &Path) -> io::Result<Stamp> {
    let m = fs::metadata(path)?;
    Ok(Stamp {
        len: m.len(),
        modified: m.modified()?,
        #[cfg(unix)]
        identity: (m.dev(), m.ino(), m.ctime(), m.ctime_nsec()),
    })
}
type Cache = HashMap<PathBuf, (Stamp, Stamp, KeySet)>;
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
impl KeySet {
    pub fn new(keys: Vec<Digest>) -> Self {
        Self { keys: keys.into() }
    }
    pub fn len(&self) -> usize {
        self.keys.len()
    }
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
    pub fn slice(&self, count: usize) -> &[Digest] {
        &self.keys[..count]
    }
    pub fn generate_parallel(count: usize) -> Self {
        Self::new(
            (0..count as u64)
                .into_par_iter()
                .map(|i| Sha256::hash(&[&i.to_be_bytes()]))
                .collect(),
        )
    }
    pub fn sidecar(path: &Path) -> PathBuf {
        let mut p = path.as_os_str().to_os_string();
        p.push(".json");
        p.into()
    }
    pub fn generate_and_save(path: &Path, count: usize) -> io::Result<Self> {
        if count == 0 {
            return Err(invalid("key count must be positive"));
        }
        let lock = CreationLock::acquire(path)?;
        if path.try_exists()? {
            drop(lock);
            let keys = Self::load_from_file(path)?;
            if keys.len() != count {
                return Err(invalid(
                    "existing keyset has a different count; choose a new path",
                ));
            }
            return Ok(keys);
        }
        if Self::sidecar(path).try_exists()? {
            return Err(invalid("orphaned keyset sidecar; choose a new path"));
        }
        let keys = Self::generate_parallel(count);
        let mut raw = Vec::with_capacity(
            count
                .checked_mul(KEY_SIZE)
                .ok_or_else(|| invalid("key count too large"))?,
        );
        for key in keys.keys.iter() {
            raw.extend_from_slice(key.as_ref());
        }
        artifact_io::atomic_write(path, &raw)?;
        Self::publish_sidecar(path, count, &raw)?;
        Ok(keys)
    }
    fn publish_sidecar(path: &Path, count: usize, raw: &[u8]) -> io::Result<()> {
        let manifest = Manifest {
            schema_version: 1,
            algorithm: ALGORITHM.into(),
            count,
            checksum: checksum(raw),
        };
        artifact_io::atomic_write(&Self::sidecar(path), &serde_json::to_vec_pretty(&manifest)?)
    }
    pub fn load_from_file(path: &Path) -> io::Result<Self> {
        let path = fs::canonicalize(path)?;
        let sidecar = Self::sidecar(&path);
        let before = stamp(&path)?;
        let side_before = if sidecar.try_exists()? {
            Some(stamp(&sidecar)?)
        } else {
            None
        };
        let cache = CACHE.get_or_init(Default::default);
        if let Some(side) = &side_before
            && let Some((a, b, keys)) = cache.lock().get(&path)
            && *a == before
            && b == side
        {
            return Ok(keys.clone());
        }
        let raw = fs::read(&path)?;
        if raw.is_empty() || raw.len() % KEY_SIZE != 0 {
            return Err(invalid(
                "keyset must contain a positive multiple of 32 bytes",
            ));
        }
        let count = raw.len() / KEY_SIZE;
        let keys = Self::new(
            raw.chunks_exact(KEY_SIZE)
                .map(|b| Digest::from(<[u8; 32]>::try_from(b).unwrap()))
                .collect(),
        );
        if side_before.is_some() {
            let manifest: Manifest = serde_json::from_slice(&fs::read(&sidecar)?)?;
            if manifest.schema_version != 1
                || manifest.algorithm != ALGORITHM
                || manifest.count != count
                || manifest.checksum != checksum(&raw)
            {
                return Err(invalid("keyset checksum, count, or algorithm mismatch"));
            }
        } else {
            let _lock = CreationLock::acquire(&path)?;
            if !keys
                .keys
                .par_iter()
                .enumerate()
                .all(|(i, k)| *k == Sha256::hash(&[&(i as u64).to_be_bytes()]))
            {
                return Err(invalid(
                    "keyset is not the canonical SHA256(index) sequence",
                ));
            }
            Self::publish_sidecar(&path, count, &raw)?;
        }
        if stamp(&path)? != before || (side_before.is_some() && stamp(&sidecar).ok() != side_before)
        {
            return Err(invalid("keyset changed during validation"));
        }
        cache
            .lock()
            .insert(path, (before, stamp(&sidecar)?, keys.clone()));
        Ok(keys)
    }
    pub fn default_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("measurements-rain/data")
            .join(DEFAULT_KEYS_FILENAME)
    }
    pub fn load_or_generate(path: &Path, count: usize) -> io::Result<Self> {
        let keys = if path.try_exists()? {
            Self::load_from_file(path)?
        } else {
            Self::generate_and_save(path, count)?
        };
        if keys.len() < count {
            return Err(invalid(format!(
                "keyset contains {} keys; need {count}; generate a larger file at a new path",
                keys.len()
            )));
        }
        Ok(keys)
    }
}
