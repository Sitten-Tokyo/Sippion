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
