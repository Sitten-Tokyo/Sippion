# RC28 source-level validation

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
