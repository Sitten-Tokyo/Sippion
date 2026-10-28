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
REQUIRED_TASKS = 12


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
        # Cached input is included in input_tokens (not an additional charge).
        return self.input_tokens + self.output_tokens


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

        input_tokens = _non_negative_int(row.get("input_tokens"), "input_tokens", line_number)
        cache_read_tokens = _non_negative_int(
            row.get("cache_read_tokens"), "cache_read_tokens", line_number
        )
        if cache_read_tokens > input_tokens:
            raise ValueError(
                f"line {line_number}: cache_read_tokens cannot exceed input_tokens"
            )

        run_id = _non_negative_int(row.get("run"), "run", line_number)
        if run_id != 1:
            raise ValueError(f"line {line_number}: run must be 1 (fixed pilot)")

        runs.append(
            Run(
                task=task,
                arm=arm,
                run=run_id,
                input_tokens=input_tokens,
                output_tokens=_non_negative_int(
                    row.get("output_tokens"), "output_tokens", line_number
                ),
                cache_read_tokens=cache_read_tokens,
                correctness=correctness,
            )
        )
    if not runs:
        raise ValueError("input contains no benchmark rows")
    return runs


def analyze(runs: list[Run]) -> dict[str, object]:

    grouped: dict[tuple[str, str], list[Run]] = defaultdict(list)
    tasks = sorted({run.task for run in runs})
    if len(tasks) != REQUIRED_TASKS:
        raise ValueError(f"expected {REQUIRED_TASKS} tasks, found {len(tasks)}")
    for run in runs:
        grouped[(run.task, run.arm)].append(run)

    task_results: dict[str, dict[str, dict[str, object]]] = {}
    for task in tasks:
        task_results[task] = {}
        for arm in ARMS:
            group = grouped.get((task, arm), [])
            if len(group) > 1:
                raise ValueError(f"{task}/{arm}: duplicate or extra execution")
            if not group:
                raise ValueError(f"{task}/{arm}: expected 1 run, found 0")
            item = group[0]
            if item.run != 1:
                raise ValueError(f"{task}/{arm}: run must be 1 (fixed pilot)")
            task_results[task][arm] = {
                "correct": item.correctness,
                "median_total_tokens": item.total_tokens,
                "runs": 1,
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
    return {"expected_runs": 1, "exploratory": True, "tasks": task_results, "arms": arms, "winner": winner}


def render_markdown(result: dict[str, object]) -> str:
    arms = result["arms"]
    assert isinstance(arms, dict)
    lines = [
        "| Arm | Correctness gate | Median task tokens | vs baseline (exploratory) |",
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
    lines.extend(["", f"Provisional leader: {winner if winner is not None else 'none (correctness gate failed)'}"])
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Score Sippion efficiency runs using correctness gates and total tokens."
    )
    parser.add_argument("input", type=Path, help="JSONL benchmark results")
    parser.add_argument("--format", choices=("markdown", "json"), default="markdown")
    args = parser.parse_args()

    try:
        with args.input.open(encoding="utf-8") as handle:
            result = analyze(parse_runs(handle))
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
