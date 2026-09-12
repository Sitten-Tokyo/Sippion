# RC26–RC28 packaging/source validation records (historical record, consolidated)

Consolidated verbatim from `VALIDATION_RC26.md`, `VALIDATION_RC27.md`, and
`VALIDATION_RC28.md` to reduce file count. Content is unchanged apart from
demoting each source's top heading by one level. These notes are not
current installation instructions or release approval; use the root
`README.md`, the current workflows, and the Rust quality gates for that.

---

## Source: VALIDATION_RC26.md (verbatim below)

# RC26 packaging validation (historical record)

Performed in the packaging environment:

- parsed `Cargo.toml` successfully with a TOML parser;
- verified package version is `0.1.0-rc.26` and Rust minimum remains `1.85`;
- checked Rust source delimiter/string/comment balance with a local static scanner;
- scanned `src/` for newly introduced shell-command, process-spawn, network-socket/client, unsafe-block, or symlink-following primitives;
- confirmed the direct dependency set is unchanged from RC25;
- regenerated `SHA256SUMS` after all source/document changes;
- verified every checksum with `sha256sum -c`;
- verified the final ZIP with `unzip -t`.

Not available in this packaging environment:

- `cargo`;
- `rustc`;
- `rustfmt`.

Therefore the following remain mandatory release gates in a Rust-enabled environment:

```sh
cargo generate-lockfile
cargo build --release --locked
cargo test --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo fmt --check
```

MCP conformance/integration testing should also be run before publishing the RC as release-ready.

---

## Source: VALIDATION_RC27.md (verbatim below)

# RC27 source-level validation (historical record)

This archive contains source-level hardening only. The separate RC26 release-validation gate was intentionally not claimed as solved.

Checked in this packaging environment:

- candidate-pruning completeness flag is wired into `SearchOutcome.truncated`;
- metadata-known policy exclusions are removed before effective index-coverage calculation;
- stable non-UTF-8 policy skips persist across adaptive rounds within one call;
- verified post-read source stamps are stored in the RAM index;
- Unix hard-link rejection is applied at discovery and open-handle read time;
- AST traversal/import scanning contains deadline/cancellation checks and a node budget;
- regression tests for the new invariants are present in the source tree;
- no `Cargo.lock` was fabricated.

Not executed here and still mandatory before release:

```text
cargo generate-lockfile
cargo build --release --locked
cargo test --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo fmt --check
MCP integration/conformance tests
```

---

## Source: VALIDATION_RC28.md (verbatim below)

# RC28 source-level validation (historical record)

This archive was hardened for multi-agent use without changing the previously deferred release gate.

## Performed in this packaging environment

- inspected the RC27 source archive and applied the RC28 changes directly;
- verified the package version is `0.1.0-rc.28`;
- verified `repo_context` exposes optional bounded `session_id` and `agent_id` fields in addition to `q`;
- verified file-level index single-flight rechecks the shared RAM index while owning the flight registry before claiming work, preventing a check/claim race;
- verified structural analysis cache stores only verified stamps, extracted symbols, and source-only semantic facts (not source bodies);
- verified per-file analysis single-flight uses a condition variable, cancellation checks, and the existing wall-clock deadline;
- verified graph cache keys include canonical candidate paths and verified source stamps and is bounded to 64 entries;
- verified session/agent memory is bounded to 128 records and diversity adjustment is clamped;
- verified rate limiting remains bounded both per actor and process-wide and expired actor buckets are removed;
- verified Rust source delimiter balance with a local lexical sanity checker;
- checked for accidental production network/shell/compiler/LSP additions; none were introduced by RC28;
- regenerated `SHA256SUMS` after the final source/document changes;
- validated final ZIP integrity with `unzip -t` and independently compared every archived file to the source tree after packaging.

## Deliberately NOT performed (release gate remains open)

The packaging environment does not contain `cargo`, `rustc`, or `rustfmt`, and the user explicitly asked to leave the separate build/test gate unresolved. Therefore RC28 does not claim:

- `cargo generate-lockfile` / `Cargo.lock` generation;
- `cargo build --release --locked`;
- `cargo test --locked`;
- `cargo clippy --all-targets --all-features --locked -- -D warnings`;
- `cargo fmt --check`;
- live MCP conformance/interoperability testing.

Do not treat this archive as release-ready until those gates pass in a Rust 1.85 environment.
