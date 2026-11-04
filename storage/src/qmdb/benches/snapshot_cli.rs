//! Create and inspect validated keysets and immutable Merkleization snapshots.
#![allow(unused_macros, unused_imports, dead_code)]
mod common;
use common::{
    KeySet,
    artifact_io::invalid,
    merkleize_config::Descriptor,
    snapshots::{self, Variant},
};
use std::{collections::BTreeMap, io, path::PathBuf};
fn usage() {
    eprintln!(
        "qmdb_snapshot generate-keys [--count N] [--out FILE]\nqmdb_snapshot seed-db --variant any_unordered_btree|current_ordered [--count N] [--keys-file FILE] [--out DIR]\nqmdb_snapshot inspect-db --db-path DIR [--variant V]\nqmdb_snapshot exercise-db --db-path DIR [--updates N]\nDefaults: 10M generated keys, 4M seed keys. Compatible artifacts are reused; existing incompatible destinations are never replaced.\nSynced-seed benchmarks require these snapshots by default. QMDB_SNAPSHOT_REUSE=auto allows missing-snapshot fallback; off disables reuse."
    );
}
fn parse(args: &[String], allowed: &[&str]) -> io::Result<BTreeMap<String, String>> {
    let mut values = BTreeMap::new();
    let mut i = 0;
    while i < args.len() {
        let (flag, value) = if let Some((k, v)) = args[i].split_once('=') {
            (k.to_string(), v.to_string())
        } else {
            let key = args[i].clone();
            i += 1;
            (
                key,
                args.get(i)
                    .ok_or_else(|| invalid("missing option value"))?
                    .clone(),
            )
        };
        if !allowed.contains(&flag.as_str())
            || value.is_empty()
            || value.starts_with("--")
            || values.insert(flag.clone(), value).is_some()
        {
            return Err(invalid(format!("invalid or duplicate option: {flag}")));
        }
        i += 1;
    }
    Ok(values)
}
fn count(args: &BTreeMap<String, String>, default: usize) -> io::Result<usize> {
    let value = match args.get("--count") {
        Some(s) => s
            .parse()
            .map_err(|_| invalid("--count must be a positive integer"))?,
        None => default,
    };
    if value == 0 || value > usize::MAX / 32 {
        return Err(invalid("--count is out of range"));
    }
    Ok(value)
}
fn required<'a>(args: &'a BTreeMap<String, String>, flag: &str) -> io::Result<&'a str> {
    args.get(flag)
        .map(String::as_str)
        .ok_or_else(|| invalid(format!("{flag} is required")))
}
fn run(args: &[String]) -> io::Result<()> {
    let command = args.first().ok_or_else(|| invalid("command required"))?;
    if ["help", "--help", "-h"].contains(&command.as_str()) {
        usage();
        return Ok(());
    }
    match command.as_str() {
        "generate-keys" => {
            let args = parse(&args[1..], &["--count", "--out"])?;
            let count = count(&args, common::keys::DEFAULT_KEY_COUNT)?;
            let path = args
                .get("--out")
                .map(PathBuf::from)
                .unwrap_or_else(KeySet::default_path);
            let keys = KeySet::generate_and_save(&path, count)?;
            println!(
                "Verified {} canonical keys at {}",
                keys.len(),
                path.display()
            );
        }
        "seed-db" => {
            let args = parse(
                &args[1..],
                &["--count", "--out", "--variant", "--keys-file"],
            )?;
            let variant = Variant::parse(required(&args, "--variant")?)?;
            let count = count(&args, 4_000_000)?;
            let path = args
                .get("--out")
                .map(PathBuf::from)
                .unwrap_or_else(|| variant.default_path(count as u64));
            // Validate an existing destination before creating or importing any key artifact.
            if path.try_exists()? {
                snapshots::validate(&path, variant, count as u64, &Descriptor::production())?;
            }
            let keys = if let Some(path) = args.get("--keys-file") {
                KeySet::load_from_file(&PathBuf::from(path))?
            } else {
                KeySet::load_or_generate(
                    &KeySet::default_path(),
                    common::keys::DEFAULT_KEY_COUNT.max(count),
                )?
            };
            if keys.len() < count {
                return Err(invalid(
                    "selected keyset is too small; generate a larger file at a new path",
                ));
            }
            let manifest =
                snapshots::create(&path, variant, &Descriptor::production(), keys.slice(count))?;
            println!(
                "Verified snapshot at {}\n{}",
                path.display(),
                serde_json::to_string_pretty(&manifest)?
            );
        }
        "exercise-db" => {
            let args = parse(&args[1..], &["--db-path", "--updates"])?;
            let path = PathBuf::from(required(&args, "--db-path")?);
            let updates = args.get("--updates").map_or(Ok(10000), |v| {
                v.parse::<u64>()
                    .map_err(|_| invalid("--updates must be a positive u64"))
            })?;
            println!(
                "{}",
                serde_json::to_string(&snapshots::exercise(&path, updates)?)?
            );
        }
        "inspect-db" => {
            let args = parse(&args[1..], &["--db-path", "--variant"])?;
            let path = PathBuf::from(required(&args, "--db-path")?);
            if let Some(variant) = args.get("--variant") {
                let variant = Variant::parse(variant)?;
                if snapshots::read_manifest(&path)?.variant != variant {
                    return Err(invalid("requested variant differs from snapshot"));
                }
            }
            let manifest = snapshots::inspect(&path)?;
            println!(
                "Verified snapshot: {}",
                serde_json::to_string_pretty(&manifest)?
            );
        }
        _ => return Err(invalid(format!("unknown command: {command}"))),
    }
    Ok(())
}
fn main() {
    if let Err(error) = run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
