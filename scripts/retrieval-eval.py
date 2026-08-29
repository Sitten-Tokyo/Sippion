#!/usr/bin/env python3
import argparse
import json
import math
import re
import statistics
import subprocess
import time
from collections import Counter
from pathlib import Path

SOURCE_EXTENSIONS = {
    ".rs", ".py", ".pyi", ".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts",
    ".go", ".java", ".cs", ".c", ".cc", ".cpp", ".cxx", ".h", ".hh", ".hpp", ".hxx",
}
PRUNED_DIRS = {
    "node_modules", "target", "dist", "build", "coverage", ".venv", "venv", "__pycache__",
    ".next", "vendor", ".terraform", ".gradle", ".dart_tool", ".pytest_cache", ".ruff_cache",
}
BM25_K1 = 1.2
BM25_B = 0.75
CAMEL_BOUNDARY = re.compile(r"(?<=[a-z0-9])(?=[A-Z])|(?<=[A-Z])(?=[A-Z][a-z])")
ATOM_HEADER = re.compile(r'^(S|E) path=("(?:\\.|[^"\\])*")')


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


def simple_lexical_terms(text):
    text = CAMEL_BOUNDARY.sub(" ", text)
    parts = re.split(r"[^0-9A-Za-z_\-\u0080-\uffff]+", text)
    terms = []
    for part in parts:
        for component in re.split(r"[_\-]+", part):
            folded = component.casefold()
            if sum(ch.isalnum() for ch in folded) >= 2:
                terms.append(folded)
    return terms


def independent_bm25_rank(sources, query):
    query_terms = list(dict.fromkeys(simple_lexical_terms(query)))
    if not query_terms:
        return []
    documents = {}
    dfs = Counter()
    total_len = 0
    for path, text in sources.items():
        counts = Counter(simple_lexical_terms(text))
        document_len = max(sum(counts.values()), 1)
        documents[path] = (counts, document_len)
        total_len += document_len
        for term in query_terms:
            if counts[term] > 0:
                dfs[term] += 1
    document_count = len(documents)
    if document_count == 0:
        return []
    average_len = total_len / document_count
    ranked = []
    for path, (counts, document_len) in documents.items():
        score = 0.0
        for term in query_terms:
            tf = counts[term]
            if tf <= 0:
                continue
            df = dfs[term]
            idf = math.log(1.0 + (document_count - df + 0.5) / (df + 0.5))
            norm = tf + BM25_K1 * (
                1.0 - BM25_B + BM25_B * document_len / max(average_len, 1.0)
            )
            score += idf * (tf * (BM25_K1 + 1.0) / norm)
        if score > 0.0:
            ranked.append((score, path))
    ranked.sort(key=lambda item: (-item[0], item[1]))
    return [path for _, path in ranked]


def grep_window_baseline(binary, sources, query, cache, max_files=5, radius=4):
    terms = list(dict.fromkeys(simple_lexical_terms(query)))
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


def parse_context_atoms(context):
    atoms = []
    current = None
    for line in context.splitlines(keepends=True):
        match = ATOM_HEADER.match(line)
        if match:
            if current is not None:
                atoms.append(current)
            try:
                path = json.loads(match.group(2))
            except json.JSONDecodeError:
                path = "<invalid>"
            current = {
                "kind": "structure" if match.group(1) == "S" else "evidence",
                "path": path,
                "text": line,
            }
        elif current is not None:
            current["text"] += line
    if current is not None:
        atoms.append(current)
    return atoms


def normalized_requirements(case):
    explicit = case.get("requiredEvidence")
    if explicit is not None:
        requirements = []
        for item in explicit:
            paths = item.get("paths")
            if paths is None:
                path = item.get("path")
                paths = [path] if path else list(case["expectedPaths"])
            requirements.append(
                {
                    "paths": list(paths),
                    "kind": item.get("kind", "any"),
                    "anchors": list(item.get("anchors", [])),
                }
            )
        return requirements

    # Backward-compatible migration path for fixture/train suites: legacy anchors are no longer
    # global substrings. They must appear in an atom owned by one of the expected paths.
    anchors = case.get("requiredAnchors", [])
    if not anchors:
        return []
    return [{"paths": list(case["expectedPaths"]), "kind": "any", "anchors": list(anchors)}]


