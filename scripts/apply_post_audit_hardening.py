#!/usr/bin/env python3
from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def write(path: str, text: str) -> None:
    target = ROOT / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(text, encoding="utf-8")


def replace_once(path: str, old: str, new: str) -> None:
    text = read(path)
    if new in text:
        return
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{path}: expected exactly one replacement target, found {count}")
    write(path, text.replace(old, new, 1))


def regex_replace_once(path: str, pattern: str, replacement: str) -> None:
    text = read(path)
    if replacement in text:
        return
    updated, count = re.subn(pattern, replacement, text, count=1, flags=re.S)
    if count != 1:
        raise RuntimeError(f"{path}: regex replacement target count={count}")
    write(path, updated)


def append_once(path: str, marker: str, addition: str) -> None:
    text = read(path)
    if marker in text:
        return
    if not text.endswith("\n"):
        text += "\n"
    write(path, text + addition)


def write_if_missing(path: str, marker: str, content: str) -> None:
    target = ROOT / path
    if target.exists() and marker in target.read_text(encoding="utf-8"):
        return
    write(path, content)


# Release version and Unicode search dependencies.
replace_once("Cargo.toml", 'version = "0.1.0-rc.33"', 'version = "0.1.0-rc.34"')
replace_once(
    "Cargo.toml",
    'tree-sitter-go = "=0.25.0"\n',
    'tree-sitter-go = "=0.25.0"\nunicode-casefold = "=0.2.0"\nunicode-normalization = "=0.1.25"\n',
)

# Full default Unicode case folding plus compatibility decomposition while retaining source-byte
# provenance for excerpt offsets. Security path policy remains deliberately ASCII-only elsewhere.
replace_once(
    "src/core.rs",
    'use serde_json::{Value, json};\n',
    'use serde_json::{Value, json};\nuse unicode_casefold::UnicodeCaseFold;\nuse unicode_normalization::char::decompose_compatible;\n',
)
regex_replace_once(
    "src/core.rs",
    r'/// Unicode-aware lowercase used only for retrieval/ranking equivalence\..*?(?=/// The server process)',
    '''/// Compatibility-decomposed full Unicode case folding used only for retrieval/ranking\n/// equivalence. Security path policy keeps its deliberately narrower ASCII folding so filesystem\n/// policy semantics do not change here. Folding each original scalar independently preserves a\n/// precise source-byte provenance map while still covering full folds such as ß -> ss and common\n/// composed/decomposed compatibility-equivalent spellings.\nfn fold_search_scalar(ch: char, mut emit: impl FnMut(char)) {\n    decompose_compatible(ch, |decomposed| {\n        for folded in decomposed.case_fold() {\n            decompose_compatible(folded, &mut emit);\n        }\n    });\n}\n\n#[must_use]\npub(crate) fn unicode_search_fold(text: &str) -> String {\n    if text.is_ascii() {\n        return text.to_ascii_lowercase();\n    }\n    let mut folded = String::with_capacity(text.len());\n    for ch in text.chars() {\n        fold_search_scalar(ch, |folded_ch| folded.push(folded_ch));\n    }\n    folded\n}\n\n/// Finds a folded search term while returning the byte offset in the original UTF-8 text. Full\n/// case folding and compatibility decomposition can change encoded length, so callers that need\n/// source excerpts must never use the folded string's byte position directly.\n#[must_use]\npub(crate) fn unicode_search_fold_find_byte(text: &str, folded_needle: &str) -> Option<usize> {\n    if folded_needle.is_empty() {\n        return Some(0);\n    }\n    if text.is_ascii() && folded_needle.is_ascii() {\n        return text.to_ascii_lowercase().find(folded_needle);\n    }\n\n    let mut folded = String::with_capacity(text.len());\n    let mut folded_byte_to_source = Vec::with_capacity(text.len());\n    for (source_byte, ch) in text.char_indices() {\n        let before = folded.len();\n        fold_search_scalar(ch, |folded_ch| folded.push(folded_ch));\n        folded_byte_to_source.extend(std::iter::repeat_n(source_byte, folded.len() - before));\n    }\n    let folded_byte = folded.find(folded_needle)?;\n    folded_byte_to_source.get(folded_byte).copied()\n}\n\n''',
)
core_tests = r'''

    #[test]
    fn unicode_search_fold_handles_full_casefold_and_compatibility_decomposition() {
        assert_eq!(unicode_search_fold("Straße"), "strasse");
        assert_eq!(
            unicode_search_fold("CAFÉ"),
            unicode_search_fold("Cafe\u{301}")
        );
        assert_eq!(unicode_search_fold("ﬃAuth"), "ffiauth");
        assert_eq!(unicode_search_fold("Σςσ"), "σσσ");
    }

    #[test]
    fn unicode_fold_find_property_corpus_preserves_source_offsets() {
        let alphabet = ['A', 'ß', 'é', '\u{301}', 'Σ', 'ς', 'K', 'ﬃ', '認', '証', '_', '9'];
        let mut state = 0x9e37_79b9_u64;
        for _ in 0..256 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let len = 1 + (state as usize % 8);
            let mut sample = String::new();
            for _ in 0..len {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                sample.push(alphabet[state as usize % alphabet.len()]);
            }
            let folded = unicode_search_fold(&sample);
            assert_eq!(unicode_search_fold(&folded), folded);
            let text = format!("prefix|{sample}|suffix");
            let offset = unicode_search_fold_find_byte(&text, &folded).expect("folded match");
            assert_eq!(offset, "prefix|".len(), "sample={sample:?}");
        }
    }
'''
if "unicode_fold_find_property_corpus_preserves_source_offsets" not in read("src/core.rs"):
    text = read("src/core.rs")
    closing = text.rfind("\n}")
    if closing < 0:
        raise RuntimeError("src/core.rs: final test-module brace not found")
    write("src/core.rs", text[:closing] + core_tests + text[closing:])

