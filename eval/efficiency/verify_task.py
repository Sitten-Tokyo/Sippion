"""Small deterministic post-run checks for the real-OSS efficiency tasks.

This is intentionally a mechanical gate.  It never asks a model to grade a
patch and it never receives or writes credentials.
"""

from __future__ import annotations

import argparse
import ast
import re
import subprocess
from pathlib import Path

EXPECTED_PATHS = {
    "ts-01": ("packages/typescript/src/api/sourceFileCache.ts",),
    "ts-02": ("packages/typescript/src/api/sourceFileCache.ts",),
    "react-01": ("packages/shared/ReactElementType.js",),
    "react-02": ("src/diff/index.js",),
    "py-01": ("src/flask/app.py",),
    "py-02": ("src/requests/sessions.py",),
    "py-03": ("src/click/core.py",),
    "py-04": (),
    "rust-01": ("src/de.rs",),
    "rust-02": ("tokio/src/io/mod.rs",),
    "rust-03": ("clap_builder/src/builder/command.rs",),
    "rust-04": (),
}

DEPENDENCY_FILES = {
    "package.json",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "Cargo.toml",
    "Cargo.lock",
    "pyproject.toml",
    "requirements.txt",
    "requirements-dev.txt",
    "setup.py",
    "setup.cfg",
}
SECRET_PATTERNS = (
    re.compile(r"sk-[A-Za-z0-9_-]{20,}"),
    re.compile(r"gh[pousr]_[A-Za-z0-9_]{20,}"),
    re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----"),
    re.compile(r"OPENAI_API_KEY\s*=\s*['\"][^$\"']+['\"]"),
)


def git(repo: Path, *args: str) -> str:
    completed = subprocess.run(
        ["git", "-C", str(repo), *args],
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError("git check failed")
    return completed.stdout


def changed_paths(repo: Path) -> list[str]:
    tracked = git(repo, "diff", "--name-only", "--diff-filter=ACDMRTUXB").splitlines()
    untracked = git(repo, "ls-files", "--others", "--exclude-standard").splitlines()
    return sorted(set(tracked + untracked))


def changed_text(repo: Path, paths: list[str]) -> str:
    chunks = []
    for relative in paths:
        path = repo / relative
        if not path.is_file() or any(part in {".git", "node_modules", "target"} for part in path.parts):
            continue
        try:
            chunks.append(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError):
            continue
    return "\n".join(chunks)


def no_whitespace_errors(repo: Path) -> bool:
    completed = subprocess.run(
        ["git", "-C", str(repo), "diff", "--check"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    return completed.returncode == 0


def no_secret_material(repo: Path) -> bool:
    paths = changed_paths(repo)
    ignored = git(repo, "ls-files", "--others", "--ignored", "--exclude-standard").splitlines()
    sensitive_ignored = [
        path
        for path in ignored
        if any(
            marker in Path(path).name.casefold()
            for marker in (".env", "secret", "credential", "token", "private")
        )
    ]
    text = changed_text(repo, paths + sensitive_ignored)
    return not any(pattern.search(text) for pattern in SECRET_PATTERNS)


def parse_changed_python(repo: Path, paths: list[str]) -> bool:
    for relative in paths:
        path = repo / relative
        if path.suffix != ".py" or not path.is_file():
            continue
        try:
            ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        except (OSError, UnicodeError, SyntaxError):
            return False
    return True


def verify_task(task_id: str, repo: Path, output: Path) -> bool:
    if task_id not in EXPECTED_PATHS:
        return False
    paths = changed_paths(repo)
    if not no_whitespace_errors(repo) or not parse_changed_python(repo, paths):
        return False
    required = EXPECTED_PATHS[task_id]
    if required:
        if not any(path == expected or path.startswith(expected + "/") for path in paths for expected in required):
            return False
        if any(Path(path).name in DEPENDENCY_FILES for path in paths):
            return False
    elif paths:
        return False

    try:
        response = output.read_text(encoding="utf-8")
    except OSError:
        return False
    if task_id == "py-04":
        questions = [line for line in response.splitlines() if line.strip().endswith("?")]
        return len(questions) == 1
    if task_id == "rust-04":
        return bool(response.strip()) and bool(re.search(r"\b(review|issue|issues|none)\b", response, re.IGNORECASE))
    return bool(response.strip())


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--task")
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--security-only", action="store_true")
    args = parser.parse_args()
    try:
        if not no_secret_material(args.repo):
            return 1
        if args.security_only:
            return 0
        if args.task is None or args.output is None:
            return 2
        return 0 if verify_task(args.task, args.repo, args.output) else 1
    except (RuntimeError, OSError):
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