def match_required_evidence(atoms, requirements):
    matched = []
    missing = []
    for requirement in requirements:
        eligible = [
            atom
            for atom in atoms
            if atom["path"] in requirement["paths"]
            and (requirement["kind"] == "any" or atom["kind"] == requirement["kind"])
        ]
        combined = "\n".join(atom["text"] for atom in eligible)
        for anchor in requirement["anchors"]:
            descriptor = {
                "paths": requirement["paths"],
                "kind": requirement["kind"],
                "anchor": anchor,
            }
            if anchor in combined:
                matched.append(descriptor)
            else:
                missing.append(descriptor)
    return matched, missing


def query_once(binary, fixture, query):
    started = time.perf_counter()
    proc = subprocess.run(
        [binary, "query", "--root", fixture, "--json", "--", query],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    elapsed_ms = (time.perf_counter() - started) * 1000.0
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.strip() or f"query exited {proc.returncode}")
    return json.loads(proc.stdout), elapsed_ms


def stable_projection(payload):
    diagnostic = payload["diagnostics"]
    return {
        "context": payload["context"],
        "ranked": [(entry["path"], entry["rank"]) for entry in diagnostic["ranked_files"]],
        "packed": diagnostic.get("packed_paths", []),
        "atoms": diagnostic.get("packed_atoms", []),
    }


def measured_query(binary, fixture, query, warmup_runs, measurement_runs):
    for _ in range(warmup_runs):
        query_once(binary, fixture, query)
    payloads = []
    latencies = []
    for _ in range(measurement_runs):
        payload, elapsed_ms = query_once(binary, fixture, query)
        payloads.append(payload)
        latencies.append(elapsed_ms)
    reference = stable_projection(payloads[0])
    deterministic = all(stable_projection(payload) == reference for payload in payloads[1:])
    return payloads[0], statistics.median(latencies), latencies, deterministic


