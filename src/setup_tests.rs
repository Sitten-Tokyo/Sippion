use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

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
        "{MANAGED_BEGIN}\n[mcp_servers.sippion]\ncommand = {}\n{ROOT_AUTO_TOML_ARGS}\n{MANAGED_END}\n",
        toml_string(executable.to_str().expect("unicode path"))
    );
    let first = upsert_codex_block("[other]\nvalue = true\n", &block).expect("first upsert");
    let second = upsert_codex_block(&first, &block).expect("second upsert");
    assert_eq!(first, second);
    assert!(first.contains(r#"C:\\Tools\\日本語 project\\sippion.exe"#));
    assert!(first.contains(ROOT_AUTO_TOML_ARGS));
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
    let entry = json!({"command":"/tmp/sippion","args":["mcp","--root-auto"],"cwd":"."});
    assert_eq!(
        upsert_json_server(&path, entry).unwrap(),
        FileChange::Updated
    );
    let value = read_optional_json(&path).unwrap().unwrap();
    assert!(value["mcpServers"]["other"].is_object());
    assert!(is_current_sippion_json_entry(&value["mcpServers"]["sippion"]));
    assert_eq!(remove_json_server(&path).unwrap(), FileChange::Updated);
    assert!(read_optional_json(&path).unwrap().unwrap()["mcpServers"]["sippion"].is_null());
}

#[test]
fn legacy_root_dot_entry_is_owned_but_not_current() {
    let legacy = json!({
        "command": "/tmp/sippion",
        "args": ["mcp", "--root", "."],
        "cwd": "."
    });
    assert!(is_sippion_json_entry(&legacy));
    assert!(!is_current_sippion_json_entry(&legacy));
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
fn legacy_backups_are_removed_from_all_managed_targets() {
    let home = temp_dir();
    for target in setup_target_paths(&home) {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let backup = sibling_backup_path(&target).unwrap();
        fs::write(&backup, b"stale secret-bearing config\n").unwrap();
    }

    assert!(remove_legacy_backups(&home).is_empty());
    for target in setup_target_paths(&home) {
        assert!(!sibling_backup_path(&target).unwrap().exists());
    }
}

#[test]
fn setup_snapshot_rollback_restores_targets_and_legacy_backups() {
    let home = temp_dir();
    let target = home.join(".codex").join("config.toml");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, b"original\n").unwrap();
    let backup = sibling_backup_path(&target).unwrap();
    fs::write(&backup, b"older\n").unwrap();

    #[cfg(unix)]
    {
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&backup, fs::Permissions::from_mode(0o640)).unwrap();
    }

    let snapshots = capture_setup_snapshots(&home).expect("snapshots");
    fs::write(&target, b"changed\n").unwrap();
    fs::remove_file(&backup).unwrap();
    let newly_created = home.join(".claude.json");
    fs::write(&newly_created, b"new\n").unwrap();

    #[cfg(unix)]
    fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(restore_snapshots(&snapshots).is_empty());
    assert_eq!(fs::read(&target).unwrap(), b"original\n");
    assert_eq!(fs::read(&backup).unwrap(), b"older\n");
    assert!(!newly_created.exists());

    #[cfg(unix)]
    {
        assert_eq!(fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(&backup).unwrap().permissions().mode() & 0o777, 0o640);
    }
}

#[cfg(unix)]
#[test]
fn private_configs_are_created_and_repaired_as_owner_only() {
    let root = temp_dir();
    let existing = root.join("existing.json");
    fs::write(&existing, "{}\n").unwrap();
    fs::set_permissions(&existing, fs::Permissions::from_mode(0o644)).unwrap();

    assert_eq!(
        write_text_if_changed(&existing, "{}\n", FileSecurity::PrivateConfig).unwrap(),
        FileChange::Updated
    );
    assert_eq!(
        fs::metadata(&existing).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let created = root.join("created.json");
    assert_eq!(
        write_text_if_changed(&created, "{}\n", FileSecurity::PrivateConfig).unwrap(),
        FileChange::Updated
    );
    assert_eq!(
        fs::metadata(&created).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[cfg(unix)]
#[test]
fn managed_file_symlinks_are_rejected() {
    use std::os::unix::fs::symlink;

    let root = temp_dir();
    let real = root.join("real.json");
    let managed = root.join("managed.json");
    fs::write(&real, "{}\n").unwrap();
    symlink(&real, &managed).unwrap();

    let error = write_text_if_changed(&managed, "{\"changed\":true}\n", FileSecurity::PrivateConfig)
        .expect_err("symlink rejected");
    assert!(error.contains("symlinked managed file"));
    assert_eq!(fs::read_to_string(real).unwrap(), "{}\n");
}
