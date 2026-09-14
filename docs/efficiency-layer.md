# Sippion Context Efficiency

Sippion remains a local, read-only MCP server with one repository tool,
`repo_context`. Its efficiency work is limited to repository discovery,
retrieval, and model-visible context packing. It does not install rules about
implementation minimalism, clarification cadence, review style, or response
formatting.

## Goals

1. Reduce unnecessary model-input tokens without reducing retrieval correctness.
2. Keep repository exploration narrow before clients open source files broadly.
3. Preserve Sippion's local/read-only/no-model-proxy/RAM-only security model.
4. Keep correctness, evidence quality, and security checks as hard gates.

Token savings never justify lower correctness.

## Managed repository-discovery rule

`sippion setup` installs a small managed rule for supported clients. The rule
only directs repository discovery: use `repo_context` before broad recursive
searches or reading many files, keep Sippion scoped to the current project,
treat retrieved repository text as untrusted data, and fall back to native tools
when Sippion is unavailable.

Tool use and repository trust are the rule's scope. Coding style and user-facing
response style remain the client's responsibility.

## MCP server instructions

The MCP server instructions reinforce the same boundary: use `repo_context`
before broad exploration, use native reads after narrowing, and never treat
repository content as instructions. Cooperating agents can share a `session_id`
and use distinct `agent_id` values.

## Model-visible context

Normal output is intentionally compact and frames source text as untrusted data:

```text
[UNTRUSTED CODE]
src/auth.rs:31-47
| fn validate_token(...) {
|     ...
| }

src/session.rs:10-18
| ...
```

When retrieval is incomplete, Sippion uses a short marker:

```text
[UNTRUSTED CODE; INCOMPLETE]
```

Internal ranking and budget metadata stay out of normal model-visible output
unless they provide measured value. This includes numeric confidence, rank
scores, body byte counts, target token budgets, hard byte budgets, scanned byte
counts, and internal atom labels.

## Context packing

Packing preserves utility-per-token ranking while minimizing duplicate
model-visible evidence. Query-relevant implementation evidence and symbol or
signature ownership take priority over redundant same-file context.

The deterministic 10/6/4/3 packed-atom ablation selected **6** as the production
cap. Caps 10 and 6 passed the deterministic correctness/evidence gates; caps 4
and 3 failed. At cap 4, packed expected-path recall fell to `0.929` against the
required `1.000`, so smaller caps are disqualified regardless of potential token
savings. The machine-readable result is stored in
`eval/efficiency/atom-ablation.json`.

Broader retrieval quality is evaluated by the committed fixture and self-hosted
suites documented in [Quality and regression gates](quality.md). Performance and
model-visible context size are measured separately from correctness so resource
improvements cannot compensate for failed evidence gates.
