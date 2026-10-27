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
    def test_single_run_comparison_is_always_exploratory(self) -> None:
        lines = [
            row("task-a", arm, 1, 100 if arm == "baseline" else 80)
            for arm in score.ARMS
        ]
        result = score.analyze(score.parse_runs(lines))
        self.assertEqual(result["expected_runs"], 1)
        self.assertTrue(result["exploratory"])
        self.assertIn("Provisional leader", score.render_markdown(result))
        self.assertIn("exploratory", score.render_markdown(result))

    def test_correct_arm_with_fewer_tokens_is_provisional_leader(self) -> None:
        totals = {"baseline": 100, "retrieval": 90, "minimal-build": 80, "full": 70}
        lines = [row("task-a", arm, 1, total) for arm, total in totals.items()]
        result = score.analyze(score.parse_runs(lines))
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
        lines = [
            row("task-a", arm, 1, 50 if arm == "full" else 100, arm != "full")
            for arm in score.ARMS
        ]
        result = score.analyze(score.parse_runs(lines))
        self.assertFalse(result["arms"]["full"]["eligible"])
        self.assertIsNone(result["arms"]["full"]["vs_baseline_percent"])
        self.assertNotEqual(result["winner"], "full")

    def test_missing_arm_fails_closed(self) -> None:
        lines = [row("task-a", arm, 1, 100) for arm in score.ARMS if arm != "retrieval"]
        with self.assertRaisesRegex(ValueError, "expected 1 run, found 0"):
            score.analyze(score.parse_runs(lines))

    def test_duplicate_execution_fails_closed(self) -> None:
        lines = [row("task-a", arm, 1, 100) for arm in score.ARMS]
        lines.append(row("task-a", "full", 1, 100))
        with self.assertRaisesRegex(ValueError, "duplicate or extra execution"):
            score.analyze(score.parse_runs(lines))

    def test_repeated_run_number_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "run must be 1"):
            score.parse_runs([row("task-a", "baseline", 2, 100)])


if __name__ == "__main__":
    unittest.main()
