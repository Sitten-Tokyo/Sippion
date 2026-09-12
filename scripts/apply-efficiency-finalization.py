#!/usr/bin/env python3
from pathlib import Path


def replace_once(path: Path, old: str, new: str) -> None:
    text = path.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one replacement target, found {count}")
    path.write_text(text.replace(old, new, 1))


setup = Path("src/setup.rs")
old_rule = 'const EFFICIENCY_RULE: &str = "Build the smallest correct solution after understanding the relevant flow. Reuse existing code first; then prefer the standard library, native platform features, and installed dependencies. Avoid unrequested abstractions, dependencies, boilerplate, and speculative future-proofing. Preserve required validation, security, data-loss protection, and requested behavior; leave one runnable check for non-trivial logic. Ask one concise question only when ambiguity materially changes the result or a requested technology appears unnecessary, unless the user says not to ask. Otherwise choose the clearly reasonable default and proceed. Lead with the result or next action; no preamble. Keep explanations and choices minimal. In reviews, report only concrete correctness, security, performance, or maintainability issues; omit style-only and speculative concerns. If none exist, say so briefly. Reply in the user\'s language.";'
new_rule = 'const EFFICIENCY_RULE: &str = "Build the smallest correct solution after understanding the relevant flow. Reuse existing code first; then prefer the standard library, native platform features, and installed dependencies. Fix root causes, not symptoms, and inspect affected callers before changing shared behavior. Avoid unrequested abstractions, dependencies, boilerplate, and speculative future-proofing. Preserve required validation, security, data-loss protection, and requested behavior; leave one runnable check for non-trivial logic. Ask one concise question only when ambiguity materially changes the result or a requested technology appears unnecessary, unless the user says not to ask. Otherwise choose the clearly reasonable default and proceed. Lead with the result or next action; no preamble. Keep explanations and choices minimal. In reviews, report only concrete correctness, security, performance, or maintainability issues; omit style-only and speculative concerns. If none exist, say so briefly. Reply in the user\'s language.";'
replace_once(setup, old_rule, new_rule)

context = Path("src/service/context.rs")
old_cap = """// Keep broad repositories from turning semantic expansion into a large model-visible file list.
// Ten atoms remain the conservative ceiling until the efficiency benchmark proves a smaller cap
// preserves correctness.
const MAX_PACKED_ATOMS: usize = 10;"""
new_cap = """// The 10/6/4/3 deterministic ablation keeps six as the smallest cap that preserves the
// packed expected-path and evidence gates. Caps four and three lose required packed paths.
const MAX_PACKED_ATOMS: usize = 6;"""
replace_once(context, old_cap, new_cap)
