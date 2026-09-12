# Efficiency benchmark pilot

This benchmark validates Sippion's efficiency layer with two hard outcomes: correctness and token use.

## Arms

1. `baseline` — no Sippion behavior.
2. `retrieval` — repository retrieval only.
3. `minimal-build` — retrieval plus smallest-correct-solution behavior.
4. `full` — minimal-build plus compact decision/output behavior.

## Pilot matrix

Use 12 mechanically verifiable tasks:

- 4 TypeScript/React
- 4 Python
- 4 Rust

Include at least one monorepo. Cover bug fixes, features, refactors, reviews, dependency temptations, abstraction temptations, ambiguity, and security. Include both ambiguity cases where asking one question is correct and cases where choosing a reasonable default is correct.

Run each task/arm combination five times. Primary aggregation is median.

## Required measurements

Per session record:

- input tokens
- output tokens
- cache-read tokens
- total tokens
- API cost
- correctness result

Variance may be retained for diagnostics but is not a product success metric.

## Correctness gate

Use, in order:

1. existing repository tests
2. hidden deterministic task verifier
3. explicit security-regression checks where relevant

Do not use an LLM judge. Exclude tasks whose requested behavior cannot be evaluated mechanically.

Any correctness or security regression disqualifies an arm regardless of token savings. Among equally correct arms, lower median total tokens wins.

## Ablations

At minimum test:

- managed rule sentence removal
- MCP server instruction sentence removal
- model-visible context metadata removal
- packed atom limits: 10, 6, 4, 3
- stronger same-path deduplication
- smaller first-call context with progressive second-call retrieval

A progressive retrieval variant is only a win when total session tokens fall, not merely when the first tool response is smaller.
