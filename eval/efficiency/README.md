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

Each JSONL row records only the benchmark inputs needed for the two product metrics:

```json
{"task":"task-a","arm":"full","run":1,"input_tokens":1200,"output_tokens":300,"cache_read_tokens":100,"correctness":true}
```

`total_tokens` is computed as `input_tokens + output_tokens + cache_read_tokens`. Variance may be retained for diagnostics but is not a product success metric.

## Correctness gate

Use, in order:

1. existing repository tests
2. hidden deterministic task verifier
3. explicit security-regression checks where relevant

Do not use an LLM judge. Exclude tasks whose requested behavior cannot be evaluated mechanically.

Any correctness or security regression disqualifies an arm regardless of token savings. Among eligible arms, lower median total tokens wins. The scorer fails closed when a task/arm is missing the required run count or repeats a run id.

The repository retrieval evaluator accepts both legacy labeled context atoms and the compact model-visible format. Output compaction therefore remains subject to the same mechanically checked evidence requirements instead of weakening the retrieval gate.

## Scoring

With five runs per task/arm:

```sh
python3 eval/efficiency/score.py results.jsonl
```

Machine-readable output:

```sh
python3 eval/efficiency/score.py results.jsonl --format json
```

Scorer unit tests:

```sh
python3 eval/efficiency/score_test.py
```

The scorer first computes each task/arm's median total tokens, then reports the median across task medians. Baseline comparisons are emitted only for arms that pass the correctness gate. No model judge or subjective score participates in winner selection.

## Completed packed-atom ablation

The deterministic 10/6/4/3 ablation is complete. Results are stored in `atom-ablation.json`:

- `10`: pass
- `6`: pass
- `4`: fail
- `3`: fail

The selected production cap is **6**, the smallest tested value that preserves the correctness/evidence gate. At cap 4, packed expected-path recall was `0.929` against the required `1.000`; cap 3 also failed the deterministic retrieval gate. Token savings cannot override those failures.

## Remaining model ablations

The end-task Codex pilot still needs to measure:

- managed rule sentence removal
- MCP server instruction sentence removal
- model-visible context metadata removal
- stronger same-path deduplication
- smaller first-call context with progressive second-call retrieval

A progressive retrieval variant is only a win when total session tokens fall, not merely when the first tool response is smaller.

Real Codex runs require an `OPENAI_API_KEY` in the execution environment. Missing credentials must fail closed; do not replace Codex with a different model or synthetic usage estimate. Do not publish token-reduction claims until real model runs have completed under this protocol.
