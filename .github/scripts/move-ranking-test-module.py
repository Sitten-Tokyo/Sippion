#!/usr/bin/env python3
from pathlib import Path

path = Path("src/repo/ranking.rs")
text = path.read_text(encoding="utf-8")
block = '''#[cfg(test)]
mod coding_source_prior_tests {
    use super::coding_source_prior;

    #[test]
    fn implementation_sources_get_a_small_coding_prior() {
        assert!(
            coding_source_prior("src/repo/map.rs") > coding_source_prior("docs/architecture.md")
        );
        assert_eq!(coding_source_prior("CHANGELOG.md"), 0.0);
    }
}

'''
if text.count(block) != 1:
    raise SystemExit(f"expected one test module, found {text.count(block)}")
text = text.replace(block, "", 1).rstrip() + "\n\n" + block.rstrip() + "\n"
path.write_text(text, encoding="utf-8")
