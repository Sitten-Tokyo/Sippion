use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_home() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let home = std::env::temp_dir().join(format!("sippion-cli-home-{nonce}"));
    std::fs::create_dir_all(&home).expect("create isolated home");
    home
}

fn configure_home(command: &mut Command, home: &std::path::Path) {
    #[cfg(windows)]
    {
        command.env("USERPROFILE", home);
        command.env("LOCALAPPDATA", home.join("AppData").join("Local"));
    }
    #[cfg(not(windows))]
    command.env("HOME", home);
}

#[test]
fn doctor_returns_nonzero_when_expected_configuration_is_missing() {
    let home = temp_home();
    let mut command = Command::new(env!("CARGO_BIN_EXE_sippion"));
    command.arg("doctor");
    configure_home(&mut command, &home);

    let output = command.output().expect("run doctor");
    assert!(
        !output.status.success(),
        "doctor must fail for an empty home"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("MISSING"));
    assert!(stderr.contains("doctor found"));

    std::fs::remove_dir_all(home).expect("cleanup isolated home");
}

#[test]
fn root_auto_fails_closed_when_no_project_can_be_inferred() {
    let home = temp_home();
    let isolated = home.join("plain-directory");
    std::fs::create_dir(&isolated).expect("plain directory");

    let mut command = Command::new(env!("CARGO_BIN_EXE_sippion"));
    command.args(["mcp", "--root-auto"]).current_dir(&isolated);
    configure_home(&mut command, &home);

    let output = command.output().expect("run root-auto");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot infer a safe project root"));

    std::fs::remove_dir_all(home).expect("cleanup isolated home");
}

#[test]
fn root_auto_accepts_a_bounded_project_marker() {
    let home = temp_home();
    let project = home.join("project");
    let nested = project.join("src");
    std::fs::create_dir_all(&nested).expect("project directory");
    std::fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"root-auto-smoke\"\nversion = \"0.0.0\"\n",
    )
    .expect("project marker");

    let mut command = Command::new(env!("CARGO_BIN_EXE_sippion"));
    command.args(["mcp", "--root-auto"]).current_dir(&nested);
    configure_home(&mut command, &home);

    let output = command.output().expect("run root-auto");
    assert!(
        output.status.success(),
        "root-auto should bind to the project and exit cleanly on stdin EOF: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    std::fs::remove_dir_all(home).expect("cleanup isolated home");
}

#[test]
fn explicit_home_root_requires_broad_root_opt_in() {
    let home = temp_home();

    let mut command = Command::new(env!("CARGO_BIN_EXE_sippion"));
    command.arg("mcp").arg("--root").arg(&home);
    configure_home(&mut command, &home);

    let output = command.output().expect("run explicit broad root");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("refusing an over-broad project root"));

    std::fs::remove_dir_all(home).expect("cleanup isolated home");
}
