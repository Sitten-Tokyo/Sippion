# Sippion integration boundaries

Sippion uses the following design references while keeping all retrieval local.
No upstream project is vendored or executed at runtime.

| Technique / reference | Current use | Deliberately excluded |
|---|---|---|
| BM25 | RAM-index lexical candidate ranking (`k1=1.2`, `b=0.75`) | external search service |
| Tree-sitter | bounded AST parsing for already-ranked Rust/Python/JS/TS/Go candidates | whole-repository compiler frontend |
| RepoMapper-style map | candidate-file centrality | persistent tag database |
| codebase-memory-style structure | source-only definitions/references/import hints | persistent knowledge DB, LSP subprocess |
| Aho–Corasick | weak lexical fallback edges | treating raw string coincidence as authoritative call graph |
| weighted PageRank | semantic evidence strength affects centrality | compiler-proven call graph claims |
| Repomix-style packing | one bounded multi-file context response | whole-repository export/output files |
| RTK-style compaction | conservative whitespace-only compaction | shell interception/command rewriting |
| Engram-style continuity | RAM-only session/agent query-term/path memory with sibling diversity | persistent/cross-process source memory |
| single-flight | concurrent agents share one bounded AST/semantic parse per unchanged file | unbounded worker pools |
| structural graph cache | bounded RAM graph reuse keyed by canonical candidate set + verified source stamps | persistent graph database |

## Semantic boundary

The Tier 2 resolver is intentionally **source-only**. It records exact
syntax-tree identifier references, call/type/implementation contexts, and
import paths, then connects those references to declarations already present in
the bounded candidate set.

It does **not** execute:

- rust-analyzer or another LSP server;
- `cargo check` / compiler frontends;
- macro expansion;
- procedural macros;
- build scripts;
- repository binaries or shell commands.

This gives Sippion stronger ranking signals while preserving the local/read-only/no-execution boundary. A future authoritative type/LSP tier should be explicit opt-in and separately sandboxed.

## Adaptive resource policy

Retrieval starts at 32 MiB and can expand to the configured 16–512 MiB ceiling only when results remain incomplete and low-confidence. Model-visible output similarly scales through 8/16/24/32 KiB tiers instead of becoming unbounded.

## Implementation locations

- `src/repo.rs`: adaptive scan loop, session/agent diversity memory, single-flight analysis cache, semantic graph cache, confidence calculation, semantic edge construction.
- `src/syntax.rs`: bounded source-only semantic facts.
- `src/hybrid.rs`: weighted PageRank.
- `src/core.rs`: optional coordination IDs and adaptive context budget policy.
- `src/main.rs`: bounded multi-agent concurrency/rate limiting, adaptive packing, and model-visible coverage metadata.
