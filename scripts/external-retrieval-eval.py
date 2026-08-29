#!/usr/bin/env python3
import argparse
import json
import statistics
import subprocess
import tempfile
from pathlib import Path


def percentile(values, p):
    if not values:
        return 0.0
    ordered = sorted(values)
    index = (len(ordered) - 1) * p
    low = int(index)
    high = min(low + 1, len(ordered) - 1)
    fraction = index - low
    return ordered[low] * (1 - fraction) + ordered[high] * fraction


def run_checked(command, *, cwd=None, timeout=180):
    proc = subprocess.run(
        command,
        cwd=cwd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=timeout,
        check=False,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"command failed ({proc.returncode}): {' '.join(command)}\n{proc.stderr.strip()}"
        )
    return proc.stdout


def checkout_pinned(repo, destination):
    destination.mkdir(parents=True)
    run_checked(["git", "init", "--quiet"], cwd=destination)
    run_checked(["git", "remote", "add", "origin", repo["url"]], cwd=destination)
    run_checked(
        [
            "git",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "submodule.recurse=false",
            "fetch",
            "--quiet",
            "--depth=1",
            "origin",
            repo["commit"],
        ],
        cwd=destination,
        timeout=300,
    )
    run_checked(
        ["git", "-c", "core.hooksPath=/dev/null", "checkout", "--quiet", "--detach", "FETCH_HEAD"],
        cwd=destination,
    )
    actual = run_checked(["git", "rev-parse", "HEAD"], cwd=destination).strip()
    if actual != repo["commit"]:
        raise RuntimeError(f"{repo['name']}: expected {repo['commit']}, got {actual}")


def evaluate_case(binary, root, case):
    command = [binary, "query", "--root", str(root), "--json", "--", *case["query"].split()]
    payload = json.loads(run_checked(command, timeout=90))
    diagnostics = payload["diagnostics"]
    context = payload["context"]
    ranked = [entry["path"] for entry in diagnostics["ranked_files"]]
    expected = case["expectedPaths"]

    first_rank = None
    for path in expected:
        if path in ranked:
            rank = ranked.index(path) + 1
            first_rank = rank if first_rank is None else min(first_rank, rank)

    recall_at_5 = 1.0 if any(path in ranked[:5] for path in expected) else 0.0
    reciprocal_rank = 0.0 if first_rank is None else 1.0 / first_rank
    missing_anchors = [anchor for anchor in case.get("requiredAnchors", []) if anchor not in context]

    return {
        "name": case["name"],
        "query": case["query"],
        "recallAt5": recall_at_5,
        "reciprocalRank": reciprocal_rank,
        "estimatedTokens": diagnostics["estimated_tokens"],
        "scannedBytes": diagnostics["scanned_bytes"],
        "top5": ranked[:5],
        "missingAnchors": missing_anchors,
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--manifest", default="eval/external_repos.json")
    args = parser.parse_args()

    manifest = json.loads(Path(args.manifest).read_text(encoding="utf-8"))
    binary = str(Path(args.binary).resolve())
    results = []

    with tempfile.TemporaryDirectory(prefix="sippion-external-eval-") as temporary:
        root = Path(temporary)
        for repo in manifest["repositories"]:
            checkout = root / repo["name"]
            checkout_pinned(repo, checkout)
            for case in repo["cases"]:
                result = evaluate_case(binary, checkout, case)
                result["repository"] = repo["name"]
                results.append(result)
                print(json.dumps(result, sort_keys=True))

    recalls = [item["recallAt5"] for item in results]
    reciprocal_ranks = [item["reciprocalRank"] for item in results]
    token_counts = [item["estimatedTokens"] for item in results]
    summary = {
        "cases": len(results),
        "recallAt5": statistics.fmean(recalls) if recalls else 0.0,
        "mrr": statistics.fmean(reciprocal_ranks) if reciprocal_ranks else 0.0,
        "p95EstimatedTokens": percentile(token_counts, 0.95),
    }
    print(json.dumps({"summary": summary}, sort_keys=True))

    failures = []
    if summary["recallAt5"] < manifest["minRecallAt5"]:
        failures.append(
            f"Recall@5 {summary['recallAt5']:.3f} < {manifest['minRecallAt5']:.3f}"
        )
    if summary["mrr"] < manifest["minMrr"]:
        failures.append(f"MRR {summary['mrr']:.3f} < {manifest['minMrr']:.3f}")
    if summary["p95EstimatedTokens"] > manifest["maxP95EstimatedTokens"]:
        failures.append(
            f"p95 estimated tokens {summary['p95EstimatedTokens']:.0f} > {manifest['maxP95EstimatedTokens']}"
        )
    for item in results:
        if item["missingAnchors"]:
            failures.append(
                f"{item['repository']}/{item['name']}: missing anchors {item['missingAnchors']}"
            )

    if failures:
        raise SystemExit("external retrieval evaluation failed:\n- " + "\n- ".join(failures))


if __name__ == "__main__":
    main()
