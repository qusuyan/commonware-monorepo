# Shared orchestration helpers for qmdb_cpu focused and full matrix runners.
# Sourced by run-focused-matrix_qmdbcpu.sh and run-full-matrix_qmdbcpu.sh.

cpu_matrix_init() {
    local suite_kind="$1"
    cpu_matrix_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
    cpu_matrix_repo_root="$(cd "$cpu_matrix_root/.." && pwd)"
    source "$cpu_matrix_root/scripts/experiment-configs.sh"

    cpu_matrix_suite_dir="$cpu_matrix_root/results/cpu-suites/$cpu_matrix_suite_id"
    cpu_matrix_runs_file="$cpu_matrix_suite_dir/suite_runs.jsonl"
    cpu_matrix_status_file="$cpu_matrix_suite_dir/status.json"
    cpu_matrix_min_free_kib=$((10 * 1024 * 1024))

    cpu_matrix_source_paths=(
        bench/src/cpu.rs
        storage/src/qmdb/benches/cpu.rs
        storage/src/qmdb/benches/cpu_workload.rs
        storage/src/qmdb/benches/common.rs
        storage/src/qmdb/benches/snapshots.rs
        storage/src/qmdb/benches/merkleize_config.rs
        measurements-rain/scripts/run-cpu-telemetry.sh
        storage/Cargo.toml
        Cargo.lock
    )

    if [ -e "$cpu_matrix_suite_dir" ] && [ "${cpu_matrix_resume_step:-1}" -eq 1 ]; then
        echo "CPU suite directory already exists: $cpu_matrix_suite_dir" >&2
        exit 2
    fi
    mkdir -p "$cpu_matrix_suite_dir"
    exec > >(tee -a "$cpu_matrix_suite_dir/suite.log") 2>&1
    printf '%s\n' "$$" > "$cpu_matrix_suite_dir/runner.pid"

    cd "$cpu_matrix_repo_root"
    if [ "${cpu_matrix_resume_step:-1}" -eq 1 ]; then
        git status --short > "$cpu_matrix_suite_dir/git-status-at-start.txt"
        git rev-parse HEAD > "$cpu_matrix_suite_dir/git-revision-at-start.txt"
        cpu_matrix_capture_sources > "$cpu_matrix_suite_dir/source-hashes-at-start.txt"
        : > "$cpu_matrix_runs_file"
    elif ! cpu_matrix_sources_unchanged; then
        echo "CPU benchmark source changed since the failed suite started" >&2
        exit 1
    fi

    cpu_matrix_completed=$(( ${cpu_matrix_resume_step:-1} - 1 ))
    cpu_matrix_current="initializing ($suite_kind)"
    cpu_matrix_write_status running
}

cpu_matrix_capture_sources() {
    local src
    for src in "${cpu_matrix_source_paths[@]}"; do
        printf '%s  %s\n' "$(git hash-object "$src")" "$src"
    done
}

cpu_matrix_sources_unchanged() {
    cpu_matrix_capture_sources > "$cpu_matrix_suite_dir/source-hashes-current.txt"
    cmp -s "$cpu_matrix_suite_dir/source-hashes-at-start.txt" "$cpu_matrix_suite_dir/source-hashes-current.txt"
}

cpu_matrix_check_space() {
    local available_kib
    available_kib="$(df -Pk "$cpu_matrix_repo_root" | awk 'NR == 2 { print $4 }')"
    if [ "$available_kib" -lt "$cpu_matrix_min_free_kib" ]; then
        echo "available disk space is below 10 GiB: ${available_kib} KiB" >&2
        return 1
    fi
}

cpu_matrix_write_status() {
    local state="$1"
    jq -n \
        --arg suite_id "$cpu_matrix_suite_id" \
        --arg phase "$cpu_matrix_phase" \
        --arg state "$state" \
        --arg current "$cpu_matrix_current" \
        --argjson completed "$cpu_matrix_completed" \
        --argjson total "$cpu_matrix_total_steps" \
        --arg updated_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
        '{suite_id: $suite_id, phase: $phase, state: $state, current: $current,
          completed_steps: $completed, total_steps: $total, updated_at_utc: $updated_at}' \
        > "$cpu_matrix_status_file"
}

cpu_matrix_on_exit() {
    local exit_status=$?
    trap - EXIT
    if [ "$exit_status" -eq 0 ]; then
        cpu_matrix_current="complete"
        cpu_matrix_write_status complete
    else
        cpu_matrix_write_status failed
    fi
    exit "$exit_status"
}

cpu_matrix_setup_traps() {
    trap cpu_matrix_on_exit EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
}

cpu_matrix_run_pair() {
    local aspect="$1" keys="$2" updates="$3" write_type="$4" skew="$5" repeat="$6"
    local skew_tag mode run_id run_dir
    skew_tag="${skew//./p}"
    experiment_modes_pairwise "$repeat"

    for mode in "${modes[@]}"; do
        run_id="cpu-${cpu_matrix_suite_id}-${aspect}-k${keys}-u${updates}-${write_type}-s${skew_tag}-r${repeat}-${mode}"
        run_dir="$cpu_matrix_root/results/$run_id"
        echo "Running CPU telemetry: aspect=$aspect variant=$mode phase=$cpu_matrix_phase keys=$keys updates=$updates write_type=$write_type skew=$skew repeat=$repeat samples=$cpu_matrix_samples"
        "$cpu_matrix_root/scripts/run-cpu-telemetry.sh" "$run_id" \
            --phase "$cpu_matrix_phase" \
            --variant "$mode" \
            --keys "$keys" \
            --updates "$updates" \
            --samples "$cpu_matrix_samples" \
            --write-type "$write_type" \
            --skew "$skew"

        jq -cn \
            --arg aspect "$aspect" \
            --arg phase "$cpu_matrix_phase" \
            --arg variant "$mode" \
            --arg write_type "$write_type" \
            --arg run_id "$run_id" \
            --arg run_dir "$run_dir" \
            --argjson keys "$keys" \
            --argjson updates "$updates" \
            --argjson skew "$skew" \
            --argjson repeat "$repeat" \
            '{aspect: $aspect, phase: $phase, variant: $variant, keys: $keys, updates: $updates,
              write_type: $write_type, skew: $skew, repeat: $repeat, run_id: $run_id, run_dir: $run_dir}' \
            >> "$cpu_matrix_runs_file"
    done
}

cpu_matrix_run_step() {
    local label="$1"
    shift
    cpu_matrix_current="$label"
    cpu_matrix_write_status running
    cpu_matrix_check_space
    "$@"
    if ! cpu_matrix_sources_unchanged; then
        echo "CPU benchmark source changed during the suite" >&2
        return 1
    fi
    cpu_matrix_completed=$((cpu_matrix_completed + 1))
    cpu_matrix_current="completed: $label"
    cpu_matrix_write_status running
}

cpu_matrix_python() {
    local candidate
    for candidate in "${PYTHON3:-}" "$(command -v python3 2>/dev/null || true)" /usr/bin/python3; do
        if [ -n "$candidate" ] && "$candidate" --version >/dev/null 2>&1; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done
    echo "no working python3 interpreter found" >&2
    return 1
}

cpu_matrix_finalize() {
    local py
    py="$(cpu_matrix_python)"
    cpu_matrix_current="aggregating CPU comparison and CSVs"
    cpu_matrix_write_status running
    "$py" "$cpu_matrix_root/scripts/summarize-cpu-suite.py" "$cpu_matrix_suite_dir"
    echo "CPU suite complete. Summaries written under $cpu_matrix_suite_dir"
}
