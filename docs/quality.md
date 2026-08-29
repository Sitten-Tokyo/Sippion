# Quality and regression gates

Sippion treats retrieval quality, bounded resource use, protocol behavior, and supply-chain safety as separate regression surfaces. The normal CI remains deterministic and self-contained; network-dependent holdouts and fuzzing live in dedicated workflows.

## Local deterministic checks

Run these before changing retrieval, query normalization, or repository policy:

```sh
cargo fmt --check
cargo build --release --locked
cargo test --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
python3 scripts/retrieval-eval.py --binary target/release/sippion
```

The fixture and self-hosted evaluation suites are the merge-time quality gates because their source inputs are committed and reproducible.

## External OSS holdout

`eval/external_repos.json` pins each external repository to an exact commit SHA. `scripts/external-retrieval-eval.py` fetches only that commit, never initializes submodules, never executes repository code, and evaluates the release Sippion binary against expected paths and evidence anchors.

Run it manually with network access:

```sh
cargo build --release --locked
python3 scripts/external-retrieval-eval.py --binary target/release/sippion
```

The scheduled GitHub Actions workflow intentionally stays outside the required pull-request gate so a transient upstream/network outage cannot block merges. Pull requests that change the external manifest or evaluator still validate their syntax and JSON shape.

When adding an external case:

- pin a full 40-character commit SHA;
- choose source repositories with stable, reviewable licenses and ordinary Git transports;
- use a query of 1-8 technical terms;
- require a path and a source anchor that represent the intended implementation, not README text;
- do not add build, test, package-manager, submodule, or repository-provided command execution.

## Fuzzing and property tests

`tests/query_properties.rs` provides deterministic stable-toolchain invariants for normalization and coordination IDs. `fuzz/fuzz_targets/query_normalize.rs` feeds arbitrary bytes through the same public input boundary with cargo-fuzz.

Relevant pull requests compile the fuzz target and run a short bounded campaign. A longer campaign runs on schedule and via `workflow_dispatch`.

Local fuzzing requires the pinned nightly toolchain and cargo-fuzz:

```sh
rustup toolchain install nightly-2026-08-29
cargo +nightly-2026-08-29 install cargo-fuzz --version 0.13.2 --locked
cargo +nightly-2026-08-29 fuzz run query_normalize -- -max_total_time=180 -timeout=10
```

Crashes and minimized corpora are local artifacts and are ignored by Git.

## Performance regression comparison

`.github/workflows/performance.yml` builds the pull-request base and candidate on the same Ubuntu runner, then runs identical committed fixture queries through both binaries. `scripts/perf-regression.py` compares:

- median and p95 cold-query wall time;
- peak resident set size (RSS; maximum physical memory resident for the process);
- median scanned bytes;
- median model-visible estimated tokens.

The thresholds deliberately include both a relative ratio and an absolute allowance so normal hosted-runner jitter does not fail small changes. This gate is intended to catch gross regressions, not replace dedicated profiling.

For architectural performance work, attach the comparison JSON to the pull request and explain any intentional budget increase.
