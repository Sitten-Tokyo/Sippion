#!/usr/bin/env python3
import argparse
import json
import statistics
import subprocess
import time
from pathlib import Path

SOURCE_EXTENSIONS = {
    ".rs", ".py", ".pyi", ".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts",
    ".go", ".java", ".cs", ".c", ".cc", ".cpp", ".cxx", ".h", ".hh", ".hpp", ".hxx",
}
PRUNED_DIRS = {
    "node_modules", "target", "dist", "build", "coverage", ".venv", "venv", "__pycache__",
    ".next", "vendor", ".terraform", ".gradle", ".dart_tool", ".pytest_cache", ".ruff_cache",
}


def percentile(values, p):
    if not values:
        return 0.0
    ordered = sorted(values)
    index = (len(ordered) - 1) * p
    low = int(index)
    high = min(low + 1, len(ordered) - 1)
    fraction = index - low
    return ordered[low] * (1 - fraction) + ordered[high] * fraction


def estimated_tokens(binary, text, cache):
    cached = cache.get(text)
    if cached is not None:
        return cached
    proc = subprocess.run(
        [binary, "estimate-tokens", "--json"],
        input=text,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"canonical token estimator failed: {proc.stderr.strip()}")
    value = int(json.loads(proc.stdout)["estimatedTokens"])
    cache[text] = value
    return value


def source_corpus(root):
    root = Path(root)
    sources = {}
    for path in root.rglob("*"):
        if not path.is_file() or path.suffix.lower() not in SOURCE_EXTENSIONS:
            continue
        try:
            relative = path.relative_to(root)
        except ValueError:
            continue
        if any(part.lower() in PRUNED_DIRS for part in relative.parts[:-1]):
            continue
        try:
            if path.stat().st_size > 2 * 1024 * 1024:
                continue
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeError):
            continue
        sources[relative.as_posix()] = text
    return sources


def joined_token_cost(binary, texts, cache):
    return max(estimated_tokens(binary, "\n".join(texts), cache), 1)


def full_source_baseline(binary, sources, cache):
    return joined_token_cost(binary, sources.values(), cache), len(sources)


def top_k_full_files_baseline(binary, sources, ranked_paths, cache, k=5):
    selected = []
    seen = set()
    for path in ranked_paths:
        if path in seen or path not in sources:
            continue
        selected.append(path)
        seen.add(path)
        if len(selected) >= k:
            break
    tokens = joined_token_cost(binary, (sources[path] for path in selected), cache)
    return tokens, selected


