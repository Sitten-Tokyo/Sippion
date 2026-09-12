from __future__ import annotations

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

import run_pilot
import verify_task


def command() -> dict[str, object]:
    return {"argv": ["true"], "cwd": "."}


def task(task_id: str = "python-01") -> dict[str, object]:
    return {
        "id": task_id,
        "language": "python",
        "categories": ["feature"],
        "monorepo": False,
        "repository": {
            "url": "https://github.com/example/project.git",
            "commit": "0123456789abcdef0123456789abcdef01234567",
        },
        "prompt": "Implement the deterministic task.",
        "setup": [],
        "verifier": {"commands": [command()]},
        "security_check": [],
        "timeout_seconds": 60,
    }


def pilot_manifest() -> dict[str, object]:
    tasks = []
    for group, language in (("typescript/react", "typescript"), ("python", "python"), ("rust", "rust")):
        for index in range(4):
            language_value = "react" if group == "typescript/react" and index == 3 else language
            current = task(f"{language_value}-{index + 1:02d}")
            current["language"] = language_value
            current["monorepo"] = index == 0
            current["categories"] = ["feature"]
            tasks.append(current)
    categories = [
        "bug-fix", "feature", "refactor", "review", "dependency-temptation",
        "abstraction-temptation", "ambiguity-ask", "ambiguity-default", "security",
    ]
    for index, category in enumerate(categories):
        tasks[index % len(tasks)]["categories"].append(category)
    return {"version": 1, "default_runs": 5, "tasks": tasks}