# Ignore completeness must match every ignore source the walker honors. Symlinked/unreadable
# controls remain conservative rather than allowing an absolute NO_MATCH.
replace_once(
    "src/repo/access.rs",
    '''    if !metadata.file_type().is_file() || metadata.len() == 0 {\n        return false;\n    }\n''',
    '''    if metadata.file_type().is_symlink() {\n        return true;\n    }\n    if !metadata.file_type().is_file() || metadata.len() == 0 {\n        return false;\n    }\n''',
)
replace_once(
    "src/repo/access.rs",
    '''        let root_ignore_sentinel = if has_effective_ignore_control(&root_path) {\n            1\n        } else {\n            0\n        };\n''',
    '''        let git_info_exclude = root_path.join(".git").join("info").join("exclude");\n        let root_ignore_sentinel = if has_effective_ignore_control(&root_path)\n            || ignore_control_has_effective_rule(&git_info_exclude)\n        {\n            1\n        } else {\n            0\n        };\n''',
)

# Sensitive literal names that are common in frameworks and deployment configuration.
replace_once(
    "src/repo.rs",
    '''    "secret",\n    "token",\n''',
    '''    "secret",\n    "secret_key",\n    "secretkey",\n    "secret_key_base",\n    "secretkeybase",\n    "signing_key",\n    "signingkey",\n    "encryption_key",\n    "encryptionkey",\n    "token",\n''',
)

