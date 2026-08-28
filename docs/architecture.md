# Architecture

Sippion exposes one public MCP operation, `repo_context`. The local stdio
transport passes a validated request to the in-process service boundary:

```text
MCP / JSON-RPC
      |
      v
RepositoryService
      |
      v
LocalRepositoryService
      |
      v
RepositoryAccess
```

Retrieval is bounded and staged:

1. lexical candidate retrieval uses a RAM-only incremental index, BM25/path
   ranking, and optional volatile `session_id` / `agent_id` coordination;
2. Tree-sitter parses only already-ranked Rust, Python, JavaScript/TypeScript, Go, Java, C#, C, and C++ candidates;
3. source-only references, calls, types, implementations, and imports provide
   ranking evidence;
4. verified excerpts and structural summaries are packed into a bounded
   response.

The adaptive scan starts at 32 MiB and can expand to the configured
16–512 MiB ceiling. Source reads, AST work, concurrency, result counts, and
model-visible output are independently bounded. Caches retain verified
structural metadata and stamps, not source bodies or source-line signatures.

Semantic edges improve ranking; they are not compiler-authoritative type
resolution or LSP-grade references. Sippion does not execute an LSP, compiler,
build script, procedural macro, shell command, or repository code during
retrieval.

Implementation responsibilities are kept explicit:

- `src/repo.rs`: repository access, discovery, lexical retrieval, verification,
  caching, and structural ranking;
- `src/syntax.rs`: bounded Tree-sitter and source-only semantic extraction;
- `src/hybrid.rs`: BM25, symbols, PageRank, and excerpt compaction;
- `src/service.rs`: local service boundary and context assembly;
- `src/main.rs`: stdio MCP/JSON-RPC framing, dispatch, concurrency, and
  cancellation;
- `src/setup.rs`: idempotent client configuration and diagnostics;
- `src/core.rs`: MCP schema and bounded input/output policy.