class RunnerTests(unittest.TestCase):
    def test_manifest_validation_and_matrix(self) -> None:
        tasks = run_pilot.validate_manifest(pilot_manifest())
        self.assertEqual(len(tasks), 12)
        executions = run_pilot.expand_executions(tasks, 5)
        self.assertEqual(len(executions), 240)
        self.assertEqual(
            len({run_pilot.execution_key(item["task"], item["arm"], item["run"]) for item in executions}),
            240,
        )
        with self.assertRaisesRegex(run_pilot.PilotError, "between 1 and 5"):
            run_pilot.expand_executions(tasks, 6)

    def test_duplicate_task_id_rejected(self) -> None:
        document = pilot_manifest()
        document["tasks"][1]["id"] = document["tasks"][0]["id"]
        with self.assertRaisesRegex(run_pilot.PilotError, "duplicate task id"):
            run_pilot.validate_manifest(document)

    def test_missing_verifier_rejected(self) -> None:
        document = pilot_manifest()
        del document["tasks"][0]["verifier"]
        with self.assertRaisesRegex(run_pilot.PilotError, "verifier"):
            run_pilot.validate_manifest(document)

    def test_arm_configurations_are_real_variants(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / "sippion"
            configs = {arm: run_pilot.arm_configuration(arm, binary) for arm in run_pilot.ARMS}
        self.assertEqual(configs["baseline"], ("", ""))
        self.assertIn("repo_context", configs["retrieval"][0])
        self.assertIn("smallest correct", configs["minimal-build"][1])
        self.assertNotIn("Lead with the result", configs["minimal-build"][1])
        self.assertIn("Lead with the result", configs["full"][1])
        self.assertNotEqual(configs["retrieval"], configs["minimal-build"])
        self.assertNotEqual(configs["minimal-build"], configs["full"])

    def test_isolation_and_temporary_codex_configuration(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            first = run_pilot.create_isolated_run(base, "python-01", "baseline", 1)
            second = run_pilot.create_isolated_run(base, "python-01", "full", 1)
            self.assertNotEqual(first.root, second.root)
            no_auth = base / "no-auth"
            run_pilot.prepare_codex_home(
                first, "baseline", Path(directory) / "sippion", source_codex_home=no_auth
            )
            run_pilot.prepare_codex_home(
                second, "full", Path(directory) / "sippion", source_codex_home=no_auth
            )
            self.assertFalse((first.codex_home / "AGENTS.md").exists())
            self.assertTrue((second.codex_home / "AGENTS.md").exists())
            self.assertNotIn("secret", (second.codex_home / "config.toml").read_text())
            self.assertNotIn("secret", (second.codex_home / "AGENTS.md").read_text())
            for current in (first, second):
                os.makedirs(current.root, exist_ok=True)
                import shutil
                shutil.rmtree(current.root)

    def test_codex_jsonl_usage_sums_all_completed_turns(self) -> None:
        lines = [
            json.dumps({"type": "item.completed"}),
            json.dumps({"type": "turn.completed", "usage": {"input_tokens": 10, "cached_input_tokens": 2, "output_tokens": 3}}),
            json.dumps({"type": "turn.completed", "usage": {"input_tokens": 20, "cached_input_tokens": 4, "output_tokens": 5}}),
        ]
        self.assertEqual(run_pilot.parse_codex_usage(lines), run_pilot.Usage(30, 6, 8))

    def test_codex_jsonl_nested_cache_field_is_supported(self) -> None:
        lines = [json.dumps({"type": "turn.completed", "usage": {"input_tokens": 1, "input_tokens_details": {"cached_tokens": 2}, "output_tokens": 3}})]
        self.assertEqual(run_pilot.parse_codex_usage(lines), run_pilot.Usage(1, 2, 3))

    def test_missing_usage_and_malformed_json_rejected(self) -> None:
        with self.assertRaisesRegex(run_pilot.PilotError, "missing usage"):
            run_pilot.parse_codex_usage([json.dumps({"type": "turn.completed"})])
        with self.assertRaisesRegex(run_pilot.PilotError, "invalid JSON"):
            run_pilot.parse_codex_usage(["not-json"])
        with self.assertRaisesRegex(run_pilot.PilotError, "no turn.completed"):
            run_pilot.parse_codex_usage([json.dumps({"type": "item.completed"})])

    def test_verifier_result_and_resume_validation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "results.jsonl"
            row = {
                "task": "python-01",
                "arm": "full",
                "run": 1,
                "input_tokens": 3,
                "output_tokens": 2,
                "cache_read_tokens": 1,
                "correctness": True,
            }
            path.write_text(json.dumps(row) + "\n", encoding="utf-8")
            loaded = run_pilot.load_results(path, {"python-01"})
            self.assertIn(("python-01", "full", 1), loaded)
            with self.assertRaisesRegex(run_pilot.PilotError, "duplicate result"):
                path.write_text(json.dumps(row) + "\n" + json.dumps(row) + "\n", encoding="utf-8")
                run_pilot.load_results(path, {"python-01"})

    def test_result_rejects_credentials_and_unknown_fields(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "results.jsonl"
            row = {
                "task": "python-01", "arm": "full", "run": 1,
                "input_tokens": 1, "output_tokens": 1, "cache_read_tokens": 1,
                "correctness": True, "credential": "sk-never-store-this",
            }
            path.write_text(json.dumps(row) + "\n", encoding="utf-8")
            with self.assertRaisesRegex(run_pilot.PilotError, "schema mismatch"):
                run_pilot.load_results(path, {"python-01"})

    def test_verifier_failure_is_recorded_as_false(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repo = root / "repo"
            repo.mkdir()
            output = root / "last-message.txt"
            output.write_text("done\n", encoding="utf-8")
            run = run_pilot.IsolatedRun(root, repo, root / "home", root / "events", output)
            failing = {"argv": ["false"], "cwd": "."}
            result = run_pilot._run_task_commands(
                [failing], run=run, task_id="python-01", runner_root=root,
                env=os.environ.copy(), timeout_seconds=10,
            )
            self.assertFalse(result)

            passing = run_pilot._run_task_commands(
                [{"argv": ["true"], "cwd": "."}], run=run, task_id="python-01",
                runner_root=root, env=os.environ.copy(), timeout_seconds=10,
            )
            self.assertTrue(passing)

    def test_deterministic_verifier_success_failure_and_secret_gate(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory) / "repo"
            repo.mkdir()
            subprocess.run(["git", "-C", str(repo), "init", "-q"], check=True)
            subprocess.run(["git", "-C", str(repo), "config", "user.email", "test@example.invalid"], check=True)
            subprocess.run(["git", "-C", str(repo), "config", "user.name", "test"], check=True)
            source = repo / "src" / "requests"
            source.mkdir(parents=True)
            target = source / "sessions.py"
            target.write_text("def prepare():\n    return True\n", encoding="utf-8")
            subprocess.run(["git", "-C", str(repo), "add", "."], check=True)
            subprocess.run(["git", "-C", str(repo), "commit", "-qm", "fixture"], check=True)
            target.write_text("def prepare():\n    return False\n", encoding="utf-8")
            output = Path(directory) / "last-message.txt"
            output.write_text("implemented\n", encoding="utf-8")
            self.assertTrue(verify_task.verify_task("py-02", repo, output))
            target.write_text("return 'sk-abcdefghijklmnopqrstuvwxyz'\n", encoding="utf-8")
            self.assertFalse(verify_task.no_secret_material(repo))


if __name__ == "__main__":
    unittest.main()