def first_expected_rank(paths, expected):
    return next((index + 1 for index, path in enumerate(paths) if path in expected), None)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/release/sippion")
    parser.add_argument("--cases", default="eval/cases.json")
    parser.add_argument("--fixture", default="eval/fixture")
    parser.add_argument("--baseline-root", default=None)
    parser.add_argument("--warmup-runs", type=int, default=1)
    parser.add_argument("--measurement-runs", type=int, default=3)
    args = parser.parse_args()
    if args.warmup_runs < 0 or args.measurement_runs < 1:
        raise SystemExit("warmup-runs must be >= 0 and measurement-runs must be >= 1")

    config = json.loads(Path(args.cases).read_text(encoding="utf-8"))
    baseline_root = args.baseline_root or args.fixture
    sources = source_corpus(baseline_root)
    token_cache = {}
    baseline_tokens, baseline_files = full_source_baseline(args.binary, sources, token_cache)

    results = []
    failures = []
    reciprocal_ranks = []
    bm25_reciprocal_ranks = []
    recall_hits = 0
    bm25_recall_hits = 0
    expected_path_hits = 0
    expected_path_total = 0
    packed_expected_path_hits = 0
    required_anchor_hits = 0
    required_anchor_total = 0

    for case in config["cases"]:
        expected = set(case["expectedPaths"])
        requirements = normalized_requirements(case)
        required_anchor_total += sum(len(item["anchors"]) for item in requirements)
        expected_path_total += len(expected)
        try:
            payload, elapsed_ms, raw_latencies, deterministic = measured_query(
                args.binary,
                args.fixture,
                case["query"],
                args.warmup_runs,
                args.measurement_runs,
            )
        except RuntimeError as error:
            failures.append(f"{case['name']}: query failed: {error}")
            reciprocal_ranks.append(0.0)
            bm25_reciprocal_ranks.append(0.0)
            continue

        context = payload["context"]
        diagnostic = payload["diagnostics"]
        ranked = [entry["path"] for entry in diagnostic["ranked_files"]]
        packed = diagnostic.get("packed_paths", ranked)
        atoms = parse_context_atoms(context)
        top5 = ranked[:5]
        relevant_at5 = [path for path in top5 if path in expected]
        packed_relevant = [path for path in packed if path in expected]
        matched_evidence, missing_evidence = match_required_evidence(atoms, requirements)
        required_anchor_hits += len(matched_evidence)
        expected_path_hits += len(set(relevant_at5))
        packed_expected_path_hits += len(set(packed_relevant))

        rank = first_expected_rank(ranked, expected)
        if rank is not None and rank <= 5:
            recall_hits += 1
        reciprocal_ranks.append(0.0 if rank is None else 1.0 / rank)

        bm25_ranked = independent_bm25_rank(sources, case["query"])
        bm25_rank = first_expected_rank(bm25_ranked, expected)
        if bm25_rank is not None and bm25_rank <= 5:
            bm25_recall_hits += 1
        bm25_reciprocal_ranks.append(0.0 if bm25_rank is None else 1.0 / bm25_rank)

        if not deterministic:
            failures.append(
                f"{case['name']}: repeated measured queries changed context/ranking/packing output"
            )
        if rank is None:
            failures.append(f"{case['name']}: expected path absent from retrieval ranking: {ranked}")
        if case.get("requireAllExpectedAt5", False) and not expected.issubset(set(top5)):
            missing = sorted(expected.difference(top5))
            failures.append(
                f"{case['name']}: expected paths missing from top 5: {missing}; ranked={ranked}"
            )
        if missing_evidence:
            failures.append(
                f"{case['name']}: scoped required evidence missing from model-visible atoms: {missing_evidence}"
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
                f"{case['name']}: packed unnecessary file ratio {packed_unnecessary_ratio:.3f} > "
                f"{max_unnecessary:.3f}; packed={packed}"
            )
        max_latency = case.get("maxLatencyMs")
        if max_latency is not None and elapsed_ms > max_latency:
            failures.append(
                f"{case['name']}: median latency {elapsed_ms:.1f}ms > {max_latency:.1f}ms"
            )

        returned_tokens = max(diagnostic["estimated_tokens"], 1)
        canonical_returned_tokens = estimated_tokens(args.binary, context, token_cache)
        if canonical_returned_tokens != diagnostic["estimated_tokens"]:
            failures.append(
                f"{case['name']}: diagnostic token estimate {diagnostic['estimated_tokens']} "
                f"!= canonical estimator {canonical_returned_tokens}"
            )
        savings = 1.0 - returned_tokens / baseline_tokens
        top_k_tokens, top_k_paths = top_k_full_files_baseline(
            args.binary, sources, ranked, token_cache
        )
        grep_tokens, grep_paths = grep_window_baseline(
            args.binary, sources, case["query"], token_cache
        )
        bm25_tokens, bm25_paths = top_k_full_files_baseline(
            args.binary, sources, bm25_ranked, token_cache
        )
        top_k_savings = 1.0 - returned_tokens / top_k_tokens
        grep_savings = 1.0 - returned_tokens / grep_tokens
        bm25_savings = 1.0 - returned_tokens / bm25_tokens
        relevant_per_1k = len(set(packed_relevant)) * 1000.0 / returned_tokens
        anchors_per_1k = len(matched_evidence) * 1000.0 / returned_tokens

        results.append(
            {
                "name": case["name"],
                "rank": rank,
                "expectedPathsAt5": sorted(set(relevant_at5)),
                "packedExpectedPaths": sorted(set(packed_relevant)),
                "requiredEvidence": requirements,
                "matchedEvidence": matched_evidence,
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
                "bm25Rank": bm25_rank,
                "bm25Top5Paths": bm25_ranked[:5],
                "bm25Top5FullFilesBaselineTokens": bm25_tokens,
                "bm25Top5FullFilesBaselinePaths": bm25_paths,
                "tokenSavingsVsBm25Top5FullFiles": round(bm25_savings, 4),
                "relevantPathsPer1kTokens": round(relevant_per_1k, 4),
                "evidenceAnchorsPer1kTokens": round(anchors_per_1k, 4),
                "medianElapsedMs": round(elapsed_ms, 2),
                "measurementLatenciesMs": [round(value, 2) for value in raw_latencies],
                "packedAtomDiagnostics": diagnostic.get("packed_atoms", []),
            }
        )

    count = len(config["cases"])
    recall = recall_hits / count if count else 0.0
    bm25_recall = bm25_recall_hits / count if count else 0.0
    mrr = sum(reciprocal_ranks) / count if count else 0.0
    bm25_mrr = sum(bm25_reciprocal_ranks) / count if count else 0.0
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
    avg_bm25_savings = (
        statistics.fmean(r["tokenSavingsVsBm25Top5FullFiles"] for r in results) if results else 0.0
    )
    avg_relevant_per_1k = (
        statistics.fmean(r["relevantPathsPer1kTokens"] for r in results) if results else 0.0
    )
    avg_anchors_per_1k = (
        statistics.fmean(r["evidenceAnchorsPer1kTokens"] for r in results) if results else 0.0
    )
    latencies = [r["medianElapsedMs"] for r in results]
    tokens = [r["estimatedTokens"] for r in results]
    p95_latency = percentile(latencies, 0.95)
    p95_tokens = percentile(tokens, 0.95)

    if recall < config["minRecallAt5"]:
        failures.append(f"Recall@5 {recall:.3f} < {config['minRecallAt5']:.3f}")
    if mrr < config["minMrr"]:
        failures.append(f"MRR {mrr:.3f} < {config['minMrr']:.3f}")
    if expected_path_recall < config["minExpectedPathRecallAt5"]:
        failures.append(
            f"expected-path Recall@5 {expected_path_recall:.3f} "
            f"< {config['minExpectedPathRecallAt5']:.3f}"
        )
    min_packed_recall = config.get("minPackedExpectedPathRecall")
    if min_packed_recall is not None and packed_expected_path_recall < min_packed_recall:
        failures.append(
            f"packed expected-path recall {packed_expected_path_recall:.3f} "
            f"< {min_packed_recall:.3f}"
        )
    min_anchor_recall = config.get("minRequiredAnchorRecall")
    if min_anchor_recall is not None and required_anchor_recall < min_anchor_recall:
        failures.append(
            f"required evidence anchor recall {required_anchor_recall:.3f} "
            f"< {min_anchor_recall:.3f}"
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
            f"average packed unnecessary file ratio {avg_packed_unnecessary:.3f} "
            f"> {config['maxAverageUnnecessaryFileRatio']:.3f}"
        )
    if p95_latency > config["maxP95LatencyMs"]:
        failures.append(
            f"p95 median latency {p95_latency:.1f}ms > {config['maxP95LatencyMs']:.1f}ms"
        )
    min_savings = config.get("minAverageTokenSavings")
    if min_savings is not None and avg_savings < min_savings:
        failures.append(f"average token savings {avg_savings:.3f} < {min_savings:.3f}")
    min_top_k_savings = config.get("minAverageTokenSavingsVsTopKFullFiles")
    if min_top_k_savings is not None and avg_top_k_savings < min_top_k_savings:
        failures.append(
            f"average token savings vs top-K full files {avg_top_k_savings:.3f} "
            f"< {min_top_k_savings:.3f}"
        )
    min_efficiency = config.get("minRelevantPathsPer1kTokens")
    if min_efficiency is not None and avg_relevant_per_1k < min_efficiency:
        failures.append(
            f"relevant paths / 1k tokens {avg_relevant_per_1k:.3f} < {min_efficiency:.3f}"
        )
    min_anchor_efficiency = config.get("minEvidenceAnchorsPer1kTokens")
    if min_anchor_efficiency is not None and avg_anchors_per_1k < min_anchor_efficiency:
        failures.append(
            f"evidence anchors / 1k tokens {avg_anchors_per_1k:.3f} "
            f"< {min_anchor_efficiency:.3f}"
        )
    min_recall_delta = config.get("minRecallDeltaVsBm25")
    recall_delta = recall - bm25_recall
    if min_recall_delta is not None and recall_delta < min_recall_delta:
        failures.append(
            f"Recall@5 delta vs independent BM25 {recall_delta:.3f} < {min_recall_delta:.3f}"
        )

    summary = {
        "baselineRoot": str(baseline_root),
        "baselineSourceFiles": baseline_files,
        "baselineEstimatedTokens": baseline_tokens,
        "tokenEstimator": "sippion heuristic-v3 via estimate-tokens CLI",
        "latencyMethod": {
            "warmupRuns": args.warmup_runs,
            "measurementRuns": args.measurement_runs,
            "perCaseStatistic": "median",
        },
        "recallAt5": round(recall, 4),
        "mrr": round(mrr, 4),
        "expectedPathRecallAt5": round(expected_path_recall, 4),
        "packedExpectedPathRecall": round(packed_expected_path_recall, 4),
        "requiredEvidenceAnchorRecall": round(required_anchor_recall, 4),
        "bm25RecallAt5": round(bm25_recall, 4),
        "bm25Mrr": round(bm25_mrr, 4),
        "recallDeltaVsBm25": round(recall_delta, 4),
        "mrrDeltaVsBm25": round(mrr - bm25_mrr, 4),
        "averageEstimatedTokens": round(avg_tokens, 2),
        "p95EstimatedTokens": round(p95_tokens, 2),
        "averageTokenSavingsVsFullSource": round(avg_savings, 4),
        "averageTokenSavingsVsTopKFullFiles": round(avg_top_k_savings, 4),
        "averageTokenSavingsVsGrepWindows": round(avg_grep_savings, 4),
        "averageTokenSavingsVsBm25Top5FullFiles": round(avg_bm25_savings, 4),
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
