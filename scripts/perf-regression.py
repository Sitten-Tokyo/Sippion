#!/usr/bin/env python3
import argparse
import json
import statistics
import subprocess
import tempfile
import time
from pathlib import Path


def percentile(values, p):
    ordered = sorted(values)
    if not ordered:
        return 0.0
    index = (len(ordered) - 1) * p
    low = int(index)
    high = min(low + 1, len(ordered) - 1)
    fraction = index - low
    return ordered[low] * (1 - fraction) + ordered[high] * fraction


def run_query(binary, root, query):
    with tempfile.NamedTemporaryFile(prefix="sippion-rss-", delete=False) as rss_file:
        rss_path = Path(rss_file.name)
    command = [
        "/usr/bin/time",
        "-f",
        "%M",
        "-o",
        str(rss_path),
        binary,
        "query",
        "--root",
        root,
        "--json",
        "--",
        *query.split(),
    ]
    started = time.perf_counter()
    proc = subprocess.run(
        command,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=60,
        check=False,
    )
    elapsed_ms = (time.perf_counter() - started) * 1000.0
    try:
        peak_rss_kib = int(rss_path.read_text(encoding="utf-8").strip())
    finally:
        rss_path.unlink(missing_ok=True)
    if proc.returncode != 0:
        raise RuntimeError(f"query failed: {proc.stderr.strip()}")
    payload = json.loads(proc.stdout)
    diagnostics = payload["diagnostics"]
    return {
        "latencyMs": elapsed_ms,
        "peakRssKiB": peak_rss_kib,
        "scannedBytes": diagnostics["scanned_bytes"],
        "estimatedTokens": diagnostics["estimated_tokens"],
    }


def measure(binary, root, queries, repetitions):
    samples = []
    for _ in range(repetitions):
        for query in queries:
            samples.append(run_query(binary, root, query))
    latencies = [sample["latencyMs"] for sample in samples]
    rss = [sample["peakRssKiB"] for sample in samples]
    scans = [sample["scannedBytes"] for sample in samples]
    tokens = [sample["estimatedTokens"] for sample in samples]
    return {
        "samples": len(samples),
        "medianLatencyMs": statistics.median(latencies),
        "p95LatencyMs": percentile(latencies, 0.95),
        "peakRssKiB": max(rss),
        "medianScannedBytes": statistics.median(scans),
        "medianEstimatedTokens": statistics.median(tokens),
    }


def exceeds(candidate, baseline, ratio, allowance):
    return candidate > baseline * ratio + allowance


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--baseline", required=True)
    parser.add_argument("--candidate", required=True)
    parser.add_argument("--fixture", default="eval/fixture")
    parser.add_argument("--cases", default="eval/cases.json")
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--output")
    args = parser.parse_args()

    cases = json.loads(Path(args.cases).read_text(encoding="utf-8"))["cases"]
    queries = [case["query"] for case in cases]
    fixture = str(Path(args.fixture).resolve())

    # Both binaries run on the same hosted runner and committed fixture. Relative thresholds also
    # include absolute allowances to tolerate normal runner jitter between the two complete suites.
    baseline = measure(str(Path(args.baseline).resolve()), fixture, queries, args.repetitions)
    candidate = measure(str(Path(args.candidate).resolve()), fixture, queries, args.repetitions)

    report = {"baseline": baseline, "candidate": candidate}
    rendered = json.dumps(report, indent=2, sort_keys=True)
    print(rendered)
    if args.output:
        output = Path(args.output)
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(rendered + "\n", encoding="utf-8")

    failures = []
    if exceeds(candidate["medianLatencyMs"], baseline["medianLatencyMs"], 1.30, 20.0):
        failures.append("median latency regressed by more than 30% + 20ms")
    if exceeds(candidate["p95LatencyMs"], baseline["p95LatencyMs"], 1.40, 30.0):
        failures.append("p95 latency regressed by more than 40% + 30ms")
    if exceeds(candidate["peakRssKiB"], baseline["peakRssKiB"], 1.25, 16 * 1024):
        failures.append("peak RSS regressed by more than 25% + 16MiB")
    if exceeds(candidate["medianScannedBytes"], baseline["medianScannedBytes"], 1.10, 1024 * 1024):
        failures.append("median scanned bytes regressed by more than 10% + 1MiB")
    if exceeds(candidate["medianEstimatedTokens"], baseline["medianEstimatedTokens"], 1.10, 100.0):
        failures.append("median model-visible token estimate regressed by more than 10% + 100")

    if failures:
        raise SystemExit("performance regression detected:\n- " + "\n- ".join(failures))


if __name__ == "__main__":
    main()
