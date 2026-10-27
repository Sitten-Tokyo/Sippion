#!/usr/bin/env python3

import unittest

import score


def row(task: str, arm: str, run: int, total: int, correct: bool = True) -> str:
    return (
        '{"task":"%s","arm":"%s","run":%d,'
        '"input_tokens":%d,"output_tokens":0,"cache_read_tokens":0,'
        '"correctness":%s}'
        % (task, arm, run, total, "true" if correct else "false")
    )


class ScoreTests(unittest.TestCase):
    def test_single_run_is_exploratory_and_retains_correctness_gate(self) -> None:
        lines = [
            row("task-a", arm, 1, 100 if arm == "baseline" else 80)
            for arm in score.ARMS
        ]
        result = score.analyze(score.parse_runs(lines))
        self.assertEqual(result["expected_runs"], 1)
        self.assertTrue(result["exploratory"])
        self.assertIn("Provisional leader", score.render_markdown(result))
        self.assertIn("exploratory", score.render_markdown(result))

    def test_replicated_comparison_is_not_labeled_provisional(self) -> None:
        lines = [
            row("task-a", arm, run, 100)
            for arm in score.ARMS for run in range(1, 4)
        ]
        result = score.analyze(score.parse_runs(lines), 3)
        self.assertFalse(result["exploratory"])
        self.assertIn("Winner:", score.render_markdown(result))

    def test_correct_arm_with_fewer_tokens_wins(self) -> None:
        lines = []
        totals = {
            "baseline": 100,
            "retrieval": 90,
            "minimal-build": 80,
            "full": 70,
        }
        for arm, total in totals.items():
            for run in range(1, 6):
                lines.append(row("task-a", arm, run, total + run - 3))
        result = score.analyze(score.parse_runs(lines), 5)
        self.assertEqual(result["winner"], "full")
        self.assertEqual(result["arms"]["full"]["median_task_tokens"], 70)
        self.assertAlmostEqual(result["arms"]["full"]["vs_baseline_percent"], -30.0)

    def test_cached_input_is_not_counted_twice(self) -> None:
        example = score.parse_runs([
            '{"task":"task-a","arm":"baseline","run":1,'
            '"input_tokens":100,"output_tokens":20,"cache_read_tokens":80,'
            '"correctness":true}'
        ])[0]
        self.assertEqual(example.total_tokens, 120)

    def test_cache_read_exceeding_input_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "cannot exceed input_tokens"):
            score.parse_runs([
                '{"task":"task-a","arm":"baseline","run":1,'
                '"input_tokens":10,"output_tokens":2,"cache_read_tokens":11,'
                '"correctness":true}'
            ])

    def test_correctness_failure_disqualifies_lower_token_arm(self) -> None:
        lines = []
        for arm in score.ARMS:
            for run in range(1, 6):
                correct = not (arm == "full" and run == 3)
                lines.append(row("task-a", arm, run, 50 if arm == "full" else 100, correct))
        result = score.analyze(score.parse_runs(lines), 5)
        self.assertFalse(result["arms"]["full"]["eligible"])
        self.assertIsNone(result["arms"]["full"]["vs_baseline_percent"])
        self.assertNotEqual(result["winner"], "full")

    def test_missing_run_fails_closed(self) -> None:
        lines = []
        for arm in score.ARMS:
            limit = 4 if arm == "retrieval" else 5
            for run in range(1, limit + 1):
                lines.append(row("task-a", arm, run, 100))
        with self.assertRaisesRegex(ValueError, "expected 5 runs"):
            score.analyze(score.parse_runs(lines), 5)

    def test_duplicate_run_id_fails_closed(self) -> None:
        lines = []
        for arm in score.ARMS:
            run_ids = [1, 2, 3, 4, 5]
            if arm == "full":
                run_ids[-1] = 4
            for run in run_ids:
                lines.append(row("task-a", arm, run, 100))
        with self.assertRaisesRegex(ValueError, "duplicate run id"):
            score.analyze(score.parse_runs(lines), 5)


if __name__ == "__main__":
    unittest.main()
