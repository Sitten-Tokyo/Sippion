# Quality and regression gates

Sippion treats retrieval quality, bounded resource use, protocol behavior, tooling safety, and supply-chain safety as separate regression surfaces. The normal CI remains deterministic and self-contained; network-dependent holdouts and longer fuzz campaigns live in dedicated workflows.

## Local deterministic checks

Run these before changing retrieval, query normalization, repository policy, or model-visible packing:

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

The holdout covers every language with dedicated Tree-sitter support in Sippion:

- Rust: serde_json;
- Python: Flask;
- JavaScript: Express;
- TypeScript: Zod;
- Go: Cobra;
- Java: Apache Commons Lang;
- C#: Spectre.Console;
- C: curl;
- C++: fmt.

A pinned rust-analyzer workspace case adds a larger multi-crate repository so the holdout is not limited to small or medium projects.

Run the complete external holdout manually with network access:

```sh
cargo build --release --locked
python3 scripts/external-retrieval-eval.py --binary target/release/sippion
```

The scheduled GitHub Actions workflow intentionally stays outside the required pull-request gate so a transient upstream/network outage cannot block merges. Pull requests that change the external manifest or evaluator still validate their syntax and JSON shape.

When adding an external case:

- pin a full 40-character commit SHA;
- choose source repositories with stable, reviewable licenses and ordinary Git transports;
- prefer a representative repository over a needlessly enormous clone when the language behavior is already covered;
- use a query of 1-8 technical terms;
- require a path and a source anchor that represent the intended implementation, not README text;
- do not add build, test, package-manager, submodule, or repository-provided command execution.

## Fuzzing and property tests

`tests/query_properties.rs` provides deterministic stable-toolchain invariants for normalization and coordination IDs. The cargo-fuzz suite exercises five separate boundaries:

- `query_normalize`: arbitrary query and coordination input;
- `redaction`: bounded high-confidence secret redaction, including repeat-pass stability;
- `root_scope`: broad-root rejection invariants;
- `mcp_stdio`: arbitrary line-oriented MCP stdio input through the production binary;
- `source_parser`: arbitrary UTF-8 source through production retrieval for all nine supported language extensions.

The MCP and source-parser targets are black-box tests: they launch the release binary rather than calling an internal parser-only helper. This keeps framing, process behavior, repository policy, and resource deadlines in the exercised path.

Relevant pull requests build every fuzz target and run short bounded campaigns. Longer campaigns run on schedule and via `workflow_dispatch`.

Local fuzzing requires the pinned nightly toolchain and cargo-fuzz:

```sh
rustup toolchain install nightly-2026-08-29
cargo +nightly-2026-08-29 install cargo-fuzz --version 0.13.2 --locked
cargo build --release --locked
SIPPION_FUZZ_BIN="$PWD/target/release/sippion" \
SIPPION_FUZZ_ROOT="$PWD/eval/fixture" \
cargo +nightly-2026-08-29 fuzz run mcp_stdio -- -max_total_time=120 -timeout=10
```

Crashes and minimized corpora are local artifacts and are ignored by Git.

## Performance regression comparison

`.github/workflows/performance.yml` builds the pull-request base and candidate on the same Ubuntu runner, then runs identical committed fixture queries through both binaries.

`scripts/perf-regression.py` measures cold process behavior:

- median and p95 query wall time;
- peak resident set size (RSS; maximum physical memory resident for the process);
- median scanned bytes;
- median model-visible estimated tokens.

`scripts/mcp-perf-regression.py` keeps one MCP process alive and measures first-pass and steady-state latency plus process peak RSS. This exercises the request-local and cross-request caches under a usage pattern closer to an AI client session.

The thresholds deliberately include both a relative ratio and an absolute allowance so normal hosted-runner jitter does not fail small changes. This gate is intended to catch gross regressions, not replace dedicated profiling.

Both cold and warm reports are emitted as JSON and retained as GitHub Actions artifacts for 90 days. The workflow also runs on pushes to `main`, providing a rolling performance history instead of PR-only snapshots.

## Installer and workflow static analysis

`.github/workflows/tooling-static-analysis.yml` checks code that sits outside the Rust compiler's normal coverage:

- ShellCheck analyzes the Unix bootstrap and installer scripts;
- PSScriptAnalyzer analyzes the PowerShell bootstrap and installer scripts;
- actionlint validates GitHub Actions syntax, expressions, and shell fragments;
- zizmor audits GitHub Actions-specific security risks.

Downloaded native static-analysis tools are version-pinned and SHA-256 verified before execution. The workflow itself uses read-only repository permissions.

## Test organization

Repository regression tests are grouped under `src/repo/tests/` by responsibility rather than kept in one monolithic file. The parent `src/repo/tests.rs` contains only shared test fixtures and module declarations. Keep new tests in the narrowest existing group; create another focused group if a file starts becoming a second monolith.

Single-function helper modules should have a real architectural boundary. If a helper is used only by its parent implementation and carries no independent policy or abstraction, prefer keeping it with the parent rather than adding an extra module solely for file-count symmetry.
