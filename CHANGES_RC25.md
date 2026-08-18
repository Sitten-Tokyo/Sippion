# RC25 Context changes

- Replaced the three public MCP tools (`repo_discover`, `repo_map`, `repo_pack`) with one public tool: `repo_context`.
- Added `AGENTS.md` and `CLAUDE.md` with the requested short instruction to use Sippion before broad recursive repository exploration.
- Changed structural analysis to consume the already-ranked search candidate set instead of initiating a second repository-wide search.
- Kept BM25, path ranking, and Engram-inspired RAM-only session memory in the Local engine.
- Added explicit query-aware symbol ranking before structural rendering.
- Kept the RepoMapper/codebase-memory-inspired cross-file structural graph and PageRank-style centrality.
- Added explicit output-stage path deduplication.
- Kept bounded excerpt extraction and RTK-style conservative whitespace compaction.
- Consolidated Repomix-style multi-file packing and structural summary into one `sippion-context-v1` response.
- Consolidated capability-registry metadata to one `repository.context` capability and documented its Local engine / Output optimizer sub-capabilities.
- Added an explicit integrated-software/function matrix to `README.md` and `INTEGRATIONS.md`.
- Preserved no-network, read-only, project-scoped filesystem access, symlink refusal, secret redaction, cancellation, byte/time/result caps, and no production file writes.
- Original RC25 kept the direct Cargo dependency set unchanged; the reviewed hardening below adds `aho-corasick`.
- Bumped package version to `0.1.0-rc.25`.

Validation caveat: this packaging environment does not contain Rust tooling. Run the release gates in `README.md` before treating RC25 as release-ready.

## Reviewed hardening changes

- Replaced repeated structural `contains` scans with one Aho–Corasick multi-pattern matcher and added cancellation/wall-clock checks during graph construction.
- Allowed 1-term technical/identifier queries; single-term queries are capped at 8 search hits and 6 structural files, while 2-8-term queries retain the 16/12 limits.
- Pinned `rust-toolchain.toml` to Rust 1.85.0 instead of the moving `stable` channel.

## Incremental-index / stratified-scan / AST / licensing hardening

- Added a RAM-only incremental lexical index storing hashed term frequencies plus size/mtime stamps; source bodies are not retained.
- Discovery invalidates changed/deleted files; unchanged indexed files avoid broad source re-reads.
- Added stratified indexing: changed/path-relevant files, deterministic sample, then top-level-directory round-robin.
- Added startup `--scan-budget-mib 16..512` (default 128 MiB); the model cannot override it.
- Added response coverage metadata (`indexed/eligible`, partial index count, scanned files/bytes, budget).
- Added Tree-sitter only for already-ranked top candidates: Rust, Python, JavaScript/JSX, TypeScript/TSX, and Go, with heuristic fallback.
- Declared `MIT OR Apache-2.0`; added full license files and third-party notices.

- Preserved legacy ASCII substring recall in the RAM index with bounded 2/3-byte gram sketches; candidate false positives are removed by source re-verification before output.
- Made Tree-sitter parsing abortable with a 500 ms per-file budget plus the structural-stage cancellation/deadline guard.
- Added `discovery_complete` to coverage output so a partial metadata walk is not represented as complete repository coverage.
- Conservatively pinned `ignore` to 0.4.23 while Rust 1.85.0 remains the project toolchain; release validation must still resolve and lock the full dependency graph.
- Documented read-only repository access as an intentional least-privilege trust boundary; any future persistent cache or patch writer must use a separate explicit write boundary.
