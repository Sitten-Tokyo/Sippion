# Efficiency benchmark pilot

This directory contains the real-Codex pilot for Sippion's efficiency layer. The
only product metrics are correctness and total tokens. Input, cached-input, and
output tokens are retained for diagnosis; cost, duration, LOC, file count,
dependency count, clarification count, and subjective scores are not metrics.

## Matrix and arms

`tasks.json` contains 12 real-OSS tasks pinned to immutable commit SHAs:

- 4 TypeScript/React tasks
- 4 Python tasks
- 4 Rust tasks
- at least one monorepo

The task categories cover bug fixes, features, refactors, reviews, dependency
and abstraction temptations, both kinds of ambiguity, and security-sensitive
work. Every verifier is deterministic; no LLM judge is used.

Each task runs five times under each arm (240 executions total):

1. `baseline`: no Sippion MCP and no Sippion rule.
2. `retrieval`: only the `repo_context` MCP tool.
3. `minimal-build`: retrieval plus the smallest-correct-solution rule.
4. `full`: retrieval plus the complete managed efficiency rule.

The runner writes each run in a fresh checkout, private HOME/cache/temp
directories, and temporary `CODEX_HOME`. It checks out the manifest commit
before setup and never changes the user's global Codex configuration.

## Prerequisites and authentication

- Python 3.10 or newer
- Git
- Codex CLI (`codex exec --json`)
- A release Sippion binary for retrieval arms

Authentication can come from an already authenticated local Codex CLI or from
`OPENAI_API_KEY`. The runner accepts either, never puts credentials in command
arguments or result rows, and never prints credential-related command output.
Missing authentication fails closed. The manual GitHub Actions pilot requires
the repository `OPENAI_API_KEY` secret; it does not run on pull requests.

Build Sippion before a real run:

```sh
cargo build --release --locked
```

## Dry-run

Dry-run performs no model call and validates the complete matrix, duplicate
keys, pinned revisions, verifier presence, arm-specific configuration, and
output path:

```sh
python3 eval/efficiency/run_pilot.py --dry-run
```

The default report must show `12` tasks, `4` arms, `5` runs per task/arm, and
`240` executions. A filtered dry-run is useful for checking one task or arm.

## Real Codex runs

Run one smoke execution:

```sh
python3 eval/efficiency/run_pilot.py \
  --task rust-01 --arm baseline --runs 1 \
  --sippion-binary target/release/sippion
```

Run the full pilot:

```sh
python3 eval/efficiency/run_pilot.py \
  --runs 5 --output eval/efficiency/results.jsonl \
  --sippion-binary target/release/sippion
```

The runner stops on missing fixtures, missing verifiers, failed setup, missing
Codex usage, malformed JSONL, or an unavailable CLI. A verifier failure is a
real row with `correctness: false`; it disqualifies that task/arm regardless of
token savings.

Resume uses the same output file. Valid existing `(task, arm, run)` rows are
reused and only missing executions are run. Duplicate, conflicting, malformed,
unknown, or schema-incomplete rows fail closed; existing rows are never
silently overwritten.

## Result schema and usage accounting

Every row has exactly this schema:

```json
{"task":"rust-01","arm":"full","run":1,"input_tokens":1200,"output_tokens":300,"cache_read_tokens":100,"correctness":true}
```

The runner parses every Codex JSONL `turn.completed` event and sums its usage
for the session. It requires `input_tokens`, `output_tokens`, and cached-input
usage (flat `cached_input_tokens` or nested
`input_tokens_details.cached_tokens`). It never uses the first event alone and
never guesses a missing field. The scorer's definition remains:

```text
total_tokens = input_tokens + output_tokens + cache_read_tokens
```

## Scoring and correctness gate

Run the deterministic scorer after a complete result set:

```sh
python3 eval/efficiency/score.py eval/efficiency/results.jsonl
python3 eval/efficiency/score.py eval/efficiency/results.jsonl --format json
```

The scorer takes the median total tokens per task/arm, then the median across
task medians. An arm is eligible only when every run for every task passes the
correctness gate. Lower token use cannot compensate for a failed verifier.

## Tests and CI

No model call is made by these checks:

```sh
python3 eval/efficiency/score_test.py
python3 eval/efficiency/run_pilot_test.py
python3 eval/efficiency/run_pilot.py --dry-run
```

Pull-request CI runs syntax checks, deterministic unit tests, and the 240-run
dry-run only. The real pilot workflow is `workflow_dispatch` only and uploads
the JSONL results as an artifact. It fails clearly when the Actions secret is
missing.

At this revision no complete real-Codex pilot result is recorded, so this
README makes no token-reduction claim.
