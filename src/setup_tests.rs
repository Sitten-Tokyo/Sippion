use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir() -> PathBuf {
    let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = env::temp_dir().join(format!("sippion-setup-test-{id}"));
    fs::create_dir_all(&path).expect("temp directory");
    path
}

#[test]
fn codex_config_is_idempotent_and_escapes_unicode_spaces_and_backslashes() {
    let path = temp_dir().join("config.toml");
    let executable = Path::new(r"C:\Tools\日本語 project\sippion.exe");
    let block = format!(
        "{MANAGED_BEGIN}\n[mcp_servers.sippion]\ncommand = {}\n{MANAGED_END}\n",
        toml_string(executable.to_str().expect("unicode path"))
    );
    let first = upsert_codex_block("[other]\nvalue = true\n", &block).expect("first upsert");
    let second = upsert_codex_block(&first, &block).expect("second upsert");
    assert_eq!(first, second);
    assert!(first.contains(r#"C:\\Tools\\日本語 project\\sippion.exe"#));
    fs::write(&path, first).expect("write");
    assert!(path.exists());
}

#[test]
fn malformed_or_duplicate_managed_markers_fail_closed() {
    let malformed = format!("prefix\n{RULE_BEGIN}\nuser-owned-setting=true\n");
    assert!(upsert_marked_block(&malformed, RULE_BEGIN, RULE_END, "replacement").is_err());
    assert!(remove_block(&malformed, RULE_BEGIN, RULE_END).is_err());
    let duplicate = format!(
        "{RULE_BEGIN}\na\n{RULE_END}\nuser-owned-setting=true\n{RULE_BEGIN}\nb\n{RULE_END}\n"
    );
    assert!(upsert_marked_block(&duplicate, RULE_BEGIN, RULE_END, "replacement").is_err());
    assert!(remove_block(&duplicate, RULE_BEGIN, RULE_END).is_err());
}

#[test]
fn json_server_preserves_other_servers_and_is_removable() {
    let path = temp_dir().join("mcp_config.json");
    fs::write(&path, r#"{"mcpServers":{"other":{"command":"other"}}}"#).expect("write");
    let entry = json!({"command":"/tmp/sippion","args":["mcp","--root","."],"cwd":"."});
    assert_eq!(
        upsert_json_server(&path, entry).unwrap(),
        FileChange::Updated
    );
    let value = read_optional_json(&path).unwrap().unwrap();
    assert!(value["mcpServers"]["other"].is_object());
    assert_eq!(remove_json_server(&path).unwrap(), FileChange::Updated);
    assert!(read_optional_json(&path).unwrap().unwrap()["mcpServers"]["sippion"].is_null());
}

#[test]
fn marked_rule_handles_crlf_and_does_not_duplicate() {
    let initial = "existing\r\n";
    let block = format!("{RULE_BEGIN}\r\n# rule\r\n{RULE_END}\r\n");
    let first = upsert_marked_block(initial, RULE_BEGIN, RULE_END, &block).expect("first");
    let second = upsert_marked_block(&first, RULE_BEGIN, RULE_END, &block).expect("second");
    assert_eq!(first, second);
    assert_eq!(
        remove_block(&first, RULE_BEGIN, RULE_END).expect("remove"),
        initial
    );
}

#[test]
fn backup_tracks_the_immediately_previous_configuration() {
    let path = temp_dir().join("config.json");
    fs::write(&path, "one\n").expect("initial");
    write_bytes_with_backup(&path, b"two\n").expect("first update");
    write_bytes_with_backup(&path, b"three\n").expect("second update");
    assert_eq!(fs::read_to_string(&path).unwrap(), "three\n");
    assert_eq!(
        fs::read_to_string(path.with_file_name("config.json.sippion-backup")).unwrap(),
        "two\n"
    );
}

#[test]
fn setup_snapshot_rollback_restores_targets_and_backups() {
    let home = temp_dir();
    let target = home.join(".codex").join("config.toml");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, b"original\n").unwrap();
    let backup = sibling_backup_path(&target).unwrap();
    fs::write(&backup, b"older\n").unwrap();

    let snapshots = capture_setup_snapshots(&home).expect("snapshots");
    fs::write(&target, b"changed\n").unwrap();
    fs::write(&backup, b"changed-backup\n").unwrap();
    let newly_created = home.join(".claude.json");
    fs::write(&newly_created, b"new\n").unwrap();

    assert!(restore_snapshots(&snapshots).is_empty());
    assert_eq!(fs::read(&target).unwrap(), b"original\n");
    assert_eq!(fs::read(&backup).unwrap(), b"older\n");
    assert!(!newly_created.exists());
}
