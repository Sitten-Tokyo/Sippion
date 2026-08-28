use std::env;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(not(windows))]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(not(windows))]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(not(windows))]
static RESTORE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct Snapshot {
    path: PathBuf,
    contents: Option<Vec<u8>>,
    permissions: Option<fs::Permissions>,
}

pub(crate) fn run_setup() -> Result<(), String> {
    let home = home_dir()?;
    validate_managed_parent_boundaries(&home)?;
    crate::setup::run_setup()
}

pub(crate) fn run_doctor(json_output: bool, verbose: bool) -> Result<(), String> {
    let home = home_dir()?;
    validate_managed_parent_boundaries(&home)?;
    crate::setup::run_doctor(json_output, verbose)
}

pub(crate) fn run_uninstall() -> Result<(), String> {
    let home = home_dir()?;
    validate_managed_parent_boundaries(&home)?;
    let snapshots = capture_snapshots(&home)?;

    match crate::setup::run_uninstall() {
        Ok(()) => Ok(()),
        Err(error) => {
            let rollback_failures = restore_snapshots(&snapshots);
            if rollback_failures.is_empty() {
                Err(format!(
                    "{error}; uninstall changes were rolled back to the pre-attempt state"
                ))
            } else {
                Err(format!(
                    "{error}; uninstall rollback incomplete: {}",
                    rollback_failures.join("; ")
                ))
            }
        }
    }
}

fn managed_targets(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".codex").join("config.toml"),
        home.join(".claude.json"),
        home.join(".gemini").join("config").join("mcp_config.json"),
        home.join(".codex").join("AGENTS.md"),
        home.join(".claude").join("CLAUDE.md"),
        home.join(".gemini").join("GEMINI.md"),
    ]
}

fn snapshot_targets(home: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for target in managed_targets(home) {
        paths.push(target.clone());
        paths.push(backup_path(&target)?);
    }
    Ok(paths)
}

fn backup_path(path: &Path) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| format!("cannot derive backup path for {}", path.display()))?;
    Ok(path.with_file_name(format!("{name}.sippion-backup")))
}

fn capture_snapshots(home: &Path) -> Result<Vec<Snapshot>, String> {
    snapshot_targets(home)?
        .into_iter()
        .map(snapshot_path)
        .collect()
}

fn snapshot_path(path: PathBuf) -> Result<Snapshot, String> {
    reject_symlink_file(&path)?;
    match fs::read(&path) {
        Ok(contents) => {
            let permissions = fs::metadata(&path)
                .map_err(|error| {
                    format!("cannot stat {} before uninstall: {error}", path.display())
                })?
                .permissions();
            Ok(Snapshot {
                path,
                contents: Some(contents),
                permissions: Some(permissions),
            })
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Snapshot {
            path,
            contents: None,
            permissions: None,
        }),
        Err(error) => Err(format!(
            "cannot snapshot {} before uninstall: {error}",
            path.display()
        )),
    }
}

fn restore_snapshots(snapshots: &[Snapshot]) -> Vec<String> {
    let mut failures = Vec::new();
    for snapshot in snapshots.iter().rev() {
        let result = match &snapshot.contents {
            Some(contents) => {
                restore_bytes(&snapshot.path, contents, snapshot.permissions.as_ref())
            }
            None => remove_if_regular(&snapshot.path),
        };
        if let Err(error) = result {
            failures.push(error);
        }
    }
    failures
}

fn remove_if_regular(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(format!(
            "refusing to remove symlink that appeared during rollback: {}",
            path.display()
        )),
        Ok(metadata) if metadata.is_file() => fs::remove_file(path)
            .map_err(|error| format!("cannot remove {} during rollback: {error}", path.display())),
        Ok(_) => Err(format!(
            "cannot remove non-file managed path during rollback: {}",
            path.display()
        )),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "cannot inspect {} during rollback: {error}",
            path.display()
        )),
    }
}

