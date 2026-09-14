# Context-efficiency evidence

This directory keeps machine-readable evidence for Sippion's model-visible
context packing. It does not contain coding-behavior prompt experiments or
model-run arms.

## Packed-atom ablation

`atom-ablation.json` records the deterministic 10/6/4/3 packed-atom comparison
used to select the production cap. Six atoms is the smallest tested cap that
preserves the repository correctness/evidence gates; caps four and three fail
the packed expected-path recall requirement.

The artifact records the exact deterministic gate and the observed failure
evidence. Broader retrieval evaluation is maintained by
`scripts/retrieval-eval.py` and documented in `docs/quality.md`.

A smaller model-visible context is only considered an improvement when the
correctness and evidence gates still pass. No token-reduction claim should be
published from this artifact alone.
