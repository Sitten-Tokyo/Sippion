#!/usr/bin/env python3
"""Calibrate Sippion's local token estimator against real provider observations.

Input is JSON with an `observations` array. Each observation must contain:
  - `text`: exact model-visible text that was tokenized
  - `observedTokens`: authoritative token count from the target provider/model
  - optional `label`: human-readable source name

No provider counts are fabricated by this script. Maintainers collect them from the
provider/model they want to validate, then run this tool locally or in a trusted CI job.
"""

import argparse
import json
import math
import statistics
from pathlib import Path


def sippion_estimate(text: str) -> int:
    if not text:
        return 0
    encoded_bytes = len(text.encode("utf-8"))
    non_ascii = sum(1 for ch in text if not ch.isascii())
    return math.ceil((encoded_bytes + non_ascii * 2) / 3)


def percentile(values, p):
    if not values:
        return 0.0
    ordered = sorted(values)
    index = (len(ordered) - 1) * p
    low = int(index)
    high = min(low + 1, len(ordered) - 1)
    fraction = index - low
    return ordered[low] * (1 - fraction) + ordered[high] * fraction


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("observations", help="JSON file containing real tokenizer observations")
    parser.add_argument(
        "--max-p95-relative-error",
        type=float,
        default=None,
        help="fail when p95 absolute relative error exceeds this fraction",
    )
    parser.add_argument(
        "--max-underestimate-rate",
        type=float,
        default=None,
        help="fail when the estimator underestimates this fraction of observations",
    )
    args = parser.parse_args()

    payload = json.loads(Path(args.observations).read_text(encoding="utf-8"))
    observations = payload.get("observations", [])
    if not observations:
        raise SystemExit("observations must contain at least one real provider token count")

    rows = []
    ratios = []
    absolute_relative_errors = []
    underestimates = 0
    for index, observation in enumerate(observations):
        text = observation.get("text")
        observed = observation.get("observedTokens")
        if not isinstance(text, str) or not isinstance(observed, int) or observed <= 0:
            raise SystemExit(
                f"observation {index} requires string text and positive integer observedTokens"
            )
        estimate = sippion_estimate(text)
        ratio = observed / max(estimate, 1)
        relative_error = (estimate - observed) / observed
        if estimate < observed:
            underestimates += 1
        ratios.append(ratio)
        absolute_relative_errors.append(abs(relative_error))
        rows.append(
            {
                "label": observation.get("label", f"observation-{index + 1}"),
                "estimatedTokens": estimate,
                "observedTokens": observed,
                "relativeError": round(relative_error, 4),
            }
        )

    # A multiplier >1 means the current estimator is optimistic for this corpus/model.
    # p95 ratio is a useful conservative calibration candidate for a soft target; the independent
    # byte cap remains Sippion's hard model-visible safety guard.
    recommended_multiplier = percentile(ratios, 0.95)
    p95_error = percentile(absolute_relative_errors, 0.95)
    underestimate_rate = underestimates / len(rows)
    summary = {
        "model": payload.get("model"),
        "tokenizer": payload.get("tokenizer"),
        "observations": len(rows),
        "meanAbsoluteRelativeError": round(statistics.fmean(absolute_relative_errors), 4),
        "p95AbsoluteRelativeError": round(p95_error, 4),
        "underestimateRate": round(underestimate_rate, 4),
        "recommendedP95Multiplier": round(recommended_multiplier, 4),
        "rows": rows,
    }
    print(json.dumps(summary, indent=2, ensure_ascii=False))

    failures = []
    if args.max_p95_relative_error is not None and p95_error > args.max_p95_relative_error:
        failures.append(
            f"p95 relative error {p95_error:.3f} > {args.max_p95_relative_error:.3f}"
        )
    if (
        args.max_underestimate_rate is not None
        and underestimate_rate > args.max_underestimate_rate
    ):
        failures.append(
            f"underestimate rate {underestimate_rate:.3f} > {args.max_underestimate_rate:.3f}"
        )
    if failures:
        raise SystemExit("; ".join(failures))


if __name__ == "__main__":
    main()
