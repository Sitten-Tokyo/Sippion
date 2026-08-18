# RC28 multi-agent shared-context hardening

- Bumped package version to `0.1.0-rc.28`.
- Added optional bounded `session_id` and `agent_id` fields to `repo_context`; both are volatile and never persisted.
- Changed session memory from one undifferentiated process history into session/agent-aware ranking memory.
- Added bounded sibling-agent diversity ranking: overlapping paths already surfaced to another agent in the same session receive a small penalty, while same-agent follow-up continuity receives a small boost. Strong lexical/semantic evidence still dominates.
- Added file-level single-flight for RAM-index growth so concurrent cold-start queries do not repeatedly read/index the same unchanged source file.
- Added a process-wide bounded AST/symbol + source-only semantic-fact cache keyed by verified `SourceStamp`. Source bodies are not retained in the cache.
- Added per-file single-flight structural analysis: concurrent agents requesting the same unchanged file wait for one bounded parse instead of repeating Tree-sitter/semantic extraction.
- Added a bounded structural graph cache keyed by canonical candidate file set + verified source stamps. Candidate order is canonicalized so sibling agents can share graphs even when lexical ranking order differs.
- Cache capacities are bounded to 256 analyzed files and 64 structural graphs; stale content misses automatically because stamps participate in cache identity, and least-recently-used entries are evicted approximately by monotonic access tick.
- Increased MCP tool concurrency from 2 to 4 in-flight calls.
- Replaced the previous single global 6-call window with a two-level rate limiter: 8 calls/60s per actor and 24 calls/60s process-wide. Expired actor buckets are removed to prevent unbounded limiter-state growth from arbitrary agent IDs.
- Updated README, AGENTS guidance, MCP capability metadata, and integration boundaries for multi-agent use.
- Added source-level regression tests for coordination-ID validation, sibling-agent diversity, shared analysis/graph cache reuse, and two-level rate limiting.

RC28 intentionally does **not** close the separate release-validation gate inherited from RC26/RC27: `Cargo.lock` generation, compilation, tests, Clippy, rustfmt, and MCP conformance still need to be run in a Rust-enabled environment before release.
