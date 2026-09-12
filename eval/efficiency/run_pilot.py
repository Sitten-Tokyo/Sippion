"""Run the real Codex efficiency pilot with isolated, fail-closed executions.

The runner deliberately has no API client dependency.  Codex CLI is the only
model entry point and its JSONL ``turn.completed`` usage is the only source of
token measurements.  A result row is written only after usage has been parsed;
missing or malformed usage never becomes synthetic benchmark data.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

ARMS = ("baseline", "retrieval", "minimal-build", "full")
LANGUAGE_GROUPS = ("typescript/react", "python", "rust")
RESULT_FIELDS = frozenset(
    {
        "task",
        "arm",
        "run",
        "input_tokens",
        "output_tokens",
        "cache_read_tokens",
        "correctness",
    }
)
SHA256_RE = re.compile(r"^[0-9a-f]{40}$")
TASK_ID_RE = re.compile(r"^[a-z0-9]+-[0-9]{2}$")
DEFAULT_RUNS = 5
DEFAULT_TIMEOUT_SECONDS = 1800
DEFAULT_OUTPUT = Path("eval/efficiency/results.jsonl")
DEFAULT_MANIFEST = Path(__file__).with_name("tasks.json")

# Keep these arm inputs explicit.  In particular, an arm name must never be a
# cosmetic label on one shared prompt/configuration.
MINIMAL_BUILD_RULE = (
    "Build the smallest correct solution after understanding the relevant flow. "
    "Reuse existing code first; then prefer the standard library, native platform "
    "features, and installed dependencies. Fix root causes, not symptoms, and "
    "inspect affected callers before changing shared behavior. Avoid unrequested "
    "abstractions, dependencies, boilerplate, and speculative future-proofing. "
    "Preserve required validation, security, data-loss protection, and requested "
    "behavior; leave one runnable check for non-trivial logic."
)
FULL_RULE = (
    MINIMAL_BUILD_RULE
    + " Ask one concise question only when ambiguity materially changes the result "
    "or a requested technology appears unnecessary, unless the user says not to ask. "
    "Otherwise choose the clearly reasonable default and proceed. Lead with the "
    "result or next action; no preamble. Keep explanations and choices minimal. "
    "In reviews, report only concrete correctness, security, performance, or "
    "maintainability issues; omit style-only and speculative concerns. If none exist, "
    "say so briefly. Reply in the user's language."
)


class PilotError(ValueError):
    """A fail-closed benchmark configuration or execution error."""


@dataclass(frozen=True)
class Usage:
    input_tokens: int
    cache_read_tokens: int
    output_tokens: int


@dataclass(frozen=True)
class IsolatedRun:
    root: Path
    repository: Path
    codex_home: Path
    codex_jsonl: Path
    last_message: Path


def _non_negative_int(value: object, field: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise PilotError(f"{field} must be a non-negative integer")
    return value


def _require_string(value: object, field: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise PilotError(f"{field} must be a non-empty string")
    return value


def language_group(language: str) -> str:
    return "typescript/react" if language in {"typescript", "react"} else language


def validate_command_spec(spec: object, field: str) -> None:
    if not isinstance(spec, dict):
        raise PilotError(f"{field} must be an object")
    argv = spec.get("argv")
    if not isinstance(argv, list) or not argv or not all(
        isinstance(item, str) and item for item in argv
    ):
        raise PilotError(f"{field}.argv must be a non-empty string array")
    cwd = spec.get("cwd", ".")
    if (
        not isinstance(cwd, str)
        or not cwd
        or Path(cwd).is_absolute()
        or ".." in Path(cwd).parts
    ):
        raise PilotError(f"{field}.cwd must be a relative path")


def validate_task(task: object, index: int) -> None:
    field = f"tasks[{index}]"
    if not isinstance(task, dict):
        raise PilotError(f"{field} must be an object")
    task_id = _require_string(task.get("id"), f"{field}.id")
    if not TASK_ID_RE.fullmatch(task_id):
        raise PilotError(f"{field}.id has invalid format: {task_id}")
    language = _require_string(task.get("language"), f"{field}.language")
    if language_group(language) not in LANGUAGE_GROUPS:
        raise PilotError(f"{field}.language is unsupported: {language}")

    categories = task.get("categories")
    if not isinstance(categories, list) or not categories or not all(
        isinstance(item, str) and item.strip() for item in categories
    ):
        raise PilotError(f"{field}.categories must be a non-empty string array")

    repository = task.get("repository")
    if not isinstance(repository, dict):
        raise PilotError(f"{field}.repository must be an object")
    url = _require_string(repository.get("url"), f"{field}.repository.url")
    if not url.startswith("https://github.com/"):
        raise PilotError(f"{field}.repository.url must be an HTTPS GitHub URL")
    commit = _require_string(repository.get("commit"), f"{field}.repository.commit")
    if not SHA256_RE.fullmatch(commit):
        raise PilotError(f"{field}.repository.commit must be a 40-character SHA")

    _require_string(task.get("prompt"), f"{field}.prompt")
    setup = task.get("setup")
    if not isinstance(setup, list):
        raise PilotError(f"{field}.setup must be an array")
    for command_index, command in enumerate(setup):
        validate_command_spec(command, f"{field}.setup[{command_index}]")

    verifier = task.get("verifier")
    if not isinstance(verifier, dict):
        raise PilotError(f"{field}.verifier must be an object")
    verifier_commands = verifier.get("commands")
    if not isinstance(verifier_commands, list) or not verifier_commands:
        raise PilotError(f"{field}.verifier.commands must be non-empty")
    for command_index, command in enumerate(verifier_commands):
        validate_command_spec(command, f"{field}.verifier.commands[{command_index}]")

    security = task.get("security_check", [])
    if not isinstance(security, list):
        raise PilotError(f"{field}.security_check must be an array")
    for command_index, command in enumerate(security):
        validate_command_spec(command, f"{field}.security_check[{command_index}]")

    timeout = task.get("timeout_seconds", DEFAULT_TIMEOUT_SECONDS)
    if isinstance(timeout, bool) or not isinstance(timeout, int) or timeout < 1:
        raise PilotError(f"{field}.timeout_seconds must be positive")
    if not isinstance(task.get("monorepo", False), bool):
        raise PilotError(f"{field}.monorepo must be boolean")


def validate_manifest(document: object, *, require_pilot_matrix: bool = True) -> list[dict[str, Any]]:
    if not isinstance(document, dict):
        raise PilotError("manifest must be a JSON object")
    if document.get("version") != 1:
        raise PilotError("manifest version must be 1")
    default_runs = document.get("default_runs")
    if default_runs != DEFAULT_RUNS:
        raise PilotError(f"manifest default_runs must be {DEFAULT_RUNS}")
    tasks = document.get("tasks")
    if not isinstance(tasks, list) or not tasks:
        raise PilotError("manifest tasks must be a non-empty array")

    ids: set[str] = set()
    for index, task in enumerate(tasks):
        validate_task(task, index)
        task_id = task["id"]
        if task_id in ids:
            raise PilotError(f"duplicate task id: {task_id}")
        ids.add(task_id)

    if not require_pilot_matrix:
        return tasks
    if len(tasks) != 12:
        raise PilotError(f"pilot manifest must contain 12 tasks, found {len(tasks)}")
    counts = {group: 0 for group in LANGUAGE_GROUPS}
    for task in tasks:
        counts[language_group(task["language"])] += 1
    expected_counts = {"typescript/react": 4, "python": 4, "rust": 4}
    if counts != expected_counts:
        raise PilotError(f"language counts must be {expected_counts}, found {counts}")
    if not any(task.get("monorepo") for task in tasks):
        raise PilotError("pilot must include at least one monorepo task")
    required_categories = {
        "bug-fix",
        "feature",
        "refactor",
        "review",
        "dependency-temptation",
        "abstraction-temptation",
        "ambiguity-ask",
        "ambiguity-default",
        "security",
    }
    categories = {category for task in tasks for category in task["categories"]}
    missing = sorted(required_categories - categories)
    if missing:
        raise PilotError(f"pilot manifest is missing categories: {', '.join(missing)}")
    return tasks


def load_manifest(path: Path, *, require_pilot_matrix: bool = True) -> list[dict[str, Any]]:
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except OSError as error:
        raise PilotError(f"cannot read manifest: {error}") from error
    except json.JSONDecodeError as error:
        raise PilotError(f"manifest is invalid JSON: {error.msg}") from error
    return validate_manifest(document, require_pilot_matrix=require_pilot_matrix)


def execution_key(task_id: str, arm: str, run: int) -> tuple[str, str, int]:
    return (task_id, arm, run)


def expand_executions(
    tasks: Sequence[Mapping[str, Any]],
    runs: int,
    task_filter: str | None = None,
    arm_filter: str | None = None,
) -> list[dict[str, Any]]:
    if runs < 1 or runs > DEFAULT_RUNS:
        raise PilotError(f"runs must be between 1 and {DEFAULT_RUNS}")
    if arm_filter is not None and arm_filter not in ARMS:
        raise PilotError(f"unknown arm: {arm_filter}")
    selected_tasks = [task for task in tasks if task_filter is None or task["id"] == task_filter]
    if task_filter is not None and not selected_tasks:
        raise PilotError(f"unknown task: {task_filter}")
    selected_arms = [arm_filter] if arm_filter else list(ARMS)
    executions = [
        {"task": task["id"], "arm": arm, "run": run}
        for task in selected_tasks
        for arm in selected_arms
        for run in range(1, runs + 1)
    ]
    keys = [execution_key(item["task"], item["arm"], item["run"]) for item in executions]
    if len(keys) != len(set(keys)):
        raise PilotError("execution expansion contains duplicate keys")
    return executions


def _toml_quote(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


def arm_configuration(arm: str, sippion_binary: Path) -> tuple[str, str]:
    if arm not in ARMS:
        raise PilotError(f"unknown arm: {arm}")
    if arm == "baseline":
        return "", ""
    config = (
        "[mcp_servers.sippion]\n"
        f"command = {_toml_quote(str(sippion_binary.resolve()))}\n"
        'args = ["mcp", "--root-auto"]\n'
        'cwd = "."\n'
        'enabled_tools = ["repo_context"]\n'
    )
    if arm == "retrieval":
        return config, ""
    if arm == "minimal-build":
        return config, f"# Sippion efficiency: minimal-build\n{MINIMAL_BUILD_RULE}\n"
    return config, f"# Sippion efficiency: full\n{FULL_RULE}\n"


def create_isolated_run(base_dir: Path, task_id: str, arm: str, run: int) -> IsolatedRun:
    if run < 1:
        raise PilotError("run must be positive")
    root = Path(tempfile.mkdtemp(prefix=f"sippion-pilot-{task_id}-{arm}-{run}-", dir=base_dir))
    repository = root / "repository"
    codex_home = root / "codex-home"
    codex_jsonl = root / "codex.jsonl"
    last_message = root / "last-message.txt"
    return IsolatedRun(root, repository, codex_home, codex_jsonl, last_message)


def prepare_codex_home(
    run: IsolatedRun,
    arm: str,
    sippion_binary: Path,
    *,
    source_codex_home: Path | None = None,
) -> None:
    config, rules = arm_configuration(arm, sippion_binary)
    run.codex_home.mkdir(parents=True, exist_ok=False)
    (run.codex_home / "config.toml").write_text(config, encoding="utf-8")
    if rules:
        (run.codex_home / "AGENTS.md").write_text(rules, encoding="utf-8")

    source = source_codex_home
    if source is None:
        configured = os.environ.get("CODEX_HOME")
        source = Path(configured) if configured else Path.home() / ".codex"
    auth = source / "auth.json"
    if auth.is_file():
        try:
            destination = run.codex_home / "auth.json"
            shutil.copy2(auth, destination)
            destination.chmod(0o600)
        except OSError as error:
            raise PilotError("cannot prepare temporary Codex authentication") from error


def codex_command(
    codex_binary: str,
    run: IsolatedRun,
    prompt: str,
    *,
    model: str | None = None,
) -> list[str]:
    command = [
        codex_binary,
        "exec",
        "--json",
        "--ephemeral",
        "--dangerously-bypass-approvals-and-sandbox",
        "--color",
        "never",
        "--cd",
        str(run.repository),
        "--output-last-message",
        str(run.last_message),
    ]
    if model:
        command.extend(["--model", model])
    command.append(prompt)
    return command


def _usage_field(usage: Mapping[str, Any], name: str) -> int:
    value = usage.get(name)
    return _non_negative_int(value, f"turn.completed usage.{name}")


def parse_codex_usage(lines: Iterable[str]) -> Usage:
    """Sum every turn.completed usage event for one Codex session.

    ``codex exec --json`` can emit more than one completed turn when the agent
    resumes after tool work.  The session total is therefore the sum of all
    completed-turn usage objects, not the first event or the final event alone.
    The cache field accepts the current flat name and the API-compatible nested
    spelling, but never assumes zero when it is absent.
    """

    total_input = 0
    total_cache = 0
    total_output = 0
    completed = 0
    for line_number, raw in enumerate(lines, 1):
        raw = raw.strip()
        if not raw:
            continue
        try:
            event = json.loads(raw)
        except json.JSONDecodeError as error:
            raise PilotError(f"Codex JSONL line {line_number} is invalid JSON") from error
        if not isinstance(event, dict):
            raise PilotError(f"Codex JSONL line {line_number} must be an object")
        if event.get("type") != "turn.completed":
            continue
        usage = event.get("usage")
        if not isinstance(usage, dict):
            raise PilotError(f"turn.completed line {line_number} is missing usage")
        input_tokens = _usage_field(usage, "input_tokens")
        output_tokens = _usage_field(usage, "output_tokens")
        if "cached_input_tokens" in usage:
            cache_tokens = _usage_field(usage, "cached_input_tokens")
        else:
            details = usage.get("input_tokens_details")
            if not isinstance(details, dict) or "cached_tokens" not in details:
                raise PilotError(
                    f"turn.completed line {line_number} is missing cached input usage"
                )
            cache_tokens = _non_negative_int(
                details["cached_tokens"],
                "turn.completed usage.input_tokens_details.cached_tokens",
            )
        total_input += input_tokens
        total_cache += cache_tokens
        total_output += output_tokens
        completed += 1
    if completed == 0:
        raise PilotError("Codex JSONL contains no turn.completed usage")
    return Usage(total_input, total_cache, total_output)


def _format_command(argv: Sequence[str], run: IsolatedRun, runner_root: Path, task_id: str) -> list[str]:
    values = {
        "repo": str(run.repository),
        "codex_home": str(run.codex_home),
        "codex_output": str(run.last_message),
        "codex_jsonl": str(run.codex_jsonl),
        "runner_root": str(runner_root),
        "task_id": task_id,
    }
    try:
        return [item.format(**values) for item in argv]
    except KeyError as error:
        raise PilotError(f"unknown command placeholder: {error.args[0]}") from error


def run_external(
    argv: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
    timeout_seconds: int,
    stdout: int = subprocess.PIPE,
    stderr: int = subprocess.PIPE,
) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            list(argv),
            cwd=cwd,
            env=dict(env),
            text=True,
            stdout=stdout,
            stderr=stderr,
            timeout=timeout_seconds,
            check=False,
        )
    except FileNotFoundError as error:
        raise PilotError(f"required executable is unavailable: {argv[0]}") from error
    except subprocess.TimeoutExpired as error:
        raise PilotError(f"command timed out after {timeout_seconds}s: {argv[0]}") from error
    except OSError as error:
        raise PilotError(f"could not run command {argv[0]}: {error}") from error


def isolated_environment(run: IsolatedRun) -> dict[str, str]:
    """Give each setup, model, and verifier process private mutable state."""

    environment = os.environ.copy()
    private_home = run.root / "home"
    private_cache = run.root / "cache"
    private_tmp = run.root / "tmp"
    for path in (private_home, private_cache, private_tmp):
        path.mkdir(parents=True, exist_ok=True)
    environment.update(
        {
            "HOME": str(private_home),
            "XDG_CONFIG_HOME": str(run.root / "config"),
            "XDG_CACHE_HOME": str(private_cache),
            "TMPDIR": str(private_tmp),
            "PIP_CACHE_DIR": str(private_cache / "pip"),
            "npm_config_cache": str(private_cache / "npm"),
            "CARGO_HOME": str(run.root / "cargo-home"),
            # Keep the installed compiler toolchain visible while isolating
            # Cargo's mutable registry/git/target caches above.
            "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup")),
        }
    )
    return environment


def clone_at_revision(task: Mapping[str, Any], repository: Path, timeout_seconds: int) -> None:
    source = task["repository"]
    url = source["url"]
    commit = source["commit"]
    repository.parent.mkdir(parents=True, exist_ok=True)
    parent_env = os.environ.copy()
    clone = run_external(
        [
            "git",
            "clone",
            "--filter=blob:none",
            "--no-tags",
            "--no-checkout",
            url,
            str(repository),
        ],
        cwd=repository.parent,
        env=parent_env,
        timeout_seconds=timeout_seconds,
    )
    if clone.returncode != 0:
        raise PilotError(f"could not clone task repository {task['id']}")
    fetched = run_external(
        ["git", "-C", str(repository), "fetch", "--depth=1", "origin", commit],
        cwd=repository.parent,
        env=parent_env,
        timeout_seconds=timeout_seconds,
    )
    if fetched.returncode != 0:
        raise PilotError(f"could not fetch pinned revision for {task['id']}")
    checked_out = run_external(
        ["git", "-C", str(repository), "checkout", "--detach", commit],
        cwd=repository.parent,
        env=parent_env,
        timeout_seconds=timeout_seconds,
    )
    if checked_out.returncode != 0:
        raise PilotError(f"could not check out pinned revision for {task['id']}")
    revision = run_external(
        ["git", "-C", str(repository), "rev-parse", "HEAD"],
        cwd=repository.parent,
        env=parent_env,
        timeout_seconds=timeout_seconds,
    )
    if revision.returncode != 0 or revision.stdout.strip() != commit:
        raise PilotError(f"task {task['id']} is not at its pinned revision")


def _run_task_commands(
    commands: Sequence[Mapping[str, Any]],
    *,
    run: IsolatedRun,
    task_id: str,
    runner_root: Path,
    env: Mapping[str, str],
    timeout_seconds: int,
) -> bool:
    for command in commands:
        argv = _format_command(command["argv"], run, runner_root, task_id)
        cwd = run.repository / command.get("cwd", ".")
        completed = run_external(
            argv,
            cwd=cwd,
            env=env,
            timeout_seconds=timeout_seconds,
        )
        if completed.returncode != 0:
            return False
    return True


def credential_available(codex_binary: str) -> bool:
    if os.environ.get("OPENAI_API_KEY"):
        return True
    try:
        status = subprocess.run(
            [codex_binary, "login", "status"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            text=True,
            timeout=30,
            check=False,
        )
    except (FileNotFoundError, OSError, subprocess.TimeoutExpired):
        return False
    return status.returncode == 0


def run_one(
    task: Mapping[str, Any],
    arm: str,
    run_number: int,
    *,
    temp_parent: Path,
    runner_root: Path,
    codex_binary: str,
    sippion_binary: Path,
    model: str | None,
) -> dict[str, Any]:
    timeout_seconds = task.get("timeout_seconds", DEFAULT_TIMEOUT_SECONDS)
    isolated = create_isolated_run(temp_parent, task["id"], arm, run_number)
    try:
        clone_at_revision(task, isolated.repository, timeout_seconds)
        setup_env = isolated_environment(isolated)
        setup_env["SIPPION_TASK_ROOT"] = str(isolated.repository)
        setup_ok = _run_task_commands(
            task["setup"],
            run=isolated,
            task_id=task["id"],
            runner_root=runner_root,
            env=setup_env,
            timeout_seconds=timeout_seconds,
        )
        if not setup_ok:
            raise PilotError(f"setup failed for {task['id']}")

        prepare_codex_home(isolated, arm, sippion_binary)
        codex_env = isolated_environment(isolated)
        codex_env["CODEX_HOME"] = str(isolated.codex_home)
        codex_env["SIPPION_TASK_ROOT"] = str(isolated.repository)
        command = codex_command(codex_binary, isolated, task["prompt"], model=model)
        try:
            with isolated.codex_jsonl.open("w", encoding="utf-8") as jsonl_handle:
                completed = subprocess.run(
                    command,
                    cwd=isolated.repository,
                    env=codex_env,
                    text=True,
                    stdout=jsonl_handle,
                    stderr=subprocess.PIPE,
                    timeout=timeout_seconds,
                    check=False,
                )
        except FileNotFoundError as error:
            raise PilotError(f"Codex CLI is unavailable: {codex_binary}") from error
        except subprocess.TimeoutExpired as error:
            raise PilotError(f"Codex run timed out after {timeout_seconds}s") from error

        try:
            usage = parse_codex_usage(isolated.codex_jsonl.read_text(encoding="utf-8").splitlines())
        except OSError as error:
            raise PilotError("cannot read Codex JSONL output") from error
        if completed.returncode != 0:
            raise PilotError(f"Codex exited with status {completed.returncode}")

        verify_env = os.environ.copy()
        verify_env["SIPPION_TASK_ROOT"] = str(isolated.repository)
        verify_env["CODEX_OUTPUT_PATH"] = str(isolated.last_message)
        verifier_ok = _run_task_commands(
            task["verifier"]["commands"],
            run=isolated,
            task_id=task["id"],
            runner_root=runner_root,
            env=verify_env,
            timeout_seconds=timeout_seconds,
        )
        security_ok = _run_task_commands(
            task.get("security_check", []),
            run=isolated,
            task_id=task["id"],
            runner_root=runner_root,
            env=verify_env,
            timeout_seconds=timeout_seconds,
        )
        return {
            "task": task["id"],
            "arm": arm,
            "run": run_number,
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
            "cache_read_tokens": usage.cache_read_tokens,
            "correctness": bool(verifier_ok and security_ok),
        }
    finally:
        shutil.rmtree(isolated.root, ignore_errors=True)


def _validate_result_row(row: object, line_number: int, task_ids: set[str]) -> dict[str, Any]:
    if not isinstance(row, dict):
        raise PilotError(f"result line {line_number} must be an object")
    if set(row) != RESULT_FIELDS:
        missing = sorted(RESULT_FIELDS - set(row))
        extra = sorted(set(row) - RESULT_FIELDS)
        detail = []
        if missing:
            detail.append(f"missing {','.join(missing)}")
        if extra:
            detail.append(f"unexpected {','.join(extra)}")
        raise PilotError(f"result line {line_number} schema mismatch ({'; '.join(detail)})")
    task = _require_string(row["task"], f"result line {line_number}.task")
    if task not in task_ids:
        raise PilotError(f"result line {line_number} references unknown task {task}")
    arm = row["arm"]
    if arm not in ARMS:
        raise PilotError(f"result line {line_number}.arm is invalid")
    run = _non_negative_int(row["run"], f"result line {line_number}.run")
    if run < 1:
        raise PilotError(f"result line {line_number}.run must be positive")
    for field in ("input_tokens", "output_tokens", "cache_read_tokens"):
        _non_negative_int(row[field], f"result line {line_number}.{field}")
    if not isinstance(row["correctness"], bool):
        raise PilotError(f"result line {line_number}.correctness must be boolean")
    return dict(row)


def load_results(
    path: Path,
    task_ids: set[str],
    *,
    max_run: int = DEFAULT_RUNS,
) -> dict[tuple[str, str, int], dict[str, Any]]:
    if not path.exists():
        return {}
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise PilotError(f"cannot read results: {error}") from error
    results: dict[tuple[str, str, int], dict[str, Any]] = {}
    for line_number, raw in enumerate(lines, 1):
        if not raw.strip():
            raise PilotError(f"result line {line_number} is blank")
        try:
            row = json.loads(raw)
        except json.JSONDecodeError as error:
            raise PilotError(f"result line {line_number} is invalid JSON") from error
        normalized = _validate_result_row(row, line_number, task_ids)
        if normalized["run"] > max_run:
            raise PilotError(f"result line {line_number}.run exceeds configured run count")
        key = execution_key(normalized["task"], normalized["arm"], normalized["run"])
        if key in results:
            raise PilotError(f"duplicate result execution: {key[0]}/{key[1]}/{key[2]}")
        results[key] = normalized
    return results


def append_result(path: Path, row: Mapping[str, Any], task_ids: set[str]) -> None:
    normalized = _validate_result_row(dict(row), 1, task_ids)
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        with path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(normalized, sort_keys=True, separators=(",", ":")))
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
    except OSError as error:
        raise PilotError(f"cannot append result: {error}") from error


def dry_run_report(
    tasks: Sequence[Mapping[str, Any]],
    executions: Sequence[Mapping[str, Any]],
    *,
    output: Path,
    sippion_binary: Path,
) -> dict[str, Any]:
    configs = {}
    for arm in ARMS:
        config, rules = arm_configuration(arm, sippion_binary)
        configs[arm] = {
            "config_sha256": hashlib.sha256(config.encode()).hexdigest(),
            "rules_sha256": hashlib.sha256(rules.encode()).hexdigest(),
            "has_repo_context": "enabled_tools = [\"repo_context\"]" in config,
            "has_minimal_build_rule": MINIMAL_BUILD_RULE in rules,
            "has_full_concise_rule": "Lead with the result or next action" in rules,
        }
    keys = [execution_key(item["task"], item["arm"], item["run"]) for item in executions]
    return {
        "manifest_tasks": len(tasks),
        "arms": len(ARMS),
        "runs_per_task_arm": max((item["run"] for item in executions), default=0),
        "executions": len(executions),
        "task_ids_unique": len({task["id"] for task in tasks}) == len(tasks),
        "execution_keys_unique": len(keys) == len(set(keys)),
        "verifier_present_for_all_tasks": all(task.get("verifier") for task in tasks),
        "pinned_revision_for_all_tasks": all(
            SHA256_RE.fullmatch(task["repository"]["commit"]) for task in tasks
        ),
        "arm_configurations": configs,
        "output": str(output),
        "model_calls": 0,
    }


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--task")
    parser.add_argument("--arm", choices=ARMS)
    parser.add_argument("--runs", type=int, default=DEFAULT_RUNS)
    parser.add_argument("--codex", default="codex")
    parser.add_argument("--model", default=None)
    parser.add_argument("--sippion-binary", type=Path, default=None)
    parser.add_argument("--work-dir", type=Path, default=None)
    parser.add_argument("--dry-run", action="store_true")
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    runner_root = Path(__file__).resolve().parents[2]
    sippion_binary = args.sippion_binary or Path(
        os.environ.get("SIPPION_BINARY", runner_root / "target" / "release" / "sippion")
    )
    try:
        tasks = load_manifest(args.manifest)
        executions = expand_executions(tasks, args.runs, args.task, args.arm)
        task_ids = {task["id"] for task in tasks}
        existing = load_results(args.output, task_ids, max_run=DEFAULT_RUNS)
        if args.dry_run:
            print(
                json.dumps(
                    dry_run_report(
                        tasks,
                        executions,
                        output=args.output,
                        sippion_binary=sippion_binary,
                    ),
                    indent=2,
                    sort_keys=True,
                )
            )
            return 0

        if not credential_available(args.codex):
            raise PilotError(
                "Codex authentication unavailable; set OPENAI_API_KEY or run 'codex login'"
            )
        task_by_id = {task["id"]: task for task in tasks}
        pending = [
            item
            for item in executions
            if execution_key(item["task"], item["arm"], item["run"]) not in existing
        ]
        if not pending:
            print(f"No pending executions; validated {len(existing)} existing result rows.")
            return 0
        if not sippion_binary.is_file() and any(item["arm"] != "baseline" for item in pending):
            raise PilotError(
                f"Sippion binary is required for retrieval arms but was not found: {sippion_binary}"
            )
        work_parent = args.work_dir or Path(tempfile.gettempdir())
        work_parent.mkdir(parents=True, exist_ok=True)
        for item in pending:
            row = run_one(
                task_by_id[item["task"]],
                item["arm"],
                item["run"],
                temp_parent=work_parent,
                runner_root=runner_root,
                codex_binary=args.codex,
                sippion_binary=sippion_binary,
                model=args.model,
            )
            append_result(args.output, row, task_ids)
            existing[execution_key(item["task"], item["arm"], item["run"])] = row
            print(
                f"completed {item['task']}/{item['arm']}/{item['run']} "
                f"correctness={row['correctness']}"
            )
        return 0
    except PilotError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
