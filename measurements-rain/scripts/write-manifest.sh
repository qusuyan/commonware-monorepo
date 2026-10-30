#!/usr/bin/env bash
# Write stable run metadata without requiring Python or third-party packages.
set -euo pipefail

if [ "$#" -ne 3 ]; then
    echo "usage: $0 <run-id> <kind> <command>" >&2
    exit 2
fi

run_id="$1"
kind="$2"
command="$3"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
run_dir="$root/results/$run_id"
mkdir -p "$run_dir"

json_escape() {
    printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}

cpu="unknown"
if command -v sysctl >/dev/null 2>&1; then
    cpu="$(sysctl -n machdep.cpu.brand_string 2>/dev/null || sysctl -n hw.model 2>/dev/null || printf unknown)"
elif [ -r /proc/cpuinfo ]; then
    cpu="$(sed -n 's/^model name[[:space:]]*:[[:space:]]*//p' /proc/cpuinfo | head -1)"
fi
if [ -z "$cpu" ] || [ "$cpu" = "unknown" ]; then
    if command -v system_profiler >/dev/null 2>&1; then
        cpu="$(system_profiler SPHardwareDataType 2>/dev/null | sed -n 's/^[[:space:]]*Chip: //p; s/^[[:space:]]*Processor Name: //p' | head -1)"
    fi
fi
cpu="${cpu:-unknown}"

fixed_updates="null"
write_type="null"
skew="null"
selection="null"
if [ "$kind" = "criterion" ] && [ -n "${QMDB_MERKLEIZE_UPDATES:-}" ]; then
    fixed_updates="$QMDB_MERKLEIZE_UPDATES"
fi
if [ "$kind" = "criterion" ]; then
    write_type="\"$(json_escape "${QMDB_MERKLEIZE_WRITE_TYPE:-overwrite}")\""
    skew="$("$root/scripts/validate-skew.sh" "${QMDB_MERKLEIZE_SKEW:-0}")"
    if [ "$skew" = 0 ]; then selection='"uniform"'; else selection='"zipf"'; fi
fi

cat > "$run_dir/manifest.json" <<EOF
{
  "run_id": "$(json_escape "$run_id")",
  "kind": "$(json_escape "$kind")",
  "fixed_updates_per_batch": $fixed_updates,
  "write_type": $write_type,
  "write_skew_exponent": $skew,
  "write_selection": $selection,
  "merkleize_input_generation_timed": true,
  "timestamp_utc": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "git_revision": "$(git rev-parse HEAD)",
  "git_dirty": $(test -n "$(git status --porcelain)" && printf true || printf false),
  "host": "$(json_escape "$(hostname)")",
  "os": "$(json_escape "$(uname -srm)")",
  "cpu": "$(json_escape "$cpu")",
  "rustc": "$(json_escape "$(rustc --version)")",
  "command": "$(json_escape "$command")"
}
EOF

touch "$run_dir/notes.md"
