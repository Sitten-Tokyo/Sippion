#!/usr/bin/env python3
"""Run a small baseline-vs-Sippion token benchmark.

This is a smoke benchmark, not a replacement for the full 240-execution pilot.
It reuses the pinned tasks, isolated runner, deterministic verifiers, and Codex
usage accounting from ``run_pilot.py`` while reducing the matrix to six model
calls: three tasks x two arms x one run.
"""

from __future__ import annotations

import argparse
import json
import statistics
import sys
from pathlib import Path
from typing import Any, Sequence

import run_pilot
import score

QUICK_TASKS = ("react-02", "py-02", "rust-01")
QUICK_ARMS = ("baseline", "retrieval")
DEFAULT_OUTPUT = Path("eval/efficiency/quick-results.jsonl")


def _percent_saved(baseline: int, retrieval: int) -> float | None:
    if baseline == 0:
        return None
    return (baseline - retrieval) / baseline * 100.0


def analyze_quick(runs: list[score.Run]) -> dict[str, Any]:
    selected = [
        run for run in runs if run.task in QUICK_TASKS and run.arm in QUICK_ARMS
    ]
    by_pair: dict[tuple[str, str], score.Run] = {}
    for run in selected:
        if run.run != 1:
            raise ValueError(
                f"{run.task}/{run.arm}: quick benchmark requires exactly run id 1"
            )
        key = (run.task, run.arm)
        if key in by_pair:
            raise ValueError(f"{run.task}/{run.arm}: duplicate quick benchmark row")
        by_pair[key] = run

    expected = {(task, arm) for task in QUICK_TASKS for arm in QUICK_ARMS}
    missing = sorted(expected - set(by_pair))
    if missing:
        labels = ", ".join(f"{task}/{arm}" for task, arm in missing)
        raise ValueError(f"quick benchmark is incomplete; missing {labels}")

    tasks: dict[str, dict[str, Any]] = {}
    baseline_totals: list[int] = []
    retrieval_totals: list[int] = []
    all_correct = True
    for task in QUICK_TASKS:
        baseline = by_pair[(task, "baseline")]
        retrieval = by_pair[(task, "retrieval")]
        baseline_total = baseline.total_tokens
        retrieval_total = retrieval.total_tokens
        comparable = baseline.correctness and retrieval.correctness
        all_correct = all_correct and comparable
        baseline_totals.append(baseline_total)
        retrieval_totals.append(retrieval_total)
        tasks[task] = {
            "baseline_total_tokens": baseline_total,
            "retrieval_total_tokens": retrieval_total,
            "baseline_correct": baseline.correctness,
            "retrieval_correct": retrieval.correctness,
            "comparable": comparable,
            "saved_tokens": baseline_total - retrieval_total if comparable else None,
            "saved_percent": (
                _percent_saved(baseline_total, retrieval_total) if comparable else None
            ),
        }

    baseline_sum = sum(baseline_totals)
    retrieval_sum = sum(retrieval_totals)
    return {
        "tasks": tasks,
        "overall": {
            "correctness_gate": all_correct,
            "baseline_total_tokens": baseline_sum,
            "retrieval_total_tokens": retrieval_sum,
            "saved_tokens": baseline_sum - retrieval_sum if all_correct else None,
            "saved_percent": (
                _percent_saved(baseline_sum, retrieval_sum) if all_correct else None
            ),
            "baseline_median_task_tokens": statistics.median(baseline_totals),
            "retrieval_median_task_tokens": statistics.median(retrieval_totals),
        },
    }


def load_and_analyze(path: Path) -> dict[str, Any]:
    try:
        with path.open(encoding="utf-8") as handle:
            runs = score.parse_runs(handle)
    except OSError as error:
        raise ValueError(f"cannot read quick benchmark results: {error}") from error
    return analyze_quick(runs)


def render_markdown(result: dict[str, Any]) -> str:
    lines = [
        "| Task | Baseline tokens | Sippion tokens | Saved | Correctness |",
        "| --- | ---: | ---: | ---: | --- |",
    ]
    tasks = result["tasks"]
    for task in QUICK_TASKS:
        row = tasks[task]
        if row["saved_percent"] is None:
            saved = "—"
        else:
            saved = f"{row['saved_percent']:+.2f}%"
        gate = "PASS" if row["comparable"] else "FAIL"
        lines.append(
            f"| {task} | {row['baseline_total_tokens']} | "
            f"{row['retrieval_total_tokens']} | {saved} | {gate} |"
        )

    overall = result["overall"]
    lines.append("")
    if overall["correctness_gate"]:
        lines.append(
            "Overall: "
            f"{overall['baseline_total_tokens']} -> {overall['retrieval_total_tokens']} "
            f"tokens ({overall['saved_percent']:+.2f}% saved)."
        )
    else:
        lines.append(
            "Overall: not comparable because at least one baseline/retrieval pair "
            "failed the deterministic correctness gate."
        )
    lines.append(
        "Median task tokens: "
        f"{overall['baseline_median_task_tokens']:g} -> "
        f"{overall['retrieval_median_task_tokens']:g}."
    )
    return "\n".join(lines)


def _runner_args(args: argparse.Namespace, task: str, arm: str) -> list[str]:
    command = [
        "--task",
        task,
        "--arm",
        arm,
        "--runs",
        "1",
        "--output",
        str(args.output),
        "--codex",
        args.codex,
    ]
    if args.model:
        command.extend(["--model", args.model])
    if args.sippion_binary:
        command.extend(["--sippion-binary", str(args.sippion_binary)])
    if args.work_dir:
        command.extend(["--work-dir", str(args.work_dir)])
    return command


def run_quick(args: argparse.Namespace) -> int:
    for task in QUICK_TASKS:
        for arm in QUICK_ARMS:
            exit_code = run_pilot.main(_runner_args(args, task, arm))
            if exit_code != 0:
                return exit_code
    return 0


def dry_run_report() -> dict[str, Any]:
    tasks = run_pilot.load_manifest(run_pilot.DEFAULT_MANIFEST)
    available = {task["id"] for task in tasks}
    missing = [task for task in QUICK_TASKS if task not in available]
    if missing:
        raise ValueError(f"quick benchmark tasks are missing from manifest: {missing}")
    return {
        "tasks": list(QUICK_TASKS),
        "arms": list(QUICK_ARMS),
        "runs_per_task_arm": 1,
        "planned_model_calls": len(QUICK_TASKS) * len(QUICK_ARMS),
        "model_calls": 0,
        "token_definition": "input_tokens + output_tokens + cache_read_tokens",
    }


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--codex", default="codex")
    parser.add_argument("--model", default=None)
    parser.add_argument("--sippion-binary", type=Path, default=None)
    parser.add_argument("--work-dir", type=Path, default=None)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--score-only", action="store_true")
    parser.add_argument("--format", choices=("markdown", "json"), default="markdown")
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    if args.dry_run and args.score_only:
        print("error: --dry-run and --score-only cannot be combined", file=sys.stderr)
        return 2

    try:
        if args.dry_run:
            print(json.dumps(dry_run_report(), indent=2, sort_keys=True))
            return 0

        if not args.score_only:
            exit_code = run_quick(args)
            if exit_code != 0:
                return exit_code

        result = load_and_analyze(args.output)
        if args.format == "json":
            print(json.dumps(result, indent=2, sort_keys=True))
        else:
            print(render_markdown(result))
        return 0
    except ValueError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