# Unicode substring candidate sketches now preserve adjacency/order instead of using an unordered
# set of scalar hashes.
regex_replace_once(
    "src/repo/ranking.rs",
    r'pub\(super\) fn unicode_scalar_gram_key.*?(?=pub\(super\) fn add_index_term)',
    '''pub(super) fn unicode_sequence_gram_key(window: &[char]) -> u32 {\n    // ASCII substring keys use top-byte namespaces 2 and 3. Reserve the high bit for Unicode\n    // sequence sketches and include both sequence length and scalar order in the hash.\n    let mut hash = 0x811c9dc5u32 ^ window.len() as u32;\n    for ch in window {\n        let mut encoded = [0u8; 4];\n        for byte in ch.encode_utf8(&mut encoded).as_bytes() {\n            hash ^= u32::from(*byte);\n            hash = hash.wrapping_mul(0x01000193);\n        }\n        hash ^= 0xff;\n        hash = hash.wrapping_mul(0x01000193);\n    }\n    0x8000_0000 | (hash & 0x7fff_ffff)\n}\n\npub(super) fn query_substring_grams(term: &str) -> Vec<u32> {\n    if term.is_ascii() {\n        if term.len() < 2 {\n            return Vec::new();\n        }\n        let bytes = term.as_bytes();\n        let width = if bytes.len() == 2 { 2 } else { 3 };\n        let mut grams = bytes\n            .windows(width)\n            .map(substring_gram_key)\n            .collect::<Vec<_>>();\n        grams.sort_unstable();\n        grams.dedup();\n        return grams;\n    }\n\n    // Full Unicode folding happens before this stage. Use the longest available one/two/three\n    // scalar window so a candidate must preserve local order and adjacency rather than merely\n    // contain the same set of scalars. Exact source verification remains authoritative.\n    let chars = term.chars().collect::<Vec<_>>();\n    if chars.is_empty() {\n        return Vec::new();\n    }\n    let width = chars.len().min(3);\n    let mut grams = chars\n        .windows(width)\n        .map(unicode_sequence_gram_key)\n        .collect::<Vec<_>>();\n    grams.sort_unstable();\n    grams.dedup();\n    grams\n}\n\n''',
)
replace_once(
    "src/repo/ranking.rs",
    '''        // Candidate sketches never retain source bodies or plaintext tokens. ASCII keeps compact\n        // two/three-byte grams. Tokens containing Unicode additionally get hashed scalar sketches,\n        // so queries such as "認証" can nominate "ユーザー認証処理" for exact source verification.\n        // ASCII runs inside mixed identifiers also keep ordinary substring recall.\n''',
    '''        // Candidate sketches never retain source bodies or plaintext tokens. ASCII keeps compact\n        // two/three-byte grams. Tokens containing Unicode additionally get ordered one/two/three\n        // scalar sequence sketches, so substring recall is retained without discarding adjacency.\n        // ASCII runs inside mixed identifiers also keep ordinary substring recall.\n''',
)
regex_replace_once(
    "src/repo/ranking.rs",
    r'''        if !lower\.is_ascii\(\) \{\n            for ch in lower\.chars\(\) \{\n                if substring_grams\.len\(\) >= MAX_INDEX_SUBSTRING_GRAMS_PER_FILE \{\n                    term_truncated = true;\n                    break;\n                \}\n                substring_grams\.insert\(unicode_scalar_gram_key\(ch\)\);\n            \}\n        \}\n''',
    '''        if !lower.is_ascii() {\n            let chars = lower.chars().collect::<Vec<_>>();\n            for width in 1..=chars.len().min(3) {\n                for window in chars.windows(width) {\n                    if substring_grams.len() >= MAX_INDEX_SUBSTRING_GRAMS_PER_FILE {\n                        term_truncated = true;\n                        break;\n                    }\n                    substring_grams.insert(unicode_sequence_gram_key(window));\n                }\n                if term_truncated && substring_grams.len() >= MAX_INDEX_SUBSTRING_GRAMS_PER_FILE {\n                    break;\n                }\n            }\n        }\n''',
)
append_once(
    "src/repo/tests.rs",
    "unicode_substring_grams_preserve_sequence_order",
    r'''

#[test]
fn unicode_substring_grams_preserve_sequence_order() {
    let forward = query_substring_grams(&crate::core::unicode_search_fold("認証処理"));
    let reordered = query_substring_grams(&crate::core::unicode_search_fold("証認処理"));
    assert_ne!(forward, reordered);
    assert!(!forward.is_empty());
}
''',
)

