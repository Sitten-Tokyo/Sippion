#!/usr/bin/env python3
from __future__ import annotations

import os
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = os.environ.get("GITHUB_REPOSITORY", "Sitten-Tokyo/Sippion")
PIN = re.compile(
    r"https://raw\.githubusercontent\.com/Sitten-Tokyo/Sippion/([0-9a-f]{40})/scripts/(bootstrap\.(?:sh|ps1))"
)

observed: list[tuple[str, str, str]] = []
for readme in ("README.md", "README.ja.md"):
    text = (ROOT / readme).read_text(encoding="utf-8")
    matches = PIN.findall(text)
    scripts = {script for _, script in matches}
    if scripts != {"bootstrap.sh", "bootstrap.ps1"}:
        raise SystemExit(f"{readme}: expected pinned bootstrap.sh and bootstrap.ps1 URLs")
    for sha, script in matches:
        observed.append((readme, sha, script))

shas = {sha for _, sha, _ in observed}
if len(shas) != 1:
    raise SystemExit(f"README bootstrap pins disagree: {sorted(shas)}")
pin = shas.pop()

subprocess.run(
    ["gh", "api", f"repos/{REPOSITORY}/commits/{pin}", "--jq", ".sha"],
    check=True,
    stdout=subprocess.DEVNULL,
)
for script in ("scripts/bootstrap.sh", "scripts/bootstrap.ps1"):
    current = subprocess.check_output(
        ["git", "hash-object", script], cwd=ROOT, text=True
    ).strip()
    pinned = subprocess.check_output(
        [
            "gh",
            "api",
            f"repos/{REPOSITORY}/contents/{script}?ref={pin}",
            "--jq",
            ".sha",
        ],
        text=True,
    ).strip()
    if current != pinned:
        raise SystemExit(
            f"{script}: README pin {pin} resolves to blob {pinned}, current blob is {current}; update the README pin"
        )

print(f"Bootstrap README pins verified at {pin}")