#[cfg(windows)]
fn restore_bytes(
    path: &Path,
    contents: &[u8],
    permissions: Option<&fs::Permissions>,
) -> Result<(), String> {
    reject_symlink_file(path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "cannot create {} during rollback: {error}",
                parent.display()
            )
        })?;
    }

    let existed = fs::symlink_metadata(path).is_ok();
    let mut options = OpenOptions::new();
    options.write(true).truncate(true);
    if existed {
        options.create(false);
    } else {
        options.create_new(true);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("cannot open {} during rollback: {error}", path.display()))?;
    file.write_all(contents)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("cannot restore {}: {error}", path.display()))?;
    if let Some(permissions) = permissions {
        fs::set_permissions(path, permissions.clone()).map_err(|error| {
            format!("cannot restore permissions on {}: {error}", path.display())
        })?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn restore_bytes(
    path: &Path,
    contents: &[u8],
    permissions: Option<&fs::Permissions>,
) -> Result<(), String> {
    reject_symlink_file(path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "cannot create {} during rollback: {error}",
                parent.display()
            )
        })?;
    }

    let temporary = write_temporary(path, contents)?;
    if let Some(permissions) = permissions {
        if let Err(error) = fs::set_permissions(&temporary, permissions.clone()) {
            let _ = fs::remove_file(&temporary);
            return Err(format!(
                "cannot restore permissions on {}: {error}",
                temporary.display()
            ));
        }
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("cannot restore {}: {error}", path.display()));
    }
    Ok(())
}

#[cfg(not(windows))]
fn write_temporary(path: &Path, contents: &[u8]) -> Result<PathBuf, String> {
    for _ in 0..8 {
        let temporary = temporary_path(path);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(&temporary) {
            Ok(mut file) => {
                let result = file
                    .write_all(contents)
                    .and_then(|_| file.sync_all())
                    .map_err(|error| format!("cannot write {}: {error}", temporary.display()));
                if let Err(error) = result {
                    let _ = fs::remove_file(&temporary);
                    return Err(error);
                }
                return Ok(temporary);
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!("cannot create {}: {error}", temporary.display()));
            }
        }
    }
    Err(format!(
        "cannot allocate rollback temporary file next to {}",
        path.display()
    ))
}

#[cfg(not(windows))]
fn temporary_path(path: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = RESTORE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("managed");
    path.with_file_name(format!(
        ".{name}.sippion-rollback-{}-{nanos}-{counter}",
        std::process::id()
    ))
}

fn validate_managed_parent_boundaries(home: &Path) -> Result<(), String> {
    let parents = [
        home.join(".codex"),
        home.join(".claude"),
        home.join(".gemini"),
        home.join(".gemini").join("config"),
    ];
    for path in parents {
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "refusing managed configuration under symlinked parent directory {}",
                    path.display()
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(format!(
                    "managed parent path is not a directory: {}",
                    path.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "cannot inspect managed parent {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

fn reject_symlink_file(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(format!(
            "refusing symlinked managed file {}",
            path.display()
        )),
        Ok(metadata) if !metadata.is_file() => Err(format!(
            "managed path is not a regular file: {}",
            path.display()
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot inspect {}: {error}", path.display())),
    }
}

fn home_dir() -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        if let Some(path) = env::var_os("USERPROFILE") {
            return Ok(PathBuf::from(path));
        }
        if let (Some(drive), Some(path)) = (env::var_os("HOMEDRIVE"), env::var_os("HOMEPATH")) {
            let mut home = PathBuf::from(drive);
            home.push(path);
            return Ok(home);
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(path) = env::var_os("HOME") {
            return Ok(PathBuf::from(path));
        }
    }
    Err("cannot determine user home directory".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = env::temp_dir().join(format!("sippion-managed-{label}-{nonce}"));
        fs::create_dir_all(&path).expect("temp root");
        path
    }

    #[test]
    fn snapshot_restore_recovers_modified_and_created_files() {
        let home = temp_dir("rollback");
        fs::create_dir_all(home.join(".codex")).unwrap();
        let existing = home.join(".codex").join("config.toml");
        fs::write(&existing, b"before\n").unwrap();
        let created = home.join(".claude.json");

        let snapshots = capture_snapshots(&home).expect("snapshots");
        fs::write(&existing, b"after\n").unwrap();
        fs::write(&created, b"new\n").unwrap();

        assert!(restore_snapshots(&snapshots).is_empty());
        assert_eq!(fs::read(&existing).unwrap(), b"before\n");
        assert!(!created.exists());
        fs::remove_dir_all(home).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_managed_parent_is_rejected() {
        use std::os::unix::fs::symlink;

        let home = temp_dir("parent-symlink");
        let real = home.join("real-codex");
        fs::create_dir_all(&real).unwrap();
        symlink(&real, home.join(".codex")).unwrap();

        let error = validate_managed_parent_boundaries(&home).expect_err("symlink rejected");
        assert!(error.contains("symlinked parent"));
        fs::remove_dir_all(home).unwrap();
    }
}
