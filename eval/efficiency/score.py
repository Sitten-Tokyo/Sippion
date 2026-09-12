#!/usr/bin/env python3
"""Score Sippion efficiency benchmark JSONL without model judges."""

from __future__ import annotations

import argparse
import json
import statistics
import sys
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

ARMS = ("baseline", "retrieval", "minimal-build", "full")


@dataclass(frozen=True)
class Run:
    task: str
    arm: str
    run: int
    input_tokens: int
    output_tokens: int
    cache_read_tokens: int
    correctness: bool

    @property
    def total_tokens(self) -> int:
        return self.input_tokens + self.output_tokens + self.cache_read_tokens


def _non_negative_int(value: object, field: str, line: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise ValueError(f"line {line}: {field} must be a non-negative integer")
    return value


def parse_runs(lines: Iterable[str]) -> list[Run]:
    runs: list[Run] = []
    for line_number, raw in enumerate(lines, 1):
        raw = raw.strip()
        if not raw:
            continue
        try:
            row = json.loads(raw)
        except json.JSONDecodeError as error:
            raise ValueError(f"line {line_number}: invalid JSON: {error.msg}") from error
        if not isinstance(row, dict):
            raise ValueError(f"line {line_number}: row must be a JSON object")

        task = row.get("task")
        arm = row.get("arm")
        correctness = row.get("correctness")
        if not isinstance(task, str) or not task.strip():
            raise ValueError(f"line {line_number}: task must be a non-empty string")
        if arm not in ARMS:
            raise ValueError(f"line {line_number}: arm must be one of {', '.join(ARMS)}")
        if not isinstance(correctness, bool):
            raise ValueError(f"line {line_number}: correctness must be boolean")

        runs.append(
            Run(
                task=task,
                arm=arm,
                run=_non_negative_int(row.get("run"), "run", line_number),
                input_tokens=_non_negative_int(
                    row.get("input_tokens"), "input_tokens", line_number
                ),
                output_tokens=_non_negative_int(
                    row.get("output_tokens"), "output_tokens", line_number
                ),
                cache_read_tokens=_non_negative_int(
                    row.get("cache_read_tokens"), "cache_read_tokens", line_number
                ),
                correctness=correctness,
            )
        )
    if not runs:
        raise ValueError("input contains no benchmark rows")
    return runs


def analyze(runs: list[Run], expected_runs: int = 5) -> dict[str, object]:
    if expected_runs < 1:
        raise ValueError("expected_runs must be positive")

    grouped: dict[tuple[str, str], list[Run]] = defaultdict(list)
    tasks = sorted({run.task for run in runs})
    for run in runs:
        grouped[(run.task, run.arm)].append(run)

    task_results: dict[str, dict[str, dict[str, object]]] = {}
    for task in tasks:
        task_results[task] = {}
        for arm in ARMS:
            group = grouped.get((task, arm), [])
            if len(group) != expected_runs:
                raise ValueError(
                    f"{task}/{arm}: expected {expected_runs} runs, found {len(group)}"
                )
            run_ids = [item.run for item in group]
            if len(set(run_ids)) != len(run_ids):
                raise ValueError(f"{task}/{arm}: duplicate run id")
            totals = [item.total_tokens for item in group]
            task_results[task][arm] = {
                "correct": all(item.correctness for item in group),
                "median_total_tokens": statistics.median(totals),
                "runs": expected_runs,
            }

    arms: dict[str, dict[str, object]] = {}
    for arm in ARMS:
        medians = [
            float(task_results[task][arm]["median_total_tokens"]) for task in tasks
        ]
        arms[arm] = {
            "correct": all(bool(task_results[task][arm]["correct"]) for task in tasks),
            "median_task_tokens": statistics.median(medians),
            "tasks": len(tasks),
        }

    baseline = arms["baseline"]
    baseline_tokens = float(baseline["median_task_tokens"])
    for arm in ARMS[1:]:
        current = arms[arm]
        if not baseline["correct"] or not current["correct"]:
            current["vs_baseline_percent"] = None
            current["eligible"] = False
        else:
            current["eligible"] = True
            current_tokens = float(current["median_task_tokens"])
            current["vs_baseline_percent"] = (
                None
                if baseline_tokens == 0
                else (current_tokens - baseline_tokens) / baseline_tokens * 100.0
            )
    baseline["eligible"] = bool(baseline["correct"])
    baseline["vs_baseline_percent"] = 0.0 if baseline["correct"] else None

    eligible = [arm for arm in ARMS if arms[arm]["eligible"]]
    winner = (
        min(eligible, key=lambda arm: float(arms[arm]["median_task_tokens"]))
        if eligible
        else None
    )
    return {"expected_runs": expected_runs, "tasks": task_results, "arms": arms, "winner": winner}


def render_markdown(result: dict[str, object]) -> str:
    arms = result["arms"]
    assert isinstance(arms, dict)
    lines = [
        "| Arm | Correctness gate | Median total tokens | vs baseline |",
        "| --- | --- | ---: | ---: |",
    ]
    for arm in ARMS:
        row = arms[arm]
        assert isinstance(row, dict)
        gate = "PASS" if row["correct"] else "FAIL"
        tokens = row["median_task_tokens"]
        delta = row["vs_baseline_percent"]
        delta_text = "—" if delta is None else f"{float(delta):+.2f}%"
        lines.append(f"| {arm} | {gate} | {tokens:g} | {delta_text} |")
    winner = result["winner"]
    lines.extend(["", f"Winner: {winner if winner is not None else 'none (correctness gate failed)'}"])
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Score Sippion efficiency runs using correctness gates and total tokens."
    )
    parser.add_argument("input", type=Path, help="JSONL benchmark results")
    parser.add_argument("--runs", type=int, default=5, help="required runs per task/arm")
    parser.add_argument("--format", choices=("markdown", "json"), default="markdown")
    args = parser.parse_args()

    try:
        with args.input.open(encoding="utf-8") as handle:
            result = analyze(parse_runs(handle), args.runs)
    except (OSError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2

    if args.format == "json":
        print(json.dumps(result, indent=2, sort_keys=True))
    else:
        print(render_markdown(result))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
