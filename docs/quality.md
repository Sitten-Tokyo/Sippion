# Quality and regression gates

Sippion treats retrieval quality, bounded resource use, protocol behavior, supply-chain safety, installer correctness, and workflow security as separate regression surfaces. The normal CI remains deterministic and self-contained; network-dependent holdouts and fuzzing live in dedicated workflows.

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

The holdout covers every supported parser language: Rust, Python, JavaScript, TypeScript, Go, Java, C#, C, and C++. TypeScript, libgit2, and fmt also provide larger-repository stress cases so evaluation is not limited to small fixtures.

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

`tests/query_properties.rs` provides deterministic stable-toolchain invariants for normalization and coordination IDs. The cargo-fuzz suite exercises five independent input surfaces:

- `query_normalize`: arbitrary query and coordination input;
- `redaction`: bounded secret redaction plus a non-disclosure property for synthesized API keys;
- `path_policy`: traversal, absolute-path, deny, and prune classification;
- `syntax_parse`: bounded Tree-sitter parsing across all nine supported languages;
- `mcp_json`: arbitrary MCP JSON and `repo_context` argument decoding.

Relevant pull requests compile every target and run short bounded campaigns. Longer campaigns run on schedule and via `workflow_dispatch`.

Local fuzzing requires the pinned nightly toolchain and cargo-fuzz:

```sh
rustup toolchain install nightly-2026-08-29
cargo +nightly-2026-08-29 install cargo-fuzz --version 0.13.2 --locked
cargo +nightly-2026-08-29 fuzz run redaction -- -max_total_time=180 -timeout=10
```

Crashes and minimized corpora are local artifacts and are ignored by Git. The `fuzzing` Cargo feature exists only to expose narrow safety probes to the fuzz crate; it is disabled in normal builds and does not widen the normal public API surface.

## Performance regression comparison

`.github/workflows/performance.yml` builds the pull-request base and candidate on the same Ubuntu runner. It runs both cold CLI queries and a long-lived MCP process so process-global indexes/caches and repeated session queries are measured separately.

The cold comparison records:

- median and p95 cold-query wall time;
- peak resident set size (RSS; maximum physical memory resident for the process);
- median scanned bytes;
- median model-visible estimated tokens.

The warm MCP comparison records first-pass and repeated-query median/p95 latency, long-lived process RSS, and median scanned bytes. The benchmark keeps request volume below the server's normal local rate limits rather than disabling production guards.

Thresholds deliberately include both a relative ratio and an absolute allowance so normal hosted-runner jitter does not fail small changes. Each run uploads `cold.json` and `warm-mcp.json` as a 30-day GitHub Actions artifact, making regressions reviewable after the job completes instead of leaving the numbers only in logs.

For architectural performance work, inspect or attach those comparison JSON files and explain any intentional budget increase.

## Installer static analysis

`.github/workflows/installer-lint.yml` supplements the existing shell/parser syntax checks with:

- checksum-pinned ShellCheck 0.11.0 for `bootstrap.sh` and `install.sh`;
- exact PSScriptAnalyzer 1.25.0 for `bootstrap.ps1` and `install.ps1`.

Installer findings should be fixed or narrowly justified rather than broadly suppressing a rule, because these scripts are part of the distribution trust boundary.

## GitHub Actions lint and security audit

`.github/workflows/workflow-lint.yml` runs checksum-pinned actionlint 1.7.12 for workflow syntax/expression validation and zizmor 1.29.0 for GitHub Actions security findings. New actions should continue to use immutable commit SHAs rather than floating tags.

## Repository module hygiene

Large implementation files should be split only at behavior-preserving boundaries. Repository state/data types live in `src/repo/types.rs`, while orchestration remains in `src/repo.rs`. Tiny one-function modules are not kept solely for file-count symmetry; helpers can stay in the parent module when that reduces indirection without changing visibility.
