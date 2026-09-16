#!/usr/bin/env python3
from __future__ import annotations

import unittest

import run_quick
import score


def _run(task: str, arm: str, total: int, correctness: bool = True) -> score.Run:
    return score.Run(
        task=task,
        arm=arm,
        run=1,
        input_tokens=total,
        output_tokens=0,
        cache_read_tokens=0,
        correctness=correctness,
    )


class QuickBenchmarkTest(unittest.TestCase):
    def test_reports_paired_savings(self) -> None:
        runs = []
        for task, baseline, retrieval in (
            ("react-02", 1000, 600),
            ("py-02", 800, 600),
            ("rust-01", 1200, 600),
        ):
            runs.extend(
                [
                    _run(task, "baseline", baseline),
                    _run(task, "retrieval", retrieval),
                ]
            )

        result = run_quick.analyze_quick(runs)

        self.assertTrue(result["overall"]["correctness_gate"])
        self.assertEqual(result["overall"]["baseline_total_tokens"], 3000)
        self.assertEqual(result["overall"]["retrieval_total_tokens"], 1800)
        self.assertEqual(result["overall"]["saved_tokens"], 1200)
        self.assertAlmostEqual(result["overall"]["saved_percent"], 40.0)
        self.assertEqual(result["overall"]["baseline_median_task_tokens"], 1000)
        self.assertEqual(result["overall"]["retrieval_median_task_tokens"], 600)

    def test_correctness_failure_disables_overall_savings_claim(self) -> None:
        runs = []
        for task in run_quick.QUICK_TASKS:
            runs.append(_run(task, "baseline", 1000))
            runs.append(
                _run(
                    task,
                    "retrieval",
                    500,
                    correctness=task != "py-02",
                )
            )

        result = run_quick.analyze_quick(runs)

        self.assertFalse(result["overall"]["correctness_gate"])
        self.assertIsNone(result["overall"]["saved_tokens"])
        self.assertIsNone(result["overall"]["saved_percent"])
        self.assertFalse(result["tasks"]["py-02"]["comparable"])
        self.assertIsNone(result["tasks"]["py-02"]["saved_percent"])

    def test_rejects_incomplete_matrix(self) -> None:
        runs = [
            _run(task, arm, 1000)
            for task in run_quick.QUICK_TASKS
            for arm in run_quick.QUICK_ARMS
        ]
        runs.pop()

        with self.assertRaisesRegex(ValueError, "incomplete"):
            run_quick.analyze_quick(runs)

    def test_dry_run_has_six_planned_calls(self) -> None:
        report = run_quick.dry_run_report()
        self.assertEqual(report["planned_model_calls"], 6)
        self.assertEqual(report["model_calls"], 0)
        self.assertEqual(report["arms"], ["baseline", "retrieval"])


if __name__ == "__main__":
    unittest.main()
