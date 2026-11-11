# Shared configuration selection; sourced by experiment runners.
experiment_configs_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

experiment_modes() {
    case $(( ($1 - 1) % 3 )) in
        0) modes=(any_unordered current_unordered current_ordered) ;;
        1) modes=(current_unordered current_ordered any_unordered) ;;
        2) modes=(current_ordered any_unordered current_unordered) ;;
    esac
}

# Controlled pair: both variants use the ordered B-Tree index implementation.
experiment_modes_pairwise() {
    case $(( ($1 - 1) % 2 )) in
        0) modes=(any_unordered_btree current_ordered) ;;
        1) modes=(current_ordered any_unordered_btree) ;;
    esac
}

experiment_variant() {
    case "$1" in
        any_unordered) variant="any::unordered::fixed::mmb" ;;
        any_unordered_btree) variant="any::unordered_btree::fixed::mmb" ;;
        current_unordered) variant="current::unordered::fixed::mmb chunk=32" ;;
        current_unordered_btree) variant="current::unordered_btree::fixed::mmb chunk=32" ;;
        current_ordered) variant="current::ordered::fixed::mmb chunk=32" ;;
        *) echo "unknown configuration: $1" >&2; return 1 ;;
    esac
}

# Explicit phase selection keeps the timing boundary separate from mode selection.
experiment_study_init() {
    export QMDB_STUDY_OPERATION="${QMDB_STUDY_OPERATION:-merkleize}"
    case "$QMDB_STUDY_OPERATION" in
        merkleize)
            benchmark_operation=merkleize
            study_prefix=""
            criterion_study_flags=()
            ;;
        apply)
            benchmark_operation=apply_study
            study_prefix="apply/"
            criterion_study_flags=(--warm-up-time 0.05 --measurement-time 0.05 --sample-size 10)
            ;;
        *) echo "QMDB_STUDY_OPERATION must be merkleize or apply" >&2; return 2 ;;
    esac
}

experiment_study_modes() {
    experiment_modes_pairwise "$1"
}

# Fail before a multi-step required-reuse study if one of its masters is absent.
# Snapshot contents are still fully validated by the Rust benchmark when opened.
experiment_preflight_snapshots() {
    local reuse_mode="${QMDB_SNAPSHOT_REUSE:-required}"
    case "$reuse_mode" in
        required) ;;
        auto|1|off|0) return 0 ;;
        *) echo "QMDB_SNAPSHOT_REUSE must be required, auto, off, 1, or 0" >&2; return 2 ;;
    esac

    local measurements_root="$experiment_configs_root" missing=false count suffix variant manifest
    for count in "$@"; do
        for variant in any_unordered_btree current_ordered; do
            case "$variant" in
                any_unordered_btree) suffix=any_unordered_btree_fixed_mmb ;;
                current_ordered) suffix=current_ordered_fixed_mmb_chunk32 ;;
            esac
            manifest="$measurements_root/storage-seeds/k${count}_${suffix}/snapshot_manifest.json"
            if [ ! -f "$manifest" ]; then
                echo "missing required snapshot: $manifest" >&2
                echo "prepare it with: measurements-rain/scripts/seed-db.sh --variant $variant --count $count" >&2
                missing=true
            fi
        done
    done
    [ "$missing" = false ]
}

# Serialize runners that share a Cargo target and Criterion output directory.
# The owner PID lets a later invocation distinguish a live runner from a stale
# marker left by an ungraceful termination.
experiment_acquire_lock() {
    local lock_dir="$1" label="$2" owner pid now modified age
    if mkdir "$lock_dir" 2>/dev/null; then
        printf '%s\n%s\n' "$$" "$label" > "$lock_dir/owner"
        return 0
    fi

    owner="$lock_dir/owner"
    if [ -f "$owner" ]; then
        IFS= read -r pid < "$owner" || true
        if [[ "$pid" =~ ^[1-9][0-9]*$ ]] && kill -0 "$pid" 2>/dev/null; then
            echo "$label is already running (PID $pid; lock $lock_dir)" >&2
            return 1
        fi
        rm -f -- "$owner"
        if rmdir "$lock_dir" 2>/dev/null; then
            echo "reclaimed stale $label lock from PID ${pid:-unknown}: $lock_dir" >&2
            experiment_acquire_lock "$lock_dir" "$label"
            return
        fi
        echo "stale $label lock is not empty; inspect $lock_dir" >&2
        return 1
    fi

    # Compatibility with the former empty-directory lock: only reclaim it
    # after a grace period so we cannot race its creator before `owner` exists.
    now="$(date +%s)"
    modified="$(stat -f %m "$lock_dir" 2>/dev/null || echo "$now")"
    age=$((now - modified))
    if [ "$age" -lt 60 ]; then
        echo "$label lock has no owner and is too new to reclaim: $lock_dir" >&2
        return 1
    fi
    if rmdir "$lock_dir" 2>/dev/null; then
        echo "reclaimed stale unowned $label lock: $lock_dir" >&2
        experiment_acquire_lock "$lock_dir" "$label"
        return
    fi
    echo "unowned $label lock is not empty; inspect $lock_dir" >&2
    return 1
}

experiment_release_lock() {
    local lock_dir="$1"
    rm -f -- "$lock_dir/owner"
    rmdir "$lock_dir" 2>/dev/null || true
}
