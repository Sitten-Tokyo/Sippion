use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir() -> PathBuf {
    let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = env::temp_dir().join(format!("sippion-setup-test-{id}"));
    fs::create_dir_all(&path).expect("temp directory");
    path
}

fn fake_executable(home: &Path) -> PathBuf {
    home.join("bin").join(if cfg!(windows) {
        "sippion.exe"
    } else {
        "sippion"
    })
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
fn setup_failure_restores_all_targets_and_previous_backups() {
    let home = temp_dir();
    let executable = fake_executable(&home);
    let codex = home.join(".codex").join("config.toml");
    let claude = home.join(".claude.json");
    fs::create_dir_all(codex.parent().unwrap()).expect("codex dir");
    let malformed = format!("user=true\n{MANAGED_BEGIN}\nunterminated=true\n");
    fs::write(&codex, &malformed).expect("malformed codex");
    let original_claude = r#"{"mcpServers":{"other":{"command":"other"}}}"#;
    fs::write(&claude, original_claude).expect("claude config");
    let claude_backup = backup_path(&claude).expect("backup path");
    fs::write(&claude_backup, "preexisting-backup\n").expect("previous backup");

    let error = run_setup_at(&home, &executable).expect_err("malformed config must fail setup");
    assert!(error.contains("all managed setup files were restored"));
    assert_eq!(fs::read_to_string(&codex).unwrap(), malformed);
    assert_eq!(fs::read_to_string(&claude).unwrap(), original_claude);
    assert_eq!(
        fs::read_to_string(&claude_backup).unwrap(),
        "preexisting-backup\n"
    );
    assert!(!home.join(".gemini").join("config").join("mcp_config.json").exists());
    assert!(!home.join(".codex").join("AGENTS.md").exists());
    assert!(!home.join(".claude").join("CLAUDE.md").exists());
    assert!(!home.join(".gemini").join("GEMINI.md").exists());
}

#[test]
fn doctor_reports_unhealthy_until_every_managed_entry_matches() {
    let home = temp_dir();
    let executable = fake_executable(&home);
    assert!(!doctor::run_checks(&home, &executable));
    run_setup_at(&home, &executable).expect("setup");
    assert!(doctor::run_checks(&home, &executable));
}

#[cfg(unix)]
#[test]
fn setup_refuses_symlinked_targets_and_backup_destinations() {
    use std::os::unix::fs::symlink;

    let root = temp_dir();
    let external = root.join("external.txt");
    fs::write(&external, "outside\n").expect("external");
    let linked = root.join("config.toml");
    symlink(&external, &linked).expect("target symlink");
    assert!(write_bytes_with_backup(&linked, b"replacement\n").is_err());
    assert_eq!(fs::read_to_string(&external).unwrap(), "outside\n");

    let regular = root.join("regular.json");
    fs::write(&regular, "original\n").expect("regular");
    let backup = backup_path(&regular).expect("backup");
    let backup_target = root.join("backup-target.txt");
    fs::write(&backup_target, "do-not-touch\n").expect("backup target");
    symlink(&backup_target, &backup).expect("backup symlink");
    assert!(write_bytes_with_backup(&regular, b"replacement\n").is_err());
    assert_eq!(fs::read_to_string(&regular).unwrap(), "original\n");
    assert_eq!(fs::read_to_string(&backup_target).unwrap(), "do-not-touch\n");
}
