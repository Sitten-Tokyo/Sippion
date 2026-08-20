use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_home() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let home = std::env::temp_dir().join(format!("sippion-doctor-empty-home-{nonce}"));
    std::fs::create_dir_all(&home).expect("create isolated home");
    home
}

#[test]
fn doctor_returns_nonzero_when_expected_configuration_is_missing() {
    let home = temp_home();
    let mut command = Command::new(env!("CARGO_BIN_EXE_sippion"));
    command.arg("doctor");
    #[cfg(windows)]
    {
        command.env("USERPROFILE", &home);
        command.env("LOCALAPPDATA", home.join("AppData").join("Local"));
    }
    #[cfg(not(windows))]
    command.env("HOME", &home);

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
