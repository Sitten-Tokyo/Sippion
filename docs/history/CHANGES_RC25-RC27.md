# RC25–RC27 change records (historical record, consolidated)

Consolidated verbatim from `CHANGES_RC25.md`, `CHANGES_RC26.md`, and
`CHANGES_RC27.md` to reduce file count. Content is unchanged apart from
demoting each source's top heading by one level. These notes are not
current installation instructions or release approval; use the root
`README.md`, the current workflows, and the Rust quality gates for that.

---

## Source: CHANGES_RC25.md (verbatim below)

# RC25 context changes (historical record)

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
- Added an explicit integrated-software/function matrix to `README.md` and the integration-boundary document now kept at `docs/integrations.md`.
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

---

## Source: CHANGES_RC26.md (verbatim below)

# RC26 adaptive semantic changes (historical record)

- Bumped package version to `0.1.0-rc.26`.
- Kept the single public MCP tool: `repo_context`.
- Replaced the default fixed 128 MiB retrieval budget with bounded adaptive retrieval:
  - starts at 32 MiB;
  - expands through 64 / 128 / 256 / 512 MiB only when incomplete and low-confidence;
  - `--scan-budget-mib 16..512` is now the adaptive ceiling.
- Added deterministic retrieval confidence metadata and adaptive-round/cap reporting.
- Replaced the fixed ~8 KB model-visible response with bounded 8 / 16 / 24 / 32 KiB tiers.
- Added source-only semantic extraction on already-ranked Tree-sitter candidates:
  - exact identifier references;
  - call context;
  - type context;
  - implementation/inheritance context where represented by supported grammars;
  - import/module path hints.
- Added weighted repository edges:
  - implementation `0.95`;
  - call `0.90`;
  - type `0.85`;
  - exact reference `0.80`;
  - import `0.40`;
  - lexical fallback `0.15`.
- Added weighted PageRank so stronger semantic evidence contributes more than raw name coincidence.
- Updated structural output to expose semantic edge kind and weight.
- Preserved the safety boundary: no network client, shell execution, LSP/compiler subprocess, macro expansion, build script, procedural macro, persistent index, or repository mutation.
- Kept `Cargo.lock` absent rather than fabricating a lockfile without a Rust toolchain.

## Important semantic caveat

RC26 improves **semantic ranking**, but it does not claim compiler-authoritative type resolution or LSP-grade find-references/go-to-definition. Full compiler/LSP analysis should remain a separately authorized, sandboxed, on-demand tier because it can interact with build scripts, procedural macros, toolchains, and repository-controlled execution paths.

## Validation status

The packaging environment for this archive did not contain `cargo`, `rustc`, or `rustfmt`. Static source/TOML checks, safety-boundary scans, checksum verification, and ZIP integrity are performed during packaging; compilation, unit tests, Clippy, formatting, and MCP conformance remain release gates.

---

## Source: CHANGES_RC27.md (verbatim below)

# RC27 completeness and safety hardening (historical record)

- Bumped package version to `0.1.0-rc.27`.
- Candidate generation now records when the ranked candidate list is pruned before exact source verification. Any such pruning forces `SearchOutcome.truncated = true`, so an n-gram/path candidate cap can never be misreported as a complete `NO_MATCH`.
- Discovery now treats metadata-known oversized sources as deliberate policy exclusions instead of unfinished index coverage. Stable non-UTF-8 sources discovered during a search are cached for the remainder of that adaptive call and likewise removed from the effective searchable denominator. `SearchCoverage.policy_excluded_files` reports the count.
- Tree-sitter post-parse AST traversal now shares the per-file deadline/cancellation guard and has a 500,000-node hard budget. Source import scanning is deadline/cancellation bounded as well.
- Source verification now compares a stronger open-handle stamp before and after reading. On Unix this includes device, inode, status-change time, and hard-link count in addition to length and modification time. Indexed documents store the verified post-read stamp rather than stale discovery metadata.
- Unix reads reject any regular file whose hard-link count is greater than one, preventing a repository entry from aliasing an out-of-root inode through a hard link. Discovery reports such files as policy exclusions. Non-Unix retains the documented trusted-root requirement because stable Rust does not expose a portable hard-link count on every platform.
- Added regression tests for candidate-pruning completeness, oversized/non-UTF-8 policy exclusions, same-length file replacement stamps, Unix hard-link rejection, and the AST node budget.

## Intentionally not completed in this archive

RC27 does **not** close the separate release-validation gate from RC26: `Cargo.lock` generation, compilation, tests, Clippy, rustfmt, and MCP conformance still need to be run in a Rust-enabled environment before release.
