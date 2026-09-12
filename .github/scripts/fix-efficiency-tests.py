from pathlib import Path

service_path = Path("src/service.rs")
service = service_path.read_text()
old = '''        assert!(output.contains("[NO_MATCH_IN_SEARCHABLE_SET:"));
        assert!(output.contains("excluded=1"));
        assert!(output.contains("CTX v=4"));'''
new = '''        assert!(output.contains("[UNTRUSTED CODE; INCOMPLETE]"));
        assert!(output.contains("[NO_MATCH_IN_SEARCHABLE_SET:"));
        assert!(!output.contains("excluded="));
        assert!(!output.contains("CTX v="));'''
if service.count(old) != 1:
    raise SystemExit("service compact-context test anchor mismatch")
service_path.write_text(service.replace(old, new, 1))

setup_tests_path = Path("src/setup_tests.rs")
setup_tests = setup_tests_path.read_text()
old_fixture = r'''r#"{"model":"example/model","mcp":{"other":{"type":"remote","url":"https://example.invalid/mcp"}}}}"#'''
new_fixture = r'''r#"{"model":"example/model","mcp":{"other":{"type":"remote","url":"https://example.invalid/mcp"}}}"#'''
if setup_tests.count(old_fixture) != 1:
    raise SystemExit("OpenCode JSON fixture anchor mismatch")
setup_tests_path.write_text(setup_tests.replace(old_fixture, new_fixture, 1))

ignore_path = Path("tests/ignore_completeness.rs")
ignore = ignore_path.read_text()
old_assert = '''fn assert_complete_no_match(text: &str) {
    assert!(text.contains("excluded=0"));
    assert!(text.contains("\\n[NO_MATCH]\\n"));
    assert!(!text.contains("NO_MATCH_IN_SEARCHABLE_SET"));
}'''
new_assert = '''fn assert_complete_no_match(text: &str) {
    assert!(text.contains("[UNTRUSTED CODE]"));
    assert!(!text.contains("INCOMPLETE"));
    assert!(!text.contains("excluded="));
    assert!(text.contains("\\n[NO_MATCH]\\n"));
    assert!(!text.contains("NO_MATCH_IN_SEARCHABLE_SET"));
}'''
if ignore.count(old_assert) != 1:
    raise SystemExit("ignore completeness anchor mismatch")
ignore_path.write_text(ignore.replace(old_assert, new_assert, 1))
