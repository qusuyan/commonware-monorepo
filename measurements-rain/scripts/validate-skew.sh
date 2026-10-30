#!/usr/bin/env bash
# Validate and normalize the exponent shared by measurement launchers.
set -euo pipefail
if [ "$#" -ne 1 ]; then
    echo "usage: $0 <finite non-negative exponent>" >&2
    exit 2
fi
if ! jq -ner --arg value "$1" '
    $value | tonumber
    | if . >= 0 and (isinfinite | not) and (isnan | not)
      then (if . == 0 then 0 else . end)
      else error("invalid skew") end
' 2>/dev/null; then
    echo "skew must be a finite, non-negative exponent" >&2
    exit 2
fi
