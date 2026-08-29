#!/usr/bin/env python3
import argparse
import json
import re
import statistics
import subprocess
import time
from pathlib import Path

SCAN_RE = re.compile(r"\bscan_b=(\d+)\b")
LEGACY_PROTOCOL = "2025-11-25"


def percentile(values, p):
    ordered = sorted(values)
    if not ordered:
        return 0.0
    index = (len(ordered) - 1) * p
    low = int(index)
    high = min(low + 1, len(ordered) - 1)
    fraction = index - low
    return ordered[low] * (1 - fraction) + ordered[high] * fraction


def rss_kib(pid):
    status = Path(f"/proc/{pid}/status")
    try:
        for line in status.read_text(encoding="utf-8").splitlines():
            if line.startswith("VmRSS:"):
                return int(line.split()[1])
    except (FileNotFoundError, ProcessLookupError):
        pass
    return 0


class McpProcess:
    def __init__(self, binary, root):
        self.proc = subprocess.Popen(
            [binary, "mcp", "--root", root],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        self.next_id = 1
        self.peak_rss_kib = rss_kib(self.proc.pid)
        self.request(
            "initialize",
            {
                "protocolVersion": LEGACY_PROTOCOL,
                "capabilities": {},
                "clientInfo": {"name": "sippion-warm-perf", "version": "1.0"},
            },
        )

    def request(self, method, params):
        request_id = self.next_id
        self.next_id += 1
        payload = {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}
        encoded = json.dumps(payload, separators=(",", ":"))
        started = time.perf_counter()
        assert self.proc.stdin is not None
        assert self.proc.stdout is not None
        self.proc.stdin.write(encoded + "\n")
        self.proc.stdin.flush()
        line = self.proc.stdout.readline()
        elapsed_ms = (time.perf_counter() - started) * 1000.0
        self.peak_rss_kib = max(self.peak_rss_kib, rss_kib(self.proc.pid))
        if not line:
            stderr = self.proc.stderr.read() if self.proc.stderr is not None else ""
            raise RuntimeError(f"MCP process ended before response: {stderr.strip()}")
        response = json.loads(line)
        if response.get("id") != request_id:
            raise RuntimeError(f"unexpected MCP response id: {response.get('id')} != {request_id}")
        if "error" in response:
            raise RuntimeError(f"MCP request failed: {response['error']}")
        return elapsed_ms, response.get("result", {})

    def call_context(self, query, session_id):
        elapsed_ms, result = self.request(
            "tools/call",
            {
                "name": "repo_context",
                "arguments": {
                    "q": query,
                    "session_id": session_id,
                    "agent_id": "perf-agent",
                },
            },
        )
        text = "\n".join(
            item.get("text", "")
            for item in result.get("content", [])
            if item.get("type") == "text"
        )
        match = SCAN_RE.search(text)
        return {"latencyMs": elapsed_ms, "scannedBytes": int(match.group(1)) if match else 0}

    def close(self):
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=5)


def summarize(samples):
    latencies = [sample["latencyMs"] for sample in samples]
    scans = [sample["scannedBytes"] for sample in samples]
    return {
        "samples": len(samples),
        "medianLatencyMs": statistics.median(latencies),
        "p95LatencyMs": percentile(latencies, 0.95),
        "medianScannedBytes": statistics.median(scans),
    }


def measure(binary, root, queries):
    client = McpProcess(binary, root)
    try:
        first = []
        warm = []
        for index, query in enumerate(queries):
            first.append(client.call_context(query, f"perf-{index}"))
        for index, query in enumerate(queries):
            warm.append(client.call_context(query, f"perf-{index}"))
        return {
            "firstPass": summarize(first),
            "warmPass": summarize(warm),
            "peakRssKiB": client.peak_rss_kib,
        }
    finally:
        client.close()


def exceeds(candidate, baseline, ratio, allowance):
    return candidate > baseline * ratio + allowance


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--baseline", required=True)
    parser.add_argument("--candidate", required=True)
    parser.add_argument("--fixture", default="eval/fixture")
    parser.add_argument("--cases", default="eval/cases.json")
    parser.add_argument("--max-queries", type=int, default=8)
    parser.add_argument("--output")
    args = parser.parse_args()

    cases = json.loads(Path(args.cases).read_text(encoding="utf-8"))["cases"]
    queries = [case["query"] for case in cases[: args.max_queries]]
    if not queries:
        raise SystemExit("warm MCP benchmark requires at least one query")
    fixture = str(Path(args.fixture).resolve())

    baseline = measure(str(Path(args.baseline).resolve()), fixture, queries)
    candidate = measure(str(Path(args.candidate).resolve()), fixture, queries)
    report = {"baseline": baseline, "candidate": candidate}
    rendered = json.dumps(report, indent=2, sort_keys=True)
    print(rendered)
    if args.output:
        output = Path(args.output)
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(rendered + "\n", encoding="utf-8")

    failures = []
    base_warm = baseline["warmPass"]
    cand_warm = candidate["warmPass"]
    if exceeds(cand_warm["medianLatencyMs"], base_warm["medianLatencyMs"], 1.30, 20.0):
        failures.append("warm median latency regressed by more than 30% + 20ms")
    if exceeds(cand_warm["p95LatencyMs"], base_warm["p95LatencyMs"], 1.40, 30.0):
        failures.append("warm p95 latency regressed by more than 40% + 30ms")
    if exceeds(candidate["peakRssKiB"], baseline["peakRssKiB"], 1.25, 16 * 1024):
        failures.append("long-lived MCP peak RSS regressed by more than 25% + 16MiB")
    if exceeds(cand_warm["medianScannedBytes"], base_warm["medianScannedBytes"], 1.10, 1024 * 1024):
        failures.append("warm median scanned bytes regressed by more than 10% + 1MiB")

    if failures:
        raise SystemExit("warm MCP performance regression detected:\n- " + "\n- ".join(failures))


if __name__ == "__main__":
    main()
