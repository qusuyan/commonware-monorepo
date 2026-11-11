"""Read controlled B-Tree pairs and legacy comparison archives."""
import json
from pathlib import Path

LABELS = {"any_unordered": "Any + unordered", "any_unordered_btree": "Any + unordered logic (B-Tree)",
          "current_unordered": "Current + unordered", "current_ordered": "Current + ordered"}
COLORS = {"any_unordered": "#477C98", "any_unordered_btree": "#477C98",
          "current_unordered": "#C2673F", "current_ordered": "#658647"}
PAIRS = (("any_unordered", "current_unordered"), ("current_unordered", "current_ordered"),
         ("any_unordered", "current_ordered"), ("any_unordered_btree", "current_ordered"))
ALLOWED_CONFIGURATION_SETS = (
    {"any_unordered", "current_unordered", "current_ordered"},
    {"any_unordered", "current_ordered"},
    {"any_unordered_btree", "current_ordered"},
)
EXPECTED_IDENTITIES = {
    "any_unordered": ("any", "unordered"),
    "any_unordered_btree": ("any", "unordered_btree"),
    "current_unordered": ("current", "unordered"),
    "current_ordered": ("current", "ordered"),
}

def read_comparison(path):
    result = json.loads(Path(path).read_text())
    if result["experiment"] != "writesize" or result["keyspace_keys"] <= 0:
        raise ValueError("expected a writesize comparison with a positive key count")
    version = result.get("schema_version", 1)
    if version == 1:
        cases = {"any_unordered": result["any"], "current_unordered": result["current"]}
    elif version == 2:
        cases = result["configurations"]
        if set(cases) not in ALLOWED_CONFIGURATION_SETS:
            raise ValueError("unexpected configuration set")
        for name, case in cases.items():
            auth, order = EXPECTED_IDENTITIES[name]
            if case["auth_mode"] != auth or case["index_order"] != order:
                raise ValueError("configuration identity mismatch")
    else:
        raise ValueError("unsupported comparison schema")
    cases = {name: cases[name] for name in LABELS if name in cases}
    for case in cases.values():
        if not (case["sample_count"] > 0 and 0 <= case["mean_ci_low_us"] <= case["mean_us"] <= case["mean_ci_high_us"] and case["mean_us"] > 0):
            raise ValueError("invalid sample count or confidence interval")
    differences = {}
    for base, target in PAIRS:
        if base in cases and target in cases:
            differences[target + "_minus_" + base] = {
                "baseline": base, "target": target,
                "delta_mean_us": cases[target]["mean_us"] - cases[base]["mean_us"],
                "overhead_percent": 100 * (cases[target]["mean_us"] / cases[base]["mean_us"] - 1)}
    return result, cases, differences