# Make every untrusted field in the compact structural grammar self-delimiting.
replace_once(
    "src/service.rs",
    '"STRUCTURE format=sippion-struct-v4 syntax=tree-sitter+source-only-semantic-weighted-graph+heuristic-fallback\\n",',
    '"STRUCTURE format=sippion-struct-v5 syntax=tree-sitter+source-only-semantic-weighted-graph+heuristic-fallback\\n",',
)
regex_replace_once(
    "src/service.rs",
    r'''        let links = if entry\.semantic_links\.is_empty\(\) \{.*?        \};\n        let mut block = format!\("FILE path=\{path\} rank=\{:\.3\} links=\{\}\\n", entry\.score, links\);\n        for symbol in entry\.symbols\.iter\(\)\.take\(4\) \{.*?        \}\n''',
    '''        let links = if entry.semantic_links.is_empty() {\n            if entry.links_to.is_empty() {\n                "-".to_string()\n            } else {\n                entry\n                    .links_to\n                    .iter()\n                    .take(4)\n                    .map(|path| {\n                        serde_json::to_string(path)\n                            .unwrap_or_else(|_| "\\\"<invalid-path>\\\"".to_string())\n                    })\n                    .collect::<Vec<_>>()\n                    .join(",")\n            }\n        } else {\n            entry\n                .semantic_links\n                .iter()\n                .take(5)\n                .map(|link| {\n                    let relative_path = serde_json::to_string(&link.relative_path)\n                        .unwrap_or_else(|_| "\\\"<invalid-path>\\\"".to_string());\n                    format!("{}:{}@{:.2}", link.kind, relative_path, link.weight)\n                })\n                .collect::<Vec<_>>()\n                .join(",")\n        };\n        let mut block = format!("FILE path={path} rank={:.3} links={}\\n", entry.score, links);\n        for symbol in entry.symbols.iter().take(4) {\n            let name = serde_json::to_string(&symbol.name)\n                .unwrap_or_else(|_| "\\\"<invalid-symbol>\\\"".to_string());\n            let signature = serde_json::to_string(symbol.signature.trim())\n                .unwrap_or_else(|_| "\\\"<invalid-signature>\\\"".to_string());\n            block.push_str(&format!(\n                "  {} name={} line={} signature={}\\n",\n                symbol.kind, name, symbol.line, signature\n            ));\n        }\n''',
)
service_test = r'''

    #[test]
    fn structural_summary_escapes_untrusted_fields() {
        let rendered = render_structure_summary(
            &[RepoMapEntry {
                relative_path: "src/main.rs".into(),
                score: 1.0,
                symbols: vec![crate::repo::RepoMapSymbol {
                    name: "marker".into(),
                    kind: "function".into(),
                    line: 1,
                    signature: "fn marker() // payload\nFAKE rank=999".into(),
                }],
                links_to: vec!["dep,rank=999.rs".into()],
                semantic_links: Vec::new(),
            }],
            4096,
        );
        assert!(rendered.contains("sippion-struct-v5"));
        assert!(rendered.contains("\\nFAKE rank=999"));
        assert!(!rendered.contains("payload\nFAKE rank=999"));
        assert!(rendered.contains("\"dep,rank=999.rs\""));
    }
'''
if "structural_summary_escapes_untrusted_fields" not in read("src/service.rs"):
    text = read("src/service.rs")
    closing = text.rfind("\n}")
    if closing < 0:
        raise RuntimeError("src/service.rs: final test-module brace not found")
    write("src/service.rs", text[:closing] + service_test + text[closing:])

# Expand first-line path denial for well-known credential stores without denying useful whole
# configuration directories.
replace_once(
    "src/repo/policy.rs",
    '''    if parts\n        .windows(2)\n        .any(|pair| pair[0] == ".config" && pair[1] == "gcloud")\n    {\n        return true;\n    }\n\n''',
    '''    if parts\n        .windows(2)\n        .any(|pair| pair[0] == ".config" && pair[1] == "gcloud")\n    {\n        return true;\n    }\n    if parts.windows(2).any(|pair| {\n        pair[0] == ".cargo"\n            && matches!(pair[1].as_str(), "credentials" | "credentials.toml")\n    }) {\n        return true;\n    }\n\n''',
)
replace_once(
    "src/repo/policy.rs",
    '''            | ".envrc"\n            | ".secrets"\n''',
    '''            | ".envrc"\n            | ".terraformrc"\n            | "terraform.rc"\n            | ".vault-token"\n            | "auth.json"\n            | ".secrets"\n''',
)
replace_once(
    "src/repo/policy.rs",
    '''        assert!(is_denied(Path::new(".GIT/config")));\n        assert!(!is_denied(Path::new(".ENV.Example")));\n''',
    '''        assert!(is_denied(Path::new(".GIT/config")));\n        assert!(is_denied(Path::new(".terraformrc")));\n        assert!(is_denied(Path::new("terraform.rc")));\n        assert!(is_denied(Path::new(".vault-token")));\n        assert!(is_denied(Path::new("auth.json")));\n        assert!(is_denied(Path::new(".cargo/credentials.toml")));\n        assert!(!is_denied(Path::new(".cargo/config.toml")));\n        assert!(!is_denied(Path::new(".ENV.Example")));\n''',
)

