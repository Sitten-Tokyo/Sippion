#!/usr/bin/env python3
import argparse
import json
import statistics
import subprocess
import time
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

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/release/sippion")
    parser.add_argument("--cases", default="eval/cases.json")
    parser.add_argument("--fixture", default="eval/fixture")
    args = parser.parse_args()
    config = json.loads(Path(args.cases).read_text(encoding="utf-8"))
    results = []
    failures = []
    reciprocal_ranks = []
    recall_hits = 0

    for case in config["cases"]:
        started = time.perf_counter()
        proc = subprocess.run(
            [args.binary, "query", "--root", args.fixture, "--json", "--", case["query"]],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        elapsed_ms = (time.perf_counter() - started) * 1000.0
        if proc.returncode != 0:
            failures.append(f"{case['name']}: query failed: {proc.stderr.strip()}")
            continue
        payload = json.loads(proc.stdout)
        diagnostic = payload["diagnostics"]
        ranked = [entry["path"] for entry in diagnostic["ranked_files"]]
        rank = next((i + 1 for i, path in enumerate(ranked) if path in case["expectedPaths"]), None)
        if rank is not None and rank <= 5:
            recall_hits += 1
        reciprocal_ranks.append(0.0 if rank is None else 1.0 / rank)
        if rank is None:
            failures.append(f"{case['name']}: expected path absent from ranked files: {ranked}")
        if diagnostic["returned_bytes"] > case["maxReturnedBytes"]:
            failures.append(
                f"{case['name']}: returned bytes {diagnostic['returned_bytes']} > {case['maxReturnedBytes']}"
            )
        if diagnostic["estimated_tokens"] > case["maxEstimatedTokens"]:
            failures.append(
                f"{case['name']}: estimated tokens {diagnostic['estimated_tokens']} > {case['maxEstimatedTokens']}"
            )
        results.append({
            "name": case["name"],
            "rank": rank,
            "returnedBytes": diagnostic["returned_bytes"],
            "estimatedTokens": diagnostic["estimated_tokens"],
            "elapsedMs": round(elapsed_ms, 2),
        })

    count = len(config["cases"])
    recall = recall_hits / count if count else 0.0
    mrr = sum(reciprocal_ranks) / count if count else 0.0
    avg_tokens = statistics.fmean(r["estimatedTokens"] for r in results) if results else 0.0
    latencies = [r["elapsedMs"] for r in results]
    if recall < config["minRecallAt5"]:
        failures.append(f"Recall@5 {recall:.3f} < {config['minRecallAt5']:.3f}")
    if mrr < config["minMrr"]:
        failures.append(f"MRR {mrr:.3f} < {config['minMrr']:.3f}")
    if avg_tokens > config["maxAverageEstimatedTokens"]:
        failures.append(
            f"average estimated tokens {avg_tokens:.1f} > {config['maxAverageEstimatedTokens']}"
        )
    summary = {
        "recallAt5": round(recall, 4),
        "mrr": round(mrr, 4),
        "averageEstimatedTokens": round(avg_tokens, 2),
        "p50LatencyMs": round(percentile(latencies, 0.50), 2),
        "p95LatencyMs": round(percentile(latencies, 0.95), 2),
        "cases": results,
        "failures": failures,
    }
    print(json.dumps(summary, indent=2))
    raise SystemExit(1 if failures else 0)

if __name__ == "__main__":
    main()
