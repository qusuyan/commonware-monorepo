# Benchmark result schema

Each benchmark run creates `results/<run-id>/` and contains:

- `manifest.json`: host, revision, toolchain, timestamp, and invoked command.
- `criterion/`: Criterion output copied after the run.
- `callgrind/`: Gungraun/Callgrind output copied after the run.
- `summary.csv` and `summary.json`: normalized observations.
- `notes.md`: operator notes and failures.

Results are intentionally ignored by Git because they are machine-specific and can be large.