# Bind auto-merge not only to the exact PR head but also to the exact base SHA against which the
# successful pull-request CI workflow ran. CI publishes the pair as a run-scoped artifact name.
replace_once(
    ".github/workflows/author-auto-merge.yml",
    '''permissions:\n  contents: write\n  pull-requests: write\n''',
    '''permissions:\n  actions: read\n  contents: write\n  pull-requests: write\n''',
)
replace_once(
    ".github/workflows/author-auto-merge.yml",
    '''          RUN_HEAD_SHA: ${{ github.event.workflow_run.head_sha }}\n''',
    '''          RUN_HEAD_SHA: ${{ github.event.workflow_run.head_sha }}\n          RUN_ID: ${{ github.event.workflow_run.id }}\n''',
)
replace_once(
    ".github/workflows/author-auto-merge.yml",
    '''          jq -e \\\n            --arg repository "$REPOSITORY" \\\n            --arg expected_head_sha "$RUN_HEAD_SHA" '\n              .state == "open"\n              and .draft == false\n              and .base.ref == "main"\n              and .user.login == "Sitten-Tokyo"\n              and .user.type == "User"\n              and .head.repo.full_name == $repository\n              and .head.repo.fork == false\n              and .head.sha == $expected_head_sha\n            ' <<<"$pr_json" >/dev/null\n\n          echo "number=$pr_number" >> "$GITHUB_OUTPUT"\n''',
    '''          jq -e \\\n            --arg repository "$REPOSITORY" \\\n            --arg expected_head_sha "$RUN_HEAD_SHA" '\n              .state == "open"\n              and .draft == false\n              and .base.ref == "main"\n              and .user.login == "Sitten-Tokyo"\n              and .user.type == "User"\n              and .head.repo.full_name == $repository\n              and .head.repo.fork == false\n              and .head.sha == $expected_head_sha\n            ' <<<"$pr_json" >/dev/null\n\n          current_base_sha="$(jq -r '.base.sha' <<<"$pr_json")"\n          if [[ ! "$current_base_sha" =~ ^[0-9a-f]{40}$ ]]; then\n            echo "Invalid current PR base SHA" >&2\n            exit 1\n          fi\n          binding_name="ci-binding-${current_base_sha}-${RUN_HEAD_SHA}"\n          artifacts_json="$(gh api "repos/${REPOSITORY}/actions/runs/${RUN_ID}/artifacts?per_page=100")"\n          jq -e --arg binding_name "$binding_name" '\n            [ .artifacts[]\n              | select(.name == $binding_name and .expired == false)\n            ]\n            | length == 1\n          ' <<<"$artifacts_json" >/dev/null || {\n            echo "Successful CI was not bound to current base/head pair: $binding_name" >&2\n            exit 1\n          }\n\n          echo "number=$pr_number" >> "$GITHUB_OUTPUT"\n''',
)

