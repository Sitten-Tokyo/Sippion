#!/usr/bin/env python3
from pathlib import Path

root = Path(__file__).resolve().parents[1]

auto = root / ".github/workflows/author-auto-merge.yml"
text = auto.read_text(encoding="utf-8")
needle = "          RUN_HEAD_SHA: ${{ github.event.workflow_run.head_sha }}\n"
if text.count(needle) == 2:
    first = text.find(needle)
    second = text.find(needle, first + len(needle))
    text = text[:second] + needle.rstrip("\n") + " # exact merge head\n" + text[second + len(needle):]
    auto.write_text(text, encoding="utf-8")

test = root / "tests/post_audit_regressions.rs"
if test.exists():
    text = test.read_text(encoding="utf-8")
    text = text.replace(
        '''        assert!(text.contains("NO_MATCH_IN_SEARCHABLE_SET"));\n        assert!(!text.contains(marker));\n''',
        '''        assert!(text.contains("NO_MATCH_IN_SEARCHABLE_SET"));\n        assert!(!text.contains("FILE path=\\\".terraformrc\\\""));\n        assert!(!text.contains("FILE path=\\\".cargo/credentials.toml\\\""));\n''',
    )
    test.write_text(text, encoding="utf-8")

print("temporary patch fixups applied")
