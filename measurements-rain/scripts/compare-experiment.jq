def config_id:
    if .auth_mode == "any" and .index_order == "unordered_btree" and .bitmap_chunk_bytes == "" then "any_unordered_btree"
    elif .auth_mode == "any" and .index_order == "unordered" and .bitmap_chunk_bytes == "" then "any_unordered"
    elif .auth_mode == "current" and .bitmap_chunk_bytes == "32" and .index_order == "unordered_btree" then "current_unordered_btree"
    elif .auth_mode == "current" and .bitmap_chunk_bytes == "32" and .index_order == "unordered" then "current_unordered"
    elif .auth_mode == "current" and .bitmap_chunk_bytes == "32" and .index_order == "ordered" then "current_ordered"
    else error("unexpected authentication/index/chunk configuration: auth_mode=\(.auth_mode) index_order=\(.index_order) chunk=\(.bitmap_chunk_bytes)") end;
def differences($cases):
    [["any_unordered", "current_unordered"],
     ["current_unordered", "current_ordered"],
     ["any_unordered", "current_ordered"]]
    | map(. as [$base, $target] | {
        key: ($target + "_minus_" + $base),
        value: {baseline: $base, target: $target,
                delta_mean_us: ($cases[$target].mean_us - $cases[$base].mean_us),
                overhead_percent: (100 * ($cases[$target].mean_us / $cases[$base].mean_us - 1))}
      }) | from_entries;
def pairwise_differences($cases):
    (if $cases | has("any_unordered") then "any_unordered" else "any_unordered_btree" end) as $base
    | {("current_ordered_minus_" + $base): {
            baseline: $base,
            target: "current_ordered",
            delta_mean_us: ($cases["current_ordered"].mean_us - $cases[$base].mean_us),
            overhead_percent: (100 * ($cases["current_ordered"].mean_us / $cases[$base].mean_us - 1))
        }};
def group_cases($rows; $kind):
    [$rows[] | select(.write_type == $kind)]
    | if length == 3 then . else error("expected three configurations per write type") end
    | map(. + {configuration: (if config_id == "any_unordered_btree" then "any_unordered" elif config_id == "current_unordered_btree" then "current_unordered" else config_id end)})
    | if (map(.configuration) | unique | length) == 3 then . else error("missing or duplicate configuration") end
    | map({key: .configuration, value: .}) | from_entries;
def group_cases_pairwise($rows; $kind):
    [$rows[] | select(.write_type == $kind)]
    | if length == 2 then . else error("expected two configurations per write type") end
    | map(. + {configuration: config_id})
    | if (map(.configuration) | sort) == ["any_unordered", "current_ordered"] or
         (map(.configuration) | sort) == ["any_unordered_btree", "current_ordered"]
      then . else error("expected Any unordered and Current ordered configurations") end
    | map({key: .configuration, value: .}) | from_entries;
if length == (if $experiment == "write-type" then 6 else ($order | split(",") | length) end) then .
else error("unexpected Criterion result count") end
| if all(.[];
    (.keyspace_keys | tonumber) == $keys and .write_calls_per_batch == $updates and
    .value_layout == "fixed" and .merkle_family == "mmb" and
    .batch_parent == "directly from database" and .seed_synced == "yes" and
    .commonware_page_cache == "retained (warm)" and .input_generation_timed == false and
    .write_skew_exponent == $skew and .mean_us > 0 and .sample_count > 0)
  then . else error("experiment workload settings do not match") end
| . as $rows
| (map(.operation // "merkleize") | unique) as $operations
| if ($operations | length) == 1 and ($operations[0] == "merkleize" or $operations[0] == "apply_study")
  then . else error("cannot mix operations or compare unsupported phases") end
| $operations[0] as $operation
| if $operation == "apply_study" and (($order | split(",") | sort) != ["any_unordered_btree", "current_ordered"])
  then error("apply study requires Any unordered B-Tree and Current ordered") else . end
| (if $operation == "apply_study" then "apply_batch" else "batch_creation_staging_merkleization_root" end) as $scope
| (if $operation == "apply_study" then "buffered" else "speculative" end) as $durability
| if all(.[]; (.timing_scope // $scope) == $scope and (.durability // $durability) == $durability and
     (if $operation == "apply_study" then .iteration_policy == "independent_snapshot_fixed_inputs" else true end))
  then . else error("mismatched timing boundary, durability, or iteration policy") end
| if ($order | split(",") | sort) == (map(config_id) | unique | sort)
  then . else error("configuration order does not match measured engines") end
| {schema_version: 2, operation: $operation, timing_scope: $scope, durability: $durability, experiment: $experiment, repeat: $repeat,
   keyspace_keys: $keys, write_calls_per_batch: $updates,
   write_skew_exponent: $skew, configuration_order: ($order | split(","))}
  + (if $experiment == "write-type" then
        if ([$rows[].write_type] | unique | length) == 3 then
            group_cases_pairwise($rows; "overwrite") as $overwrites
            | group_cases_pairwise($rows; "insert") as $inserts
            | group_cases_pairwise($rows; "delete") as $deletes
            | {configurations: ($overwrites | keys | map(. as $id | {
                  key: $id, value: {
                    overwrite: $overwrites[$id],
                    insert: $inserts[$id],
                    delete: $deletes[$id],
                    insert_minus_overwrite_mean_us: ($inserts[$id].mean_us - $overwrites[$id].mean_us),
                    insert_overhead_percent: (100 * ($inserts[$id].mean_us / $overwrites[$id].mean_us - 1)),
                    delete_minus_overwrite_mean_us: ($deletes[$id].mean_us - $overwrites[$id].mean_us),
                    delete_overhead_percent: (100 * ($deletes[$id].mean_us / $overwrites[$id].mean_us - 1))}
                }) | from_entries),
               comparisons: {
                 overwrite: pairwise_differences($overwrites),
                 insert: pairwise_differences($inserts),
                 delete: pairwise_differences($deletes)
               }}
        else
            group_cases($rows; "overwrite") as $overwrites
            | group_cases($rows; "insert") as $inserts
            | {configurations: ($overwrites | keys | map(. as $id | {
                  key: $id, value: {overwrite: $overwrites[$id], insert: $inserts[$id],
                    insert_minus_overwrite_mean_us: ($inserts[$id].mean_us - $overwrites[$id].mean_us),
                    insert_overhead_percent: (100 * ($inserts[$id].mean_us / $overwrites[$id].mean_us - 1))}
                }) | from_entries),
               comparisons: {overwrite: differences($overwrites), insert: differences($inserts)}}
        end
     else (if ($order | split(",") | length) == 2 then group_cases_pairwise($rows; "overwrite")
           else group_cases($rows; "overwrite") end) as $cases
        | {configurations: $cases, comparisons: (if ($cases | length) == 2 then pairwise_differences($cases) else differences($cases) end)} end)