# CI: persist PR base/head binding, verify README bootstrap pins, and add dependency policy checks.
replace_once(
    ".github/workflows/ci.yml",
    '''      - name: Install pinned Rust toolchain\n''',
    '''      - name: Validate README bootstrap pins\n        if: runner.os == 'Linux'\n        env:\n          GH_TOKEN: ${{ github.token }}\n        run: python3 scripts/check-bootstrap-pins.py\n\n      - name: Install pinned Rust toolchain\n''',
)
replace_once(
    ".github/workflows/ci.yml",
    '''  security-audit:\n''',
    '''  pr-binding:\n    name: Bind PR base/head for auto-merge\n    if: github.event_name == 'pull_request'\n    runs-on: ubuntu-24.04\n    permissions:\n      contents: read\n    steps:\n      - name: Record tested PR base/head\n        env:\n          BASE_SHA: ${{ github.event.pull_request.base.sha }}\n          HEAD_SHA: ${{ github.event.pull_request.head.sha }}\n        shell: bash\n        run: |\n          set -euo pipefail\n          printf 'base=%s\\nhead=%s\\n' "$BASE_SHA" "$HEAD_SHA" > ci-binding.txt\n\n      - name: Upload base/head binding\n        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7\n        with:\n          name: ci-binding-${{ github.event.pull_request.base.sha }}-${{ github.event.pull_request.head.sha }}\n          path: ci-binding.txt\n          if-no-files-found: error\n          retention-days: 1\n\n  security-audit:\n''',
)
replace_once(
    ".github/workflows/ci.yml",
    '''      - name: Audit Cargo.lock against RustSec\n        run: cargo audit --file Cargo.lock\n''',
    '''      - name: Audit Cargo.lock against RustSec\n        run: cargo audit --file Cargo.lock\n\n      - name: Install pinned cargo-deny\n        run: cargo install cargo-deny --version 0.20.2 --locked\n\n      - name: Enforce dependency licenses and sources\n        run: cargo deny check licenses sources bans\n''',
)

write_if_missing(
    "scripts/check-bootstrap-pins.py",
    "Bootstrap README pins verified",
    r'''#!/usr/bin/env python3
from __future__ import annotations

import os
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = os.environ.get("GITHUB_REPOSITORY", "Sitten-Tokyo/Sippion")
PIN = re.compile(
    r"https://raw\.githubusercontent\.com/Sitten-Tokyo/Sippion/([0-9a-f]{40})/scripts/(bootstrap\.(?:sh|ps1))"
)

observed: list[tuple[str, str, str]] = []
for readme in ("README.md", "README.ja.md"):
    text = (ROOT / readme).read_text(encoding="utf-8")
    matches = PIN.findall(text)
    scripts = {script for _, script in matches}
    if scripts != {"bootstrap.sh", "bootstrap.ps1"}:
        raise SystemExit(f"{readme}: expected pinned bootstrap.sh and bootstrap.ps1 URLs")
    for sha, script in matches:
        observed.append((readme, sha, script))

shas = {sha for _, sha, _ in observed}
if len(shas) != 1:
    raise SystemExit(f"README bootstrap pins disagree: {sorted(shas)}")
pin = shas.pop()

subprocess.run(
    ["gh", "api", f"repos/{REPOSITORY}/commits/{pin}", "--jq", ".sha"],
    check=True,
    stdout=subprocess.DEVNULL,
)
for script in ("scripts/bootstrap.sh", "scripts/bootstrap.ps1"):
    current = subprocess.check_output(
        ["git", "hash-object", script], cwd=ROOT, text=True
    ).strip()
    pinned = subprocess.check_output(
        [
            "gh",
            "api",
            f"repos/{REPOSITORY}/contents/{script}?ref={pin}",
            "--jq",
            ".sha",
        ],
        text=True,
    ).strip()
    if current != pinned:
        raise SystemExit(
            f"{script}: README pin {pin} resolves to blob {pinned}, current blob is {current}; update the README pin"
        )

print(f"Bootstrap README pins verified at {pin}")
''',
)

write_if_missing(
    "deny.toml",
    "unknown-registry = \"deny\"",
    '''[licenses]\nallow = [\n  "Apache-2.0",\n  "Apache-2.0 WITH LLVM-exception",\n  "BSD-2-Clause",\n  "BSD-3-Clause",\n  "CC0-1.0",\n  "ISC",\n  "MIT",\n  "MIT-0",\n  "Unicode-3.0",\n  "Unicode-DFS-2016",\n  "Zlib",\n]\nconfidence-threshold = 0.8\n\n[bans]\nmultiple-versions = "warn"\nwildcards = "deny"\nallow = []\ndeny = []\nskip = []\nskip-tree = []\n\n[sources]\nunknown-registry = "deny"\nunknown-git = "deny"\nallow-registry = ["https://github.com/rust-lang/crates.io-index"]\nallow-git = []\n''',
)

