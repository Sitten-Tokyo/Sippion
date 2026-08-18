# RC29 source-level validation

This archive received source-level/static validation only. The separate Rust build/test gate remains intentionally unresolved.

Validated in this packaging environment:

- applied the RC29 changes directly to the RC28 source archive;
- verified package version metadata is `0.1.0-rc.29`;
- verified shared `CachedAnalysis` stores structural cached symbols (`name`, `kind`, `line`) and no source-line `signature` field;
- verified map signatures are reconstructed from the freshly verified/redacted source path on each call;
- verified policy-excluded zero-hit searches cannot use complete-no-match confidence and model-visible output distinguishes `NO_MATCH_IN_SEARCHABLE_SET` from absolute `NO_MATCH`;
- 2026-08-17 follow-up: verified source logic now adds a conservative policy-exclusion sentinel for visible directories containing `.gitignore`/`.ignore`, while leaving ignore traversal enabled so ignored content is not newly disclosed;
- 2026-08-17 follow-up: verified exact matches removed by high-confidence secret redaction are represented by a content-withheld `[SIPPION_REDACTED_MATCH: ...]` marker rather than disappearing into `NO_MATCH`;
- 2026-08-17 follow-up: verified inline sensitive-literal parsing no longer exempts short non-empty credentials by length; added source-level regression cases for quoted and unquoted short values while preserving empty/structural/computed non-secret forms;
- 2026-08-17 follow-up: added hashed Unicode-scalar candidate sketches so non-ASCII substring queries reach exact source verification instead of being filtered out by the RAM index;
- 2026-08-17 follow-up: removed process-wide Modern/Legacy mode binding; modern 2026-07-28 requests are validated from their own `_meta`, while only legacy-initialized state is retained for legacy compatibility;
- 2026-08-17 follow-up: verified prefixed multiline sensitive keys such as `OPENAI_API_KEY` and `DATABASE_PASSWORD` use the same underscore-aware boundary rule as inline assignments, including YAML block scalars;
- 2026-08-17 follow-up: `server/discover` and unsupported-version error data advertise both implemented revisions (`2026-07-28`, `2025-11-25`) while modern request `_meta` remains restricted to `2026-07-28`;
- 2026-08-17 follow-up: kept async requests registered through the final cancellation check/response commit so cancellation cannot lose the old remove-before-write race;
- 2026-08-17 Windows cache follow-up: verified source logic now serializes top-level structural-map construction on Windows and clears prior analysis/graph cache entries before candidate reads, closing metadata-only same-stamp reuse for structural facts;
- 2026-08-17 Windows cache follow-up: added a Windows-only regression test that seeds stale same-stamp cached symbols plus an extreme stale graph-centrality value and checks that fresh structure is rebuilt instead of reused;
- verified policy exclusions remain non-adaptive-expandable rather than causing futile scan-budget expansion;
- checked production source for newly introduced child-process/network execution paths; none were introduced by RC29;
- checked source for `unsafe` usage; the crate retains `#![forbid(unsafe_code)]`;
- regenerated `SHA256SUMS` after the final source/doc changes;
- verified the final ZIP CRC and byte-for-byte extracted contents against the packaged tree.

Not executed / not claimed:

- `cargo generate-lockfile`;
- `cargo build --release --locked`;
- `cargo test --locked`;
- `cargo clippy`;
- `cargo fmt --check`;
- MCP conformance/integration execution.

Regression tests for the 2026-08-17 follow-up fixes were added across `src/repo.rs`, `src/main.rs`, and `src/core.rs`; these tests, including the Windows stale-structural-cache regression, are source-level additions only until the Rust test gate is run.

`Cargo.lock` is intentionally not fabricated in this environment.
