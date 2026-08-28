# Architecture

Sippion exposes one public MCP operation, `repo_context`. The local stdio
transport passes a validated request to the in-process service boundary.

## Optimization objective

Sippion's primary optimization objective is to organize and bound repository
context **before it is passed to an AI model**. By narrowing repository evidence
before broad file reads, Sippion aims to reduce unnecessary model-input token
consumption while preserving enough relevant context for the agent to locate and
understand the code that matters.

Actual tokenization is model/provider-specific, so Sippion does not claim an
exact provider token count. It uses a local estimated-token target as a soft
packing goal and an independent byte limit as the hard model-visible output
guard. `scripts/token-calibration.py` can compare that local estimator with real
token counts collected from a target provider/model; Sippion does not fabricate
provider counts for calibration.

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
RepositoryEngine
      |
      +--> RepositoryAccess      secure discovery/read/verification
      +--> adaptive retrieval    lexical candidate generation + marginal stopping
      +--> structural mapping    Tree-sitter + source-only semantic expansion
      `--> ContextAtom packer    utility / estimated-token optimization
```

## Retrieval pipeline

Retrieval is bounded and staged:

1. a RAM-only incremental lexical index produces initial candidates with BM25,
   path ranking, and optional volatile `session_id` / `agent_id` coordination;
2. adaptive scan grants start at 32 MiB and may expand toward the configured
   16–512 MiB ceiling only while coverage is incomplete, confidence is low, and
   the previous round still produced useful marginal evidence;
3. exact candidate verification captures source identity and a content
   fingerprint before any excerpt can become model-visible;
4. verified source files up to the request-local snapshot ceiling can be reused
   within the same `repo_context` call for structural analysis, avoiding a
   second full read on supported platforms; source bodies are never placed in a
   cross-request cache;
5. Tree-sitter and source-only semantic extraction analyze only a bounded set of
   ranked candidates, then one bounded semantic/import expansion can admit
   relevant structural neighbors that did not contain the original lexical
   query terms;
6. structural facts and verified excerpts become `ContextAtom` candidates;
   packing greedily maximizes marginal utility per estimated token while
   discounting redundant same-file evidence and respecting both the token target
   and hard byte limit.

The result is a compact evidence pack rather than a fixed structure/excerpt
percentage split. Narrow, high-confidence queries therefore do not pay for
structural metadata merely because a fixed quota was reserved for it, while
architecture-oriented queries can spend more of the same bounded budget on
symbols and semantic links when those atoms provide better value per token.

Retrieval ranking and model-visible packing are represented separately inside the service.
`ContextResult` carries typed diagnostics (retrieval-ranked files, packed paths, scan bytes,
confidence, adaptive rounds, and output budgets) alongside the final text. CLI evaluation
consumes those typed fields directly instead of parsing the compact `CTX` / `S` / `E`
representation, so output-format changes cannot silently redefine retrieval quality.

## Safety and cache boundaries

Source reads, AST work, concurrency, scan size, result counts, semantic
expansion, request-local snapshots, and model-visible output are independently
bounded. Persistent or cross-request caches retain verified structural metadata,
lexical statistics, graph facts, and source stamps—not source bodies or
source-line signatures.

Request-local source snapshots are an optimization, not a new authority. Before
a snapshot is reused, the current file generation is checked against the
verified source stamp and content fingerprint. Windows retains the stricter
re-read path where the supported metadata surface cannot safely establish the
same cross-stage identity guarantee.

Semantic edges improve ranking; they are not compiler-authoritative type
resolution or LSP-grade references. Bounded semantic expansion reads through the
same project-scoped repository capability and path policy as ordinary retrieval.
Sippion does not execute an LSP, compiler, build script, procedural macro, shell
command, or repository code during retrieval.

## Evaluation objective

Retrieval quality is evaluated together with context cost. The fixture suite and
self-hosted Sippion-repository suite measure retrieval Recall@5/MRR separately from packed
expected-path recall, packed unnecessary-file ratio, latency, estimated tokens, and
relevant packed paths per 1,000 returned tokens. Token cost is compared with three
baselines: all searchable source, opening the top-ranked files in full, and deterministic
grep-style line windows. A change that reduces output size but loses required evidence is
therefore not considered an optimization.

Provider-specific tokenizer calibration is intentionally separate from the hard
byte guard. Maintainers can collect authoritative token counts for representative
Sippion outputs and run:

```sh
python3 scripts/token-calibration.py observations.json
```

The script reports estimation error, underestimation rate, and a conservative
p95 multiplier candidate. Provider observations are external test data and
should not be invented or committed as if they were authoritative.

## Implementation responsibilities

- `src/repo.rs` and `src/repo/`: secure repository access, discovery, lexical
  retrieval, verification, adaptive scan control, coordination, redaction,
  request-local snapshot reuse, structural mapping, and bounded semantic
  expansion;
- `src/syntax.rs`: bounded Tree-sitter and source-only semantic extraction;
- `src/hybrid.rs`: BM25, symbols, PageRank, and conservative excerpt compaction;
- `src/service/engine.rs`: retrieval orchestration below the MCP boundary;
- `src/service/context.rs`: compact model-visible `ContextAtom` construction and
  utility-per-token packing;
- `src/service.rs`: local service interface and repository-error translation;
- `src/main.rs`: CLI entry points and local diagnostics;
- `src/mcp.rs`: stdio MCP/JSON-RPC framing, dispatch, concurrency, cancellation,
  and rate limiting;
- `src/setup.rs`: idempotent client configuration and diagnostics;
- `src/core.rs`: MCP schema and bounded input/output policy.
