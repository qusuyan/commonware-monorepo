//! Native QMDB CPU accounting. This executable does not use Criterion or Callgrind.
#![allow(dead_code, unused_imports, unused_macros)]
mod common;
mod cpu_workload;

use common::{
    AnyUFixBTreeDb, CurOFixDb, WriteSampler, WriteType,
    merkleize_config::Descriptor,
    parse_write_skew, prepare_sampled_writes,
    snapshots::{self, ReuseMode, Variant},
};
use commonware_bench::cpu::Measurement;
use commonware_runtime::{Runner as _, Supervisor as _, tokio};
use commonware_storage::merkle::mmb;
use commonware_utils::TestRng;
use cpu_workload::{Native, Phase};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    env,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::PathBuf,
};

const HELP: &str = "qmdb_cpu --phase merkleize|apply --variant any_unordered_btree|current_ordered --keys N --updates N --output DIR [--samples 10] [--write-type overwrite|insert|delete|mixed] [--skew 0]\nOutput directory must not exist. QMDB_SNAPSHOT_REUSE defaults to required. One independent batch per sample; caches retained; no commit or sync.";

struct Options {
    phase: Phase,
    variant: Variant,
    keys: u64,
    updates: u64,
    samples: u64,
    write_type: WriteType,
    skew: f64,
    output: PathBuf,
    reuse: ReuseMode,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn parse(args: &[String]) -> io::Result<Options> {
    let mut values = BTreeMap::new();
    let allowed = [
        "--phase",
        "--variant",
        "--keys",
        "--updates",
        "--output",
        "--samples",
        "--write-type",
        "--skew",
    ];
    let (pairs, remainder) = args.as_chunks::<2>();
    if !remainder.is_empty() {
        return Err(invalid("each option requires a value"));
    }
    for pair in pairs {
        if !allowed.contains(&pair[0].as_str())
            || pair[1].is_empty()
            || values.insert(pair[0].as_str(), pair[1].as_str()).is_some()
        {
            return Err(invalid(format!("unknown or duplicate option: {}", pair[0])));
        }
    }
    let required = |name| {
        values
            .get(name)
            .copied()
            .ok_or_else(|| invalid(format!("{name} is required")))
    };
    let positive = |value: &str| -> io::Result<u64> {
        value
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| invalid("counts must be positive u64 values"))
    };
    let phase = match required("--phase")? {
        "merkleize" => Phase::Merkleize,
        "apply" => Phase::Apply,
        _ => return Err(invalid("invalid phase")),
    };
    let variant = Variant::parse(required("--variant")?)?;
    let keys = positive(required("--keys")?)?;
    let updates = positive(required("--updates")?)?;
    // Input generation uses 0..2*keys and allocates one prepared-write vector.
    if keys > u64::MAX / 2
        || keys > (isize::MAX as u64) / std::mem::size_of::<common::Digest>() as u64
        || updates
            > (isize::MAX as u64)
                / std::mem::size_of::<(common::Digest, Option<common::Digest>)>() as u64
    {
        return Err(invalid(
            "key or update count exceeds addressable workload limits",
        ));
    }
    let write_type = match values.get("--write-type").copied().unwrap_or("overwrite") {
        "overwrite" => WriteType::Overwrite,
        "insert" => WriteType::Insert,
        "delete" => WriteType::Delete,
        "mixed" => WriteType::Mixed,
        _ => return Err(invalid("invalid write type")),
    };
    Ok(Options {
        phase,
        variant,
        keys,
        updates,
        write_type,
        samples: positive(values.get("--samples").copied().unwrap_or("10"))?,
        skew: parse_write_skew(values.get("--skew").copied().unwrap_or("0")).map_err(invalid)?,
        output: PathBuf::from(required("--output")?),
        reuse: ReuseMode::parse(env::var("QMDB_SNAPSHOT_REUSE").ok().as_deref())?,
    })
}

fn write_json(path: PathBuf, value: &Value) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    writeln!(file)
}

fn statistics(mut values: Vec<f64>) -> Value {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    let mean = values.iter().sum::<f64>() / n as f64;
    let median = if n.is_multiple_of(2) {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    } else {
        values[n / 2]
    };
    let stddev = if n > 1 {
        Some((values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1) as f64).sqrt())
    } else {
        None
    };
    json!({"mean":mean, "median":median, "sample_standard_deviation":stddev})
}

