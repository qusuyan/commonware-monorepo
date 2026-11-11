//! Validated, immutable Merkleization seeds and isolated benchmark preparation.
#![allow(dead_code)]
use super::{
    AnyUFixBTreeDb, CurOFixDb, Digest, KeySet, ScratchDir,
    artifact_io::{self, CreationLock, invalid},
    keys, make_fixed_value,
    merkleize_config::Descriptor,
};
use commonware_cryptography::{Hasher as _, Sha256};
use commonware_runtime::{Runner as _, Supervisor as _, tokio};
use commonware_storage::{
    merkle::mmb,
    qmdb::any::traits::{DbAny, MerkleizedBatch as _, UnmerkleizedBatch},
};
use commonware_utils::TestRng;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};
pub const MANIFEST: &str = "snapshot_manifest.json";
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Variant {
    AnyUnorderedBtree,
    CurrentOrdered,
}
impl Variant {
    pub fn parse(value: &str) -> io::Result<Self> {
        match value {
            "any_unordered_btree" | "any::unordered_btree::fixed::mmb" => {
                Ok(Self::AnyUnorderedBtree)
            }
            "current_ordered" | "current::ordered::fixed::mmb chunk=32" => Ok(Self::CurrentOrdered),
            _ => Err(invalid(format!("unsupported snapshot variant: {value}"))),
        }
    }
    pub const fn name(self) -> &'static str {
        match self {
            Self::AnyUnorderedBtree => "any_unordered_btree",
            Self::CurrentOrdered => "current_ordered",
        }
    }
    pub fn default_path(self, count: u64) -> PathBuf {
        let suffix = match self {
            Self::AnyUnorderedBtree => "any_unordered_btree_fixed_mmb",
            Self::CurrentOrdered => "current_ordered_fixed_mmb_chunk32",
        };
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("measurements-rain/storage-seeds")
            .join(format!("k{count}_{suffix}"))
    }
    pub fn preparation_command(self, count: u64) -> String {
        format!(
            "measurements-rain/scripts/seed-db.sh --variant {} --count {count}",
            self.name()
        )
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReuseMode {
    Required,
    Auto,
    Off,
}
impl ReuseMode {
    pub fn parse(value: Option<&str>) -> io::Result<Self> {
        match value {
            None | Some("required") => Ok(Self::Required),
            Some("auto" | "1") => Ok(Self::Auto),
            Some("off" | "0") => Ok(Self::Off),
            _ => Err(invalid(
                "QMDB_SNAPSHOT_REUSE must be required, auto, off, 1, or 0",
            )),
        }
    }
    pub const fn name(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Auto => "auto",
            Self::Off => "off",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    pub bytes: u64,
    pub checksum: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub variant: Variant,
    pub key_count: u64,
    pub key_generation: String,
    pub keyset_prefix_digest: String,
    pub value_generation: String,
    pub rng_seed: u64,
    pub configuration: Descriptor,
    pub configuration_fingerprint: String,
    pub merkle_root: String,
    pub operation_start: u64,
    pub operation_end: u64,
    pub files: BTreeMap<String, FileEntry>,
}
fn storage_error(error: impl std::fmt::Debug) -> io::Error {
    invalid(format!("database operation failed: {error:?}"))
}
fn file_checksum(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut h = Sha256::default();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        h.update(&buffer[..n]);
    }
    Ok(format!("{:?}", h.finalize().1))
}
pub fn inventory(root: &Path) -> io::Result<BTreeMap<String, FileEntry>> {
    fn visit(root: &Path, dir: &Path, output: &mut BTreeMap<String, FileEntry>) -> io::Result<()> {
        for item in fs::read_dir(dir)? {
            let item = item?;
            let path = item.path();
            let ty = item.file_type()?;
            if ty.is_dir() {
                visit(root, &path, output)?;
            } else if ty.is_file() {
                let name = path
                    .strip_prefix(root)
                    .map_err(storage_error)?
                    .to_str()
                    .ok_or_else(|| invalid("non UTF-8 snapshot path"))?
                    .to_string();
                if name == MANIFEST {
                    continue;
                }
                output.insert(
                    name,
                    FileEntry {
                        bytes: item.metadata()?.len(),
                        checksum: file_checksum(&path)?,
                    },
                );
            } else {
                return Err(invalid("snapshot contains a symlink or special file"));
            }
        }
        Ok(())
    }
    if fs::symlink_metadata(root)?.file_type().is_symlink() {
        return Err(invalid("snapshot directory must not be a symlink"));
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result)?;
    Ok(result)
}
pub fn read_manifest(path: &Path) -> io::Result<Manifest> {
    Ok(serde_json::from_slice(&fs::read(path.join(MANIFEST))?)?)
}
fn digest_valid(d: &str) -> bool {
    d.len() == 64 && d.bytes().all(|b| b.is_ascii_hexdigit())
}
pub fn validate(
    path: &Path,
    variant: Variant,
    count: u64,
    descriptor: &Descriptor,
) -> io::Result<Manifest> {
    descriptor.validate()?;
    let manifest = read_manifest(path)?;
    if manifest.schema_version != 2
        || manifest.variant != variant
        || manifest.key_count != count
        || count == 0
        || manifest.key_generation != keys::ALGORITHM
        || manifest.value_generation != "sha256-test-rng-next-u32-be-v1"
        || manifest.rng_seed != 42
        || manifest.configuration != *descriptor
        || manifest.configuration_fingerprint != descriptor.fingerprint()
        || !digest_valid(&manifest.merkle_root)
        || !digest_valid(&manifest.keyset_prefix_digest)
        || manifest.operation_start >= manifest.operation_end
        || manifest.operation_end <= count
        || manifest.files.is_empty()
    {
        return Err(invalid(
            "snapshot identity/configuration mismatch; recreate at a new destination",
        ));
    }
    let actual = inventory(path)?;
    if manifest.files != actual {
        return Err(invalid("snapshot storage inventory/checksum mismatch"));
    }
    // A valid inventory must include each configured partition, not just arbitrary files.
    let p = &descriptor.storage.partition;
    let mut required = vec![
        format!("journal-{p}-blobs"),
        format!("journal-{p}-metadata"),
        format!("metadata-{p}"),
        format!("log-journal-{p}-blobs"),
        format!("log-journal-{p}-metadata"),
    ];
    if variant == Variant::CurrentOrdered {
        required.push(format!("grafted-metadata-{p}"));
    }
    for partition in required {
        if !actual
            .keys()
            .any(|f| f.starts_with(&format!("{partition}/")))
        {
            return Err(invalid(format!("missing storage partition {partition}")));
        }
    }
    Ok(manifest)
}
pub fn check_database<
    F: commonware_storage::merkle::Family,
    C: DbAny<F, Key = Digest, Value = Digest, Digest = Digest>,
>(
    db: &C,
    manifest: &Manifest,
) -> io::Result<()> {
    let bounds = db.bounds();
    if format!("{:?}", db.root()) != manifest.merkle_root
        || bounds.start.as_u64() != manifest.operation_start
        || bounds.end.as_u64() != manifest.operation_end
    {
        return Err(invalid(
            "reopened snapshot root or operation bounds mismatch",
        ));
    }
    Ok(())
}
async fn seed<C: DbAny<mmb::Family, Key = Digest, Value = Digest, Digest = Digest>>(
    db: C,
    keys: &[Digest],
) -> io::Result<C> {
    let mut rng = TestRng::new(42);
    let mut batch = db.new_batch();
    for &key in keys {
        batch = batch.write(key, Some(make_fixed_value(&mut rng)));
    }
    let batch = batch.merkleize(&db, None).await.map_err(storage_error)?;
    let (db, _) = db.apply_batch(batch).await.map_err(storage_error)?;
    db.commit()
        .await
        .map_err(storage_error)?
        .sync()
        .await
        .map_err(storage_error)
}
fn database_identity<C: DbAny<mmb::Family, Key = Digest, Value = Digest, Digest = Digest>>(
    db: &C,
) -> (String, u64, u64) {
    let b = db.bounds();
    (format!("{:?}", db.root()), b.start.as_u64(), b.end.as_u64())
}
async fn verify_keys<C: DbAny<mmb::Family, Key = Digest, Value = Digest, Digest = Digest>>(
    db: &C,
    keys: &[Digest],
) -> io::Result<()> {
    let mut rng = TestRng::new(42);
    for key in keys {
        if db.get(key).await.map_err(storage_error)? != Some(make_fixed_value(&mut rng)) {
            return Err(invalid("saved seed key/value mismatch"));
        }
    }
    Ok(())
}
fn populate(
    path: &Path,
    variant: Variant,
    descriptor: &Descriptor,
    keys: &[Digest],
) -> io::Result<(String, u64, u64)> {
    tokio::Runner::new(tokio::Config::new().with_storage_directory(path)).start(|ctx| async move {
        let pc = descriptor.cache(&ctx);
        match variant {
            Variant::AnyUnorderedBtree => {
                let db = AnyUFixBTreeDb::<mmb::Family>::init(
                    ctx.child("storage"),
                    descriptor.any(&ctx, pc),
                )
                .await
                .map_err(storage_error)?;
                let db = seed(db, keys).await?;
                Ok(database_identity(&db))
            }
            Variant::CurrentOrdered => {
                let db = CurOFixDb::<mmb::Family>::init(
                    ctx.child("storage"),
                    descriptor.current(&ctx, pc),
                )
                .await
                .map_err(storage_error)?;
                let db = seed(db, keys).await?;
                Ok(database_identity(&db))
            }
        }
    })
}
pub fn verify_open(path: &Path, manifest: &Manifest, keys: Option<&[Digest]>) -> io::Result<()> {
    let descriptor = &manifest.configuration;
    tokio::Runner::new(tokio::Config::new().with_storage_directory(path)).start(|ctx| async move {
        let pc = descriptor.cache(&ctx);
        match manifest.variant {
            Variant::AnyUnorderedBtree => {
                let db = AnyUFixBTreeDb::<mmb::Family>::init(
                    ctx.child("storage"),
                    descriptor.any(&ctx, pc),
                )
                .await
                .map_err(storage_error)?;
                check_database(&db, manifest)?;
                if let Some(keys) = keys {
                    verify_keys(&db, keys).await?;
                }
            }
            Variant::CurrentOrdered => {
                let db = CurOFixDb::<mmb::Family>::init(
                    ctx.child("storage"),
                    descriptor.current(&ctx, pc),
                )
                .await
                .map_err(storage_error)?;
                check_database(&db, manifest)?;
                if let Some(keys) = keys {
                    verify_keys(&db, keys).await?;
                }
            }
        }
        Ok(())
    })
}
fn initialize(
    path: &Path,
    variant: Variant,
    descriptor: &Descriptor,
    keys: &[Digest],
) -> io::Result<Manifest> {
    descriptor.validate()?;
    let (merkle_root, operation_start, operation_end) = populate(path, variant, descriptor, keys)?;
    let mut manifest = Manifest {
        schema_version: 2,
        variant,
        key_count: keys.len() as u64,
        key_generation: keys::ALGORITHM.into(),
        keyset_prefix_digest: keys::prefix_digest(keys),
        value_generation: "sha256-test-rng-next-u32-be-v1".into(),
        rng_seed: 42,
        configuration: descriptor.clone(),
        configuration_fingerprint: descriptor.fingerprint(),
        merkle_root,
        operation_start,
        operation_end,
        files: BTreeMap::new(),
    };
    verify_open(path, &manifest, Some(keys))?;
    manifest.files = inventory(path)?;
    artifact_io::atomic_write(&path.join(MANIFEST), &serde_json::to_vec_pretty(&manifest)?)?;
    Ok(manifest)
}
pub fn create(
    path: &Path,
    variant: Variant,
    descriptor: &Descriptor,
    keys: &[Digest],
) -> io::Result<Manifest> {
    if keys.is_empty() {
        return Err(invalid("seed key count must be positive"));
    }
    descriptor.validate()?;
    let _lock = CreationLock::acquire(path)?;
    if path.exists() {
        let manifest = validate(path, variant, keys.len() as u64, descriptor)?;
        if manifest.keyset_prefix_digest != keys::prefix_digest(keys) {
            return Err(invalid("snapshot keyset prefix mismatch"));
        }
        let scratch = ScratchDir::new_from_clone(path, artifact_io::parent(path))?;
        verify_open(scratch.path(), &manifest, Some(keys))?;
        return Ok(manifest);
    }
    let stage = ScratchDir::empty(artifact_io::parent(path))?;
    let manifest = initialize(stage.path(), variant, descriptor, keys)?;
    fs::rename(stage.path(), path)?;
    artifact_io::sync_dir(artifact_io::parent(path))?;
    Ok(manifest)
}
pub fn inspect(path: &Path) -> io::Result<Manifest> {
    let metadata = read_manifest(path)?;
    let manifest = validate(
        path,
        metadata.variant,
        metadata.key_count,
        &Descriptor::production(),
    )?;
    let scratch = ScratchDir::new_from_clone(path, &std::env::temp_dir())?;
    verify_open(scratch.path(), &manifest, None)?;
    Ok(manifest)
}
pub struct Prepared {
    pub scratch: ScratchDir,
    pub manifest: Manifest,
    pub source: &'static str,
    pub mode: ReuseMode,
    pub identity: Option<PathBuf>,
}
impl Prepared {
    pub fn provenance(&self, benchmark: &str) -> serde_json::Value {
        serde_json::json!({"qmdb_setup": true, "benchmark": benchmark, "reuse_mode": self.mode.name(), "source": self.source, "snapshot": self.identity, "configuration_fingerprint": self.manifest.configuration_fingerprint, "initial_root": self.manifest.merkle_root, "keyset_prefix_digest": self.manifest.keyset_prefix_digest, "clone_method": self.scratch.report.method, "clone_seconds": self.scratch.report.elapsed.as_secs_f64()})
    }
}
pub fn prepare(variant: Variant, count: u64, mode: ReuseMode) -> io::Result<Prepared> {
    prepare_at(variant, count, mode, &variant.default_path(count))
}
pub fn prepare_scratch(variant: Variant, count: u64, mode: ReuseMode) -> io::Result<Prepared> {
    prepare_scratch_at(variant, count, mode, &variant.default_path(count))
}
pub fn prepare_scratch_at(
    variant: Variant,
    count: u64,
    mode: ReuseMode,
    master: &Path,
) -> io::Result<Prepared> {
    let descriptor = Descriptor::production();
    if mode != ReuseMode::Off && master.try_exists()? {
        let manifest = validate(master, variant, count, &descriptor)?;
        let scratch = ScratchDir::new_from_clone(master, &std::env::temp_dir())?;
        // Benchmark callers open and validate bounds immediately, so verify_open is redundant.
        return Ok(Prepared {
            scratch,
            manifest,
            source: "reused",
            mode,
            identity: Some(master.to_path_buf()),
        });
    }
    prepare_at(variant, count, mode, master)
}
pub fn prepare_at(
    variant: Variant,
    count: u64,
    mode: ReuseMode,
    master: &Path,
) -> io::Result<Prepared> {
    let descriptor = Descriptor::production();
    if mode != ReuseMode::Off && master.try_exists()? {
        let manifest = validate(master, variant, count, &descriptor)?;
        let scratch = ScratchDir::new_from_clone(master, &std::env::temp_dir())?;
        verify_open(scratch.path(), &manifest, None)?;
        return Ok(Prepared {
            scratch,
            manifest,
            source: "reused",
            mode,
            identity: Some(master.to_path_buf()),
        });
    }
    if mode == ReuseMode::Required {
        return Err(invalid(format!(
            "missing saved database; prepare with: {}",
            variant.preparation_command(count)
        )));
    }
    let count = usize::try_from(count).map_err(storage_error)?;
    let path = KeySet::default_path();
    let keyset = if path.try_exists()? {
        let k = KeySet::load_from_file(&path)?;
        if k.len() < count {
            return Err(invalid("saved keyset is too small"));
        }
        k
    } else {
        KeySet::generate_parallel(count)
    };
    let scratch = ScratchDir::empty(&std::env::temp_dir())?;
    let manifest = initialize(scratch.path(), variant, &descriptor, keyset.slice(count))?;
    Ok(Prepared {
        scratch,
        manifest,
        source: "fresh",
        mode,
        identity: None,
    })
}

/// Exercise deterministic speculative workloads on a scratch copy of a saved database.
pub fn exercise(path: &Path, updates: u64) -> io::Result<serde_json::Value> {
    if updates == 0 {
        return Err(invalid("updates must be positive"));
    }
    let metadata = read_manifest(path)?;
    let manifest = validate(
        path,
        metadata.variant,
        metadata.key_count,
        &Descriptor::production(),
    )?;
    let scratch = ScratchDir::new_from_clone(path, &std::env::temp_dir())?;
    async fn batches<C: DbAny<mmb::Family, Key = Digest, Value = Digest, Digest = Digest>>(
        db: C,
        manifest: &Manifest,
        updates: u64,
    ) -> io::Result<Vec<serde_json::Value>> {
        check_database(&db, manifest)?;
        let mut results = Vec::new();
        for (write_type, skew) in [
            (super::WriteType::Overwrite, 0.0),
            (super::WriteType::Insert, 0.0),
            (super::WriteType::Mixed, 0.0),
            (super::WriteType::Overwrite, 2.0),
        ] {
            let mut rng = TestRng::new(99);
            let sampler = super::WriteSampler::new(manifest.key_count, skew).map_err(invalid)?;
            let mut writes = Vec::new();
            super::prepare_sampled_writes(&mut writes, updates, &mut rng, write_type, &sampler);
            let batch = super::stage_prepared_writes(db.new_batch(), &writes);
            let batch = batch.merkleize(&db, None).await.map_err(storage_error)?;
            results.push(serde_json::json!({"write_type":write_type.name(), "skew":skew, "updates":updates, "root":format!("{:?}", batch.root())}));
        }
        check_database(&db, manifest)?;
        Ok(results)
    }
    let descriptor = &manifest.configuration;
    let expected = &manifest;
    let roots = tokio::Runner::new(tokio::Config::new().with_storage_directory(scratch.path()))
        .start(|ctx| async move {
            let pc = descriptor.cache(&ctx);
            match expected.variant {
                Variant::AnyUnorderedBtree => {
                    let db = AnyUFixBTreeDb::<mmb::Family>::init(
                        ctx.child("storage"),
                        descriptor.any(&ctx, pc),
                    )
                    .await
                    .map_err(storage_error)?;
                    batches(db, expected, updates).await
                }
                Variant::CurrentOrdered => {
                    let db = CurOFixDb::<mmb::Family>::init(
                        ctx.child("storage"),
                        descriptor.current(&ctx, pc),
                    )
                    .await
                    .map_err(storage_error)?;
                    batches(db, expected, updates).await
                }
            }
        })?;
    Ok(
        serde_json::json!({"variant":manifest.variant, "key_count":manifest.key_count, "initial_root":manifest.merkle_root, "configuration_fingerprint":manifest.configuration_fingerprint, "batches":roots}),
    )
}
