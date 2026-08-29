#!/usr/bin/env python3
"""Offline train-only search for Sippion context/retrieval constants.

The tuner never mutates the caller's checkout. It copies the repository to a temporary directory,
changes only named packer/adaptive constants there, rebuilds Sippion, and scores the training suite.
After single-coordinate candidates are scored, the best distinct coordinates are combined pairwise.
Holdout is evaluated exactly once after train-only selection and never participates in selection.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import tempfile
from itertools import combinations
from pathlib import Path

DEFAULTS = {
    "structure_score": 0.70,
    "evidence_score": 1.20,
    "same_path_novelty": 0.42,
    "min_confidence_gain": 0.025,
    "min_new_hits_per_mib": 0.02,
}

SINGLE_CANDIDATES = [
    ("default", {}),
    ("more-structure", {"structure_score": 0.80}),
    ("less-structure", {"structure_score": 0.60}),
    ("more-evidence", {"evidence_score": 1.30}),
    ("less-evidence", {"evidence_score": 1.10}),
    ("more-diversity", {"same_path_novelty": 0.32}),
    ("less-diversity", {"same_path_novelty": 0.52}),
    ("expand-easier", {"min_confidence_gain": 0.015}),
    ("expand-harder", {"min_confidence_gain": 0.035}),
    ("accept-lower-yield", {"min_new_hits_per_mib": 0.01}),
    ("require-higher-yield", {"min_new_hits_per_mib": 0.04}),
]


def run(cmd, cwd=None, env=None):
    return subprocess.run(
        cmd,
        cwd=cwd,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )


def replace_once(text, pattern, replacement, label):
    updated, count = re.subn(pattern, replacement, text, count=1, flags=re.MULTILINE)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one replacement, got {count}")
    return updated


def patch_candidate(root, parameters):
    context_path = root / "src/service/context.rs"
    adaptive_path = root / "src/repo/adaptive.rs"
    context = context_path.read_text(encoding="utf-8")
    adaptive = adaptive_path.read_text(encoding="utf-8")

    context = replace_once(
        context,
        r"(structure_score:\s*)[0-9.]+(,)",
        rf"\g<1>{parameters['structure_score']:.2f}\g<2>",
        "structure_score",
    )
    context = replace_once(
        context,
        r"(evidence_score:\s*)[0-9.]+(,)",
        rf"\g<1>{parameters['evidence_score']:.2f}\g<2>",
        "evidence_score",
    )
    context = replace_once(
        context,
        r"(same_path_novelty:\s*)[0-9.]+(,)",
        rf"\g<1>{parameters['same_path_novelty']:.2f}\g<2>",
        "same_path_novelty",
    )
    adaptive = replace_once(
        adaptive,
        r"(const MIN_CONFIDENCE_GAIN_FOR_EXPANSION: f64 = )[0-9.]+(;)",
        rf"\g<1>{parameters['min_confidence_gain']:.3f}\g<2>",
        "MIN_CONFIDENCE_GAIN_FOR_EXPANSION",
    )
    adaptive = replace_once(
        adaptive,
        r"(const MIN_NEW_HITS_PER_MIB: f64 = )[0-9.]+(;)",
        rf"\g<1>{parameters['min_new_hits_per_mib']:.3f}\g<2>",
        "MIN_NEW_HITS_PER_MIB",
    )
    context_path.write_text(context, encoding="utf-8")
    adaptive_path.write_text(adaptive, encoding="utf-8")


def evaluate(
    repo,
    binary,
    cases,
    fixture,
    baseline_root=None,
    warmup_runs=0,
    measurement_runs=2,
):
    cmd = [
        "python3",
        str(repo / "scripts/retrieval-eval.py"),
        "--binary",
        str(binary),
        "--cases",
        str(cases),
        "--fixture",
        str(fixture),
        "--warmup-runs",
        str(warmup_runs),
        "--measurement-runs",
        str(measurement_runs),
    ]
    if baseline_root is not None:
        cmd.extend(["--baseline-root", str(baseline_root)])
    proc = run(cmd, cwd=repo)
    try:
        payload = json.loads(proc.stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError(
            f"evaluation produced invalid JSON: {error}; stderr={proc.stderr.strip()}"
        ) from error
    return payload, proc.returncode


def objective(summary, returncode):
    quality = (
        summary.get("recallAt5", 0.0) * 4.0
        + summary.get("mrr", 0.0) * 2.0
        + summary.get("packedExpectedPathRecall", 0.0) * 2.0
        + summary.get("requiredEvidenceAnchorRecall", 0.0) * 2.0
        + max(summary.get("recallDeltaVsBm25", 0.0), 0.0)
    )
    cost = (
        summary.get("averageEstimatedTokens", 0.0) / 1000.0
        + summary.get("p95LatencyMs", 0.0) / 5000.0
    )
    score = quality - cost * 0.4
    if returncode != 0 or summary.get("failures"):
        score -= 100.0
    return score


def dominates(left, right):
    left_quality = (
        left["train"]["requiredEvidenceAnchorRecall"],
        left["train"]["packedExpectedPathRecall"],
        left["train"]["recallAt5"],
        left["train"]["mrr"],
    )
    right_quality = (
        right["train"]["requiredEvidenceAnchorRecall"],
        right["train"]["packedExpectedPathRecall"],
        right["train"]["recallAt5"],
        right["train"]["mrr"],
    )
    quality_not_worse = all(a >= b for a, b in zip(left_quality, right_quality))
    cost_not_worse = (
        left["train"]["averageEstimatedTokens"] <= right["train"]["averageEstimatedTokens"]
        and left["train"]["p95LatencyMs"] <= right["train"]["p95LatencyMs"]
    )
    strictly_better = left_quality != right_quality or (
        left["train"]["averageEstimatedTokens"],
        left["train"]["p95LatencyMs"],
    ) != (
        right["train"]["averageEstimatedTokens"],
        right["train"]["p95LatencyMs"],
    )
    return quality_not_worse and cost_not_worse and strictly_better


def build_and_score(
    work,
    target_dir,
    original_context,
    original_adaptive,
    name,
    overrides,
    train_cases,
    train_fixture,
    train_baseline,
    warmup_runs,
    measurement_runs,
):
    (work / "src/service/context.rs").write_text(original_context, encoding="utf-8")
    (work / "src/repo/adaptive.rs").write_text(original_adaptive, encoding="utf-8")
    parameters = {**DEFAULTS, **overrides}
    patch_candidate(work, parameters)
    build = run(
        ["cargo", "build", "--release", "--locked"],
        cwd=work,
        env={**os.environ, "CARGO_TARGET_DIR": str(target_dir)},
    )
    if build.returncode != 0:
        return {
            "name": name,
            "overrides": overrides,
            "parameters": parameters,
            "buildFailed": True,
            "stderr": build.stderr[-4000:],
            "objective": -1000.0,
        }
    binary = target_dir / "release" / ("sippion.exe" if os.name == "nt" else "sippion")
    train, returncode = evaluate(
        work,
        binary,
        train_cases,
        train_fixture,
        train_baseline,
        warmup_runs,
        measurement_runs,
    )
    return {
        "name": name,
        "overrides": overrides,
        "parameters": parameters,
        "train": train,
        "trainExitCode": returncode,
        "objective": round(objective(train, returncode), 6),
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default=".")
    parser.add_argument("--train-cases", default="eval/self_train_cases.json")
    parser.add_argument("--train-fixture", default=".")
    parser.add_argument("--train-baseline-root", default=None)
    parser.add_argument("--holdout-cases", default=None)
    parser.add_argument("--holdout-fixture", default=None)
    parser.add_argument("--holdout-baseline-root", default=None)
    parser.add_argument("--candidate", action="append", default=None, help="limit single-coordinate seeds")
    parser.add_argument("--pairwise-top", type=int, default=4)
    parser.add_argument("--warmup-runs", type=int, default=0)
    parser.add_argument("--measurement-runs", type=int, default=2)
    args = parser.parse_args()

    if args.pairwise_top < 0:
        raise SystemExit("pairwise-top must be >= 0")
    if args.warmup_runs < 0 or args.measurement_runs < 1:
        raise SystemExit("warmup-runs must be >= 0 and measurement-runs must be >= 1")

    source_repo = Path(args.repo).resolve()
    train_cases = Path(args.train_cases).resolve()
    train_fixture = Path(args.train_fixture).resolve()
    train_baseline = Path(args.train_baseline_root).resolve() if args.train_baseline_root else None
    selected_names = set(args.candidate or [])

    single_specs = [
        (name, overrides)
        for name, overrides in SINGLE_CANDIDATES
        if not selected_names or name in selected_names
    ]
    if not single_specs:
        raise SystemExit("no matching tuning candidates")

    records = []
    with tempfile.TemporaryDirectory(prefix="sippion-context-tune-") as temp_dir:
        work = Path(temp_dir) / "repo"
        shutil.copytree(
            source_repo,
            work,
            ignore=shutil.ignore_patterns(".git", "target", "node_modules", "*.mcpb"),
        )
        original_context = (work / "src/service/context.rs").read_text(encoding="utf-8")
        original_adaptive = (work / "src/repo/adaptive.rs").read_text(encoding="utf-8")
        target_dir = Path(temp_dir) / "target"

        for name, overrides in single_specs:
            records.append(
                build_and_score(
                    work,
                    target_dir,
                    original_context,
                    original_adaptive,
                    name,
                    overrides,
                    train_cases,
                    train_fixture,
                    train_baseline,
                    args.warmup_runs,
                    args.measurement_runs,
                )
            )

        viable_singles = [
            record
            for record in records
            if "train" in record and not record.get("train", {}).get("failures")
        ]
        non_default = [record for record in viable_singles if record["overrides"]]
        non_default.sort(key=lambda record: record["objective"], reverse=True)
        pairwise_seeds = non_default[: args.pairwise_top]

        for left, right in combinations(pairwise_seeds, 2):
            if set(left["overrides"]).intersection(right["overrides"]):
                continue
            overrides = {**left["overrides"], **right["overrides"]}
            name = f"pair:{left['name']}+{right['name']}"
            records.append(
                build_and_score(
                    work,
                    target_dir,
                    original_context,
                    original_adaptive,
                    name,
                    overrides,
                    train_cases,
                    train_fixture,
                    train_baseline,
                    args.warmup_runs,
                    args.measurement_runs,
                )
            )

        viable = [record for record in records if "train" in record]
        frontier = [
            record["name"]
            for record in viable
            if not any(other is not record and dominates(other, record) for other in viable)
        ]
        recommended = max(viable, key=lambda record: record["objective"]) if viable else None

        holdout = None
        if recommended and args.holdout_cases and args.holdout_fixture:
            (work / "src/service/context.rs").write_text(original_context, encoding="utf-8")
            (work / "src/repo/adaptive.rs").write_text(original_adaptive, encoding="utf-8")
            patch_candidate(work, recommended["parameters"])
            build = run(
                ["cargo", "build", "--release", "--locked"],
                cwd=work,
                env={**os.environ, "CARGO_TARGET_DIR": str(target_dir)},
            )
            if build.returncode == 0:
                binary = target_dir / "release" / (
                    "sippion.exe" if os.name == "nt" else "sippion"
                )
                holdout, holdout_exit = evaluate(
                    work,
                    binary,
                    Path(args.holdout_cases).resolve(),
                    Path(args.holdout_fixture).resolve(),
                    Path(args.holdout_baseline_root).resolve()
                    if args.holdout_baseline_root
                    else None,
                    args.warmup_runs,
                    args.measurement_runs,
                )
                holdout["exitCode"] = holdout_exit

    output = {
        "selectionUsesHoldout": False,
        "selectionStages": ["single-coordinate train", "pairwise train", "one post-selection holdout"],
        "objective": (
            "quality - 0.4*(averageTokens/1000 + p95MedianLatencyMs/5000); "
            "failing train gates are penalized"
        ),
        "pairwiseSeeds": [record["name"] for record in pairwise_seeds],
        "paretoFrontier": frontier,
        "recommended": recommended["name"] if recommended else None,
        "recommendedParameters": recommended["parameters"] if recommended else None,
        "recommendedHoldout": holdout,
        "candidates": records,
    }
    print(json.dumps(output, indent=2))


if __name__ == "__main__":
    main()