fn run(options: Options) -> io::Result<()> {
    // Check support before creating output or preparing a potentially large database.
    Measurement::start()?.finish()?;
    fs::create_dir(&options.output)?;
    let descriptor = Descriptor::production();
    let config = json!({
        "source":"libc_getrusage", "phase":options.phase.name(), "variant":options.variant.name(),
        "keys":options.keys, "submitted_write_attempts":options.updates, "samples":options.samples,
        "write_type":options.write_type.name(), "skew":options.skew, "rng_seed":99,
        "descriptor":descriptor, "snapshot_reuse":options.reuse.name(),
        "iteration_policy":"independent_snapshot_fixed_inputs", "measured_batches":1,
        "commonware_page_cache":"retained_after_preparation", "os_page_cache":"not_cleared",
        "process_cpu_monitoring":false, "process_memory_monitoring":true,
        "tokio_worker_threads":2, "durability":"buffered", "commit_sync_measured":false,
        "counter_scope":"current_process_all_threads_excluding_children",
        "boundary_order":"rusage_start, instant_start, workload, instant_end, rusage_end",
        "duration_unit":"nanoseconds; OS accounting resolution is coarser"
    });
    write_json(options.output.join("configuration.json"), &config)?;
    let mut raw = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(options.output.join("samples.jsonl"))?;
    let mut overhead = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(options.output.join("overhead.jsonl"))?;
    let sampler = WriteSampler::new(options.keys, options.skew).map_err(invalid)?;
    let mut writes = Vec::new();
    writes
        .try_reserve_exact(options.updates as usize)
        .map_err(io::Error::other)?;
    prepare_sampled_writes(
        &mut writes,
        options.updates,
        &mut TestRng::new(99),
        options.write_type,
        &sampler,
    );
    let mut rows = Vec::new();
    let mut overhead_rows = Vec::new();
    for index in 0..options.samples {
        let prepared = snapshots::prepare(options.variant, options.keys, options.reuse)?;
        let provenance = prepared.provenance("qmdb::cpu");
        let manifest = &prepared.manifest;
        let descriptor = &descriptor;
        let writes = &writes;
        let (sample, root, empty) = tokio::Runner::new(
            tokio::Config::new()
                .with_storage_directory(prepared.scratch.path())
                .with_process_cpu_monitoring(false),
        )
        .start(|ctx| async move {
            let pc = descriptor.cache(&ctx);
            macro_rules! measure {
                ($db:expr) => {{
                    let db = $db.map_err(|e| io::Error::other(format!("{e:?}")))?;
                    snapshots::check_database(&db, manifest)?;
                    let empty = Measurement::start()?.finish()?;
                    let (db, sample, root) =
                        cpu_workload::measure(db, writes, options.phase, &mut Native).await?;
                    drop(db);
                    Ok::<_, io::Error>((sample, root, empty))
                }};
            }
            match options.variant {
                Variant::AnyUnorderedBtree => measure!(
                    AnyUFixBTreeDb::<mmb::Family>::init(
                        ctx.child("storage"),
                        descriptor.any(&ctx, pc)
                    )
                    .await
                ),
                Variant::CurrentOrdered => measure!(
                    CurOFixDb::<mmb::Family>::init(
                        ctx.child("storage"),
                        descriptor.current(&ctx, pc)
                    )
                    .await
                ),
            }
        })?;
        let normalized = sample.normalize(1, options.updates)?;
        let mut row = config.clone();
        let object = row.as_object_mut().unwrap();
        object.extend(serde_json::to_value(sample)?.as_object().unwrap().clone());
        object.extend(
            serde_json::to_value(normalized)?
                .as_object()
                .unwrap()
                .clone(),
        );
        object.insert("sample_index".into(), json!(index));
        object.insert("provenance".into(), provenance);
        object.insert("result_root".into(), json!(format!("{root:?}")));
        let precision = if normalized.total_cpu_ns == 0 {
            Some("zero_cpu_delta_below_accounting_resolution")
        } else if normalized.total_cpu_ns
            <= empty
                .user_cpu_ns
                .saturating_add(empty.system_cpu_ns)
                .saturating_mul(10)
        {
            Some("cpu_delta_within_10x_empty_window_overhead")
        } else {
            None
        };
        object.insert("precision_warning".into(), json!(precision));
        serde_json::to_writer(&mut raw, &row)?;
        writeln!(raw)?;
        raw.flush()?;
        serde_json::to_writer(
            &mut overhead,
            &json!({"sample_index":index,"source":"libc_getrusage","sample":empty}),
        )?;
        writeln!(overhead)?;
        rows.push(row);
        overhead_rows.push(serde_json::to_value(empty)?);
        drop(prepared);
    }
    let mut stats = serde_json::Map::new();
    for name in [
        "wall_ns",
        "user_cpu_ns",
        "system_cpu_ns",
        "total_cpu_ns",
        "cpu_ns_per_batch",
        "cpu_ns_per_write",
        "average_cores_used",
        "cpu_usage_percent",
        "voluntary_context_switches",
        "involuntary_context_switches",
        "minor_page_faults",
        "major_page_faults",
    ] {
        stats.insert(
            name.into(),
            statistics(rows.iter().map(|row| row[name].as_f64().unwrap()).collect()),
        );
    }
    let mut overhead_stats = serde_json::Map::new();
    for name in ["wall_ns", "user_cpu_ns", "system_cpu_ns"] {
        overhead_stats.insert(
            name.into(),
            statistics(
                overhead_rows
                    .iter()
                    .map(|r| r[name].as_f64().unwrap())
                    .collect(),
            ),
        );
    }
    write_json(
        options.output.join("summary.json"),
        &json!({
            "configuration":config, "sample_count":rows.len(), "statistics":stats,
            "precision_warning_count":rows.iter().filter(|r| !r["precision_warning"].is_null()).count(),
            "empty_window_statistics":overhead_stats, "overhead_subtracted":false,
            "standard_deviation":"sample; unavailable for a single sample"
        }),
    )
}

fn main() {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    // Cargo appends this marker even for harness-free benchmark executables.
    if args.last().is_some_and(|arg| arg == "--bench") {
        args.truncate(args.len() - 1);
    }
    if args.is_empty() || args == ["--help"] || args == ["-h"] {
        println!("{HELP}");
        return;
    }
    if let Err(error) = parse(&args).and_then(run) {
        eprintln!("{error}\n{HELP}");
        std::process::exit(1);
    }
}
