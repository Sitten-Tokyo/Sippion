#!/usr/bin/env python3
import argparse
import json
import statistics
import subprocess
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


def send(proc, payload):
    proc.stdin.write(json.dumps(payload, separators=(",", ":")) + "\n")
    proc.stdin.flush()


def receive(proc):
    line = proc.stdout.readline()
    if not line:
        stderr = proc.stderr.read().strip()
        raise RuntimeError(f"MCP server closed stdout unexpectedly: {stderr}")
    return json.loads(line)


def request(proc, request_id, method, params):
    send(
        proc,
        {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": params,
        },
    )
    response = receive(proc)
    if response.get("id") != request_id:
        raise RuntimeError(f"unexpected MCP response id: {response}")
    if "error" in response:
        raise RuntimeError(f"MCP request failed: {response['error']}")
    return response.get("result")


def peak_rss_kib(pid):
    status = Path(f"/proc/{pid}/status")
    if not status.exists():
        return 0
    for line in status.read_text(encoding="utf-8").splitlines():
        if line.startswith("VmHWM:"):
            return int(line.split()[1])
    return 0


def run_session(binary, root, queries, repetitions):
    proc = subprocess.Popen(
        [binary, "mcp", "--root", root],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1,
    )
    samples = []
    first_pass = []
    steady_state = []
    try:
        request(
            proc,
            1,
            "initialize",
            {
                "protocolVersion": "2026-07-28",
                "capabilities": {},
                "clientInfo": {"name": "sippion-warm-perf", "version": "1.0.0"},
            },
        )
        send(proc, {"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})

        request_id = 2
        for repetition in range(repetitions):
            for query in queries:
                started = time.perf_counter()
                result = request(
                    proc,
                    request_id,
                    "tools/call",
                    {"name": "repo_context", "arguments": {"q": query}},
                )
                elapsed_ms = (time.perf_counter() - started) * 1000.0
                request_id += 1
                content = result.get("content", []) if isinstance(result, dict) else []
                if not any(item.get("type") == "text" for item in content if isinstance(item, dict)):
                    raise RuntimeError("repo_context returned no text content")
                samples.append(elapsed_ms)
                if repetition == 0:
                    first_pass.append(elapsed_ms)
                else:
                    steady_state.append(elapsed_ms)

        rss = peak_rss_kib(proc.pid)
    finally:
        if proc.stdin:
            proc.stdin.close()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)

    if not steady_state:
        steady_state = list(first_pass)
    return {
        "samples": len(samples),
        "firstPassMedianLatencyMs": statistics.median(first_pass),
        "steadyStateMedianLatencyMs": statistics.median(steady_state),
        "steadyStateP95LatencyMs": percentile(steady_state, 0.95),
        "peakRssKiB": rss,
    }


def exceeds(candidate, baseline, ratio, allowance):
    return candidate > baseline * ratio + allowance


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--baseline", required=True)
    parser.add_argument("--candidate", required=True)
    parser.add_argument("--fixture", default="eval/fixture")
    parser.add_argument("--cases", default="eval/cases.json")
    parser.add_argument("--repetitions", type=int, default=4)
    parser.add_argument("--output")
    args = parser.parse_args()

    cases = json.loads(Path(args.cases).read_text(encoding="utf-8"))["cases"]
    queries = [case["query"] for case in cases]
    fixture = str(Path(args.fixture).resolve())

    baseline = run_session(
        str(Path(args.baseline).resolve()), fixture, queries, args.repetitions
    )
    candidate = run_session(
        str(Path(args.candidate).resolve()), fixture, queries, args.repetitions
    )
    report = {"mode": "warm-mcp", "baseline": baseline, "candidate": candidate}
    rendered = json.dumps(report, indent=2, sort_keys=True)
    print(rendered)
    if args.output:
        output = Path(args.output)
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(rendered + "\n", encoding="utf-8")

    failures = []
    if exceeds(
        candidate["steadyStateMedianLatencyMs"],
        baseline["steadyStateMedianLatencyMs"],
        1.30,
        10.0,
    ):
        failures.append("warm median latency regressed by more than 30% + 10ms")
    if exceeds(
        candidate["steadyStateP95LatencyMs"],
        baseline["steadyStateP95LatencyMs"],
        1.40,
        20.0,
    ):
        failures.append("warm p95 latency regressed by more than 40% + 20ms")
    if baseline["peakRssKiB"] and exceeds(
        candidate["peakRssKiB"], baseline["peakRssKiB"], 1.25, 16 * 1024
    ):
        failures.append("warm MCP peak RSS regressed by more than 25% + 16MiB")

    if failures:
        raise SystemExit("warm MCP performance regression detected:\n- " + "\n- ".join(failures))


if __name__ == "__main__":
    main()