# End-to-end regressions for the newly closed correctness and disclosure gaps.
write_if_missing(
    "tests/post_audit_regressions.rs",
    "git_info_exclude_prevents_absolute_no_match",
    r'''use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_root(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("sippion-post-audit-{label}-{nonce}"));
    std::fs::create_dir_all(&root).expect("create test repository");
    root
}

fn query(root: &std::path::Path, q: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_sippion"))
        .args(["mcp", "--root"])
        .arg(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start sippion");

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {}
            },
            "name": "repo_context",
            "arguments": {"q": q}
        }
    });
    {
        let stdin = child.stdin.as_mut().expect("child stdin");
        serde_json::to_writer(&mut *stdin, &request).expect("serialize request");
        stdin.write_all(b"\n").expect("write newline");
        stdin.flush().expect("flush request");
    }

    let stdout = child.stdout.take().expect("child stdout");
    let mut reader = BufReader::new(stdout);
    let mut response_line = String::new();
    reader.read_line(&mut response_line).expect("read MCP response");
    assert!(!response_line.is_empty());
    drop(child.stdin.take());
    let status = child.wait().expect("wait for sippion");
    assert!(status.success());

    let response: serde_json::Value = serde_json::from_str(response_line.trim()).expect("JSON-RPC");
    response["result"]["content"][0]["text"]
        .as_str()
        .expect("model-visible text")
        .to_string()
}

#[test]
fn git_info_exclude_prevents_absolute_no_match() {
    let root = temp_root("git-info-exclude");
    std::fs::create_dir_all(root.join(".git/info")).expect("git info");
    std::fs::write(root.join(".git/info/exclude"), "hidden.rs\n").expect("exclude");
    std::fs::write(root.join("visible.rs"), "fn ordinary_marker() {}\n").expect("visible");
    std::fs::write(root.join("hidden.rs"), "fn git_exclude_marker() {}\n").expect("hidden");

    let text = query(&root, "git_exclude_marker");
    assert!(text.contains("NO_MATCH_IN_SEARCHABLE_SET"));
    assert!(!text.contains("\n[NO_MATCH]\n"));
    assert!(!text.contains("policy_excluded=0"));
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn framework_secret_keys_are_redacted() {
    let root = temp_root("secret-key");
    let secret = "django-production-secret-value-123456";
    let secret_base = "rails-secret-key-base-value-654321";
    std::fs::write(
        root.join("settings.rs"),
        format!(
            "const SECRET_KEY: &str = \"{secret}\"; const SECRET_KEY_BASE: &str = \"{secret_base}\"; fn django_secret_marker() {{}}\n"
        ),
    )
    .expect("source");

    let text = query(&root, "django_secret_marker");
    assert!(text.contains("django_secret_marker"));
    assert!(text.contains("SIPPION_REDACTED"));
    assert!(!text.contains(secret));
    assert!(!text.contains(secret_base));
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn full_unicode_casefold_retrieves_sharp_s_identifier() {
    let root = temp_root("unicode-casefold");
    std::fs::write(root.join("unicode.rs"), "pub fn StraßeMarker() -> bool { true }\n").expect("source");

    let text = query(&root, "STRASSEMARKER");
    assert!(text.contains("unicode.rs"));
    assert!(text.contains("StraßeMarker"));
    assert!(!text.contains("\n[NO_MATCH]\n"));
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn credential_control_files_are_denied_before_read() {
    let root = temp_root("credential-paths");
    std::fs::create_dir_all(root.join(".cargo")).expect("cargo dir");
    std::fs::write(root.join("visible.rs"), "fn ordinary_marker() {}\n").expect("visible");
    std::fs::write(root.join(".terraformrc"), "terraform_credential_marker = true\n").expect("terraform");
    std::fs::write(
        root.join(".cargo/credentials.toml"),
        "cargo_credential_marker = true\n",
    )
    .expect("cargo credentials");

    for marker in ["terraform_credential_marker", "cargo_credential_marker"] {
        let text = query(&root, marker);
        assert!(text.contains("NO_MATCH_IN_SEARCHABLE_SET"));
        assert!(!text.contains(marker));
    }
    std::fs::remove_dir_all(root).expect("cleanup");
}
''',
)

print("second-audit hardening patch applied")