def grep_window_baseline(binary, sources, query, cache, max_files=5, radius=4):
    terms = [part.casefold() for part in query.split() if len(part) >= 2]
    if not terms:
        return 1, []
    phrase = query.casefold()
    candidates = []
    for path, text in sources.items():
        folded = text.casefold()
        matched_terms = [term for term in terms if term in folded]
        if not matched_terms:
            continue
        lines = text.splitlines()
        best_index = 0
        best_line_matches = -1
        for index, line in enumerate(lines):
            folded_line = line.casefold()
            line_matches = sum(1 for term in terms if term in folded_line)
            if line_matches > best_line_matches:
                best_line_matches = line_matches
                best_index = index
        start = max(0, best_index - radius)
        end = min(len(lines), best_index + radius + 1)
        window = "\n".join(lines[start:end])
        score = len(matched_terms) * 10 + (5 if phrase and phrase in folded else 0)
        candidates.append((score, path, window))
    candidates.sort(key=lambda item: (-item[0], item[1]))
    selected = candidates[:max_files]
    tokens = joined_token_cost(binary, (window for _, _, window in selected), cache)
    return tokens, [path for _, path, _ in selected]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/release/sippion")
    parser.add_argument("--cases", default="eval/cases.json")
    parser.add_argument("--fixture", default="eval/fixture")
    parser.add_argument("--baseline-root", default=None)
    args = parser.parse_args()
    config = json.loads(Path(args.cases).read_text(encoding="utf-8"))
    baseline_root = args.baseline_root or args.fixture
    sources = source_corpus(baseline_root)
    token_cache = {}
    baseline_tokens, baseline_files = full_source_baseline(args.binary, sources, token_cache)

    results = []
    failures = []
    reciprocal_ranks = []
    recall_hits = 0
    expected_path_hits = 0
    expected_path_total = 0
    packed_expected_path_hits = 0
    required_anchor_hits = 0
    required_anchor_total = 0

    for case in config["cases"]:
        expected = set(case["expectedPaths"])
        required_anchors = case.get("requiredAnchors", [])
        expected_path_total += len(expected)
        required_anchor_total += len(required_anchors)
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
            reciprocal_ranks.append(0.0)
            continue
        payload = json.loads(proc.stdout)
        context = payload["context"]
        diagnostic = payload["diagnostics"]
        ranked = [entry["path"] for entry in diagnostic["ranked_files"]]
        packed = diagnostic.get("packed_paths", ranked)
        top5 = ranked[:5]
        relevant_at5 = [path for path in top5 if path in expected]
        packed_relevant = [path for path in packed if path in expected]
        matched_anchors = [anchor for anchor in required_anchors if anchor in context]
        missing_anchors = [anchor for anchor in required_anchors if anchor not in context]
        required_anchor_hits += len(matched_anchors)
        expected_path_hits += len(set(relevant_at5))
        packed_expected_path_hits += len(set(packed_relevant))
        rank = next((i + 1 for i, path in enumerate(ranked) if path in expected), None)
        if rank is not None and rank <= 5:
            recall_hits += 1
        reciprocal_ranks.append(0.0 if rank is None else 1.0 / rank)
        if rank is None:
            failures.append(f"{case['name']}: expected path absent from retrieval ranking: {ranked}")
        if case.get("requireAllExpectedAt5", False) and not expected.issubset(set(top5)):
            missing = sorted(expected.difference(top5))
            failures.append(f"{case['name']}: expected paths missing from top 5: {missing}; ranked={ranked}")
        if missing_anchors:
            failures.append(
                f"{case['name']}: required evidence anchors missing from model-visible context: {missing_anchors}"
            )
        if diagnostic["returned_bytes"] > case["maxReturnedBytes"]:
            failures.append(
                f"{case['name']}: returned bytes {diagnostic['returned_bytes']} > {case['maxReturnedBytes']}"
            )
        if diagnostic["estimated_tokens"] > case["maxEstimatedTokens"]:
            failures.append(
                f"{case['name']}: estimated tokens {diagnostic['estimated_tokens']} > {case['maxEstimatedTokens']}"
            )

        retrieval_unnecessary = [path for path in ranked if path not in expected]
        retrieval_unnecessary_ratio = (
            len(retrieval_unnecessary) / len(ranked) if ranked else 0.0
        )
        packed_unnecessary = [path for path in packed if path not in expected]
        packed_unnecessary_ratio = len(packed_unnecessary) / len(packed) if packed else 0.0
        max_unnecessary = case.get("maxUnnecessaryFileRatio")
        if max_unnecessary is not None and packed_unnecessary_ratio > max_unnecessary:
            failures.append(
                f"{case['name']}: packed unnecessary file ratio {packed_unnecessary_ratio:.3f} > {max_unnecessary:.3f}; packed={packed}"
            )
        max_latency = case.get("maxLatencyMs")
        if max_latency is not None and elapsed_ms > max_latency:
            failures.append(
                f"{case['name']}: latency {elapsed_ms:.1f}ms > {max_latency:.1f}ms"
            )

        returned_tokens = max(diagnostic["estimated_tokens"], 1)
        canonical_returned_tokens = estimated_tokens(args.binary, context, token_cache)
        if canonical_returned_tokens != diagnostic["estimated_tokens"]:
            failures.append(
                f"{case['name']}: diagnostic token estimate {diagnostic['estimated_tokens']} != canonical estimator {canonical_returned_tokens}"
            )
        savings = 1.0 - returned_tokens / baseline_tokens
        top_k_tokens, top_k_paths = top_k_full_files_baseline(
            args.binary, sources, ranked, token_cache
        )
        grep_tokens, grep_paths = grep_window_baseline(
            args.binary, sources, case["query"], token_cache
        )
        top_k_savings = 1.0 - returned_tokens / top_k_tokens
        grep_savings = 1.0 - returned_tokens / grep_tokens
        relevant_per_1k = len(set(packed_relevant)) * 1000.0 / returned_tokens
        anchors_per_1k = len(matched_anchors) * 1000.0 / returned_tokens
        results.append({
            "name": case["name"],
            "rank": rank,
            "expectedPathsAt5": sorted(set(relevant_at5)),
            "packedExpectedPaths": sorted(set(packed_relevant)),
            "requiredAnchors": required_anchors,
            "matchedAnchors": matched_anchors,
            "retrievalFiles": ranked,
            "packedFiles": packed,
            "retrievalUnnecessaryFileRatio": round(retrieval_unnecessary_ratio, 4),
            "packedUnnecessaryFileRatio": round(packed_unnecessary_ratio, 4),
            "returnedBytes": diagnostic["returned_bytes"],
            "estimatedTokens": diagnostic["estimated_tokens"],
            "tokenSavingsVsFullSource": round(savings, 4),
            "topKFullFilesBaselineTokens": top_k_tokens,
            "topKFullFilesBaselinePaths": top_k_paths,
            "tokenSavingsVsTopKFullFiles": round(top_k_savings, 4),
            "grepWindowBaselineTokens": grep_tokens,
            "grepWindowBaselinePaths": grep_paths,
            "tokenSavingsVsGrepWindows": round(grep_savings, 4),
            "relevantPathsPer1kTokens": round(relevant_per_1k, 4),
            "evidenceAnchorsPer1kTokens": round(anchors_per_1k, 4),
            "elapsedMs": round(elapsed_ms, 2),
        })

    count = len(config["cases"])
    recall = recall_hits / count if count else 0.0
    mrr = sum(reciprocal_ranks) / count if count else 0.0
    expected_path_recall = expected_path_hits / expected_path_total if expected_path_total else 0.0
    packed_expected_path_recall = (
        packed_expected_path_hits / expected_path_total if expected_path_total else 0.0
    )
    required_anchor_recall = (
        required_anchor_hits / required_anchor_total if required_anchor_total else 1.0
    )
    avg_tokens = statistics.fmean(r["estimatedTokens"] for r in results) if results else 0.0
    avg_retrieval_unnecessary = (
        statistics.fmean(r["retrievalUnnecessaryFileRatio"] for r in results) if results else 0.0
    )
    avg_packed_unnecessary = (
        statistics.fmean(r["packedUnnecessaryFileRatio"] for r in results) if results else 0.0
    )
    avg_savings = statistics.fmean(r["tokenSavingsVsFullSource"] for r in results) if results else 0.0
    avg_top_k_savings = (
        statistics.fmean(r["tokenSavingsVsTopKFullFiles"] for r in results) if results else 0.0
    )
    avg_grep_savings = (
        statistics.fmean(r["tokenSavingsVsGrepWindows"] for r in results) if results else 0.0
    )
    avg_relevant_per_1k = (
        statistics.fmean(r["relevantPathsPer1kTokens"] for r in results) if results else 0.0
    )
    avg_anchors_per_1k = (
        statistics.fmean(r["evidenceAnchorsPer1kTokens"] for r in results) if results else 0.0
    )
    latencies = [r["elapsedMs"] for r in results]
    tokens = [r["estimatedTokens"] for r in results]
    p95_latency = percentile(latencies, 0.95)
    p95_tokens = percentile(tokens, 0.95)

    if recall < config["minRecallAt5"]:
        failures.append(f"Recall@5 {recall:.3f} < {config['minRecallAt5']:.3f}")
    if mrr < config["minMrr"]:
        failures.append(f"MRR {mrr:.3f} < {config['minMrr']:.3f}")
    if expected_path_recall < config["minExpectedPathRecallAt5"]:
        failures.append(
            f"expected-path Recall@5 {expected_path_recall:.3f} < {config['minExpectedPathRecallAt5']:.3f}"
        )
    min_packed_recall = config.get("minPackedExpectedPathRecall")
    if min_packed_recall is not None and packed_expected_path_recall < min_packed_recall:
        failures.append(
            f"packed expected-path recall {packed_expected_path_recall:.3f} < {min_packed_recall:.3f}"
        )
    min_anchor_recall = config.get("minRequiredAnchorRecall")
    if min_anchor_recall is not None and required_anchor_recall < min_anchor_recall:
        failures.append(
            f"required evidence anchor recall {required_anchor_recall:.3f} < {min_anchor_recall:.3f}"
        )
    if avg_tokens > config["maxAverageEstimatedTokens"]:
        failures.append(
            f"average estimated tokens {avg_tokens:.1f} > {config['maxAverageEstimatedTokens']}"
        )
    max_p95_tokens = config.get("maxP95EstimatedTokens")
    if max_p95_tokens is not None and p95_tokens > max_p95_tokens:
        failures.append(f"p95 estimated tokens {p95_tokens:.1f} > {max_p95_tokens}")
    if avg_packed_unnecessary > config["maxAverageUnnecessaryFileRatio"]:
        failures.append(
            f"average packed unnecessary file ratio {avg_packed_unnecessary:.3f} > {config['maxAverageUnnecessaryFileRatio']:.3f}"
        )
    if p95_latency > config["maxP95LatencyMs"]:
        failures.append(
            f"p95 latency {p95_latency:.1f}ms > {config['maxP95LatencyMs']:.1f}ms"
        )
    min_savings = config.get("minAverageTokenSavings")
    if min_savings is not None and avg_savings < min_savings:
        failures.append(f"average token savings {avg_savings:.3f} < {min_savings:.3f}")
    min_top_k_savings = config.get("minAverageTokenSavingsVsTopKFullFiles")
    if min_top_k_savings is not None and avg_top_k_savings < min_top_k_savings:
        failures.append(
            f"average token savings vs top-K full files {avg_top_k_savings:.3f} < {min_top_k_savings:.3f}"
        )
    min_efficiency = config.get("minRelevantPathsPer1kTokens")
    if min_efficiency is not None and avg_relevant_per_1k < min_efficiency:
        failures.append(
            f"relevant paths / 1k tokens {avg_relevant_per_1k:.3f} < {min_efficiency:.3f}"
        )
    min_anchor_efficiency = config.get("minEvidenceAnchorsPer1kTokens")
    if min_anchor_efficiency is not None and avg_anchors_per_1k < min_anchor_efficiency:
        failures.append(
            f"evidence anchors / 1k tokens {avg_anchors_per_1k:.3f} < {min_anchor_efficiency:.3f}"
        )

    summary = {
        "baselineRoot": str(baseline_root),
        "baselineSourceFiles": baseline_files,
        "baselineEstimatedTokens": baseline_tokens,
        "tokenEstimator": "sippion heuristic-v3 via estimate-tokens CLI",
        "recallAt5": round(recall, 4),
        "mrr": round(mrr, 4),
        "expectedPathRecallAt5": round(expected_path_recall, 4),
        "packedExpectedPathRecall": round(packed_expected_path_recall, 4),
        "requiredEvidenceAnchorRecall": round(required_anchor_recall, 4),
        "averageEstimatedTokens": round(avg_tokens, 2),
        "p95EstimatedTokens": round(p95_tokens, 2),
        "averageTokenSavingsVsFullSource": round(avg_savings, 4),
        "averageTokenSavingsVsTopKFullFiles": round(avg_top_k_savings, 4),
        "averageTokenSavingsVsGrepWindows": round(avg_grep_savings, 4),
        "averageRelevantPathsPer1kTokens": round(avg_relevant_per_1k, 4),
        "averageEvidenceAnchorsPer1kTokens": round(avg_anchors_per_1k, 4),
        "averageRetrievalUnnecessaryFileRatio": round(avg_retrieval_unnecessary, 4),
        "averagePackedUnnecessaryFileRatio": round(avg_packed_unnecessary, 4),
        "averageUnnecessaryFileRatio": round(avg_packed_unnecessary, 4),
        "p50LatencyMs": round(percentile(latencies, 0.50), 2),
        "p95LatencyMs": round(p95_latency, 2),
        "cases": results,
        "failures": failures,
    }
    print(json.dumps(summary, indent=2))
    raise SystemExit(1 if failures else 0)


if __name__ == "__main__":
    main()
