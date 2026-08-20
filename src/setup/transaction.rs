use super::*;

#[derive(Debug)]
struct FileSnapshot {
    path: PathBuf,
    contents: Option<Vec<u8>>,
}

#[derive(Debug)]
pub(super) struct SetupSnapshot {
    files: Vec<FileSnapshot>,
}

impl SetupSnapshot {
    pub(super) fn capture(targets: &[PathBuf]) -> Result<Self, String> {
        let mut paths = Vec::with_capacity(targets.len().saturating_mul(2));
        for target in targets {
            paths.push(target.clone());
            paths.push(backup_path(target)?);
        }
        paths.sort();
        paths.dedup();

        let mut files = Vec::with_capacity(paths.len());
        for path in paths {
            let contents = match fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() || !metadata.is_file() {
                        return Err(format!(
                            "refusing to snapshot non-regular setup path {}",
                            path.display()
                        ));
                    }
                    Some(
                        fs::read(&path)
                            .map_err(|error| format!("cannot snapshot {}: {error}", path.display()))?,
                    )
                }
                Err(error) if error.kind() == ErrorKind::NotFound => None,
                Err(error) => {
                    return Err(format!("cannot inspect {}: {error}", path.display()));
                }
            };
            files.push(FileSnapshot { path, contents });
        }
        Ok(Self { files })
    }

    pub(super) fn restore(&self) -> Result<(), String> {
        let mut failures = Vec::new();
        for snapshot in self.files.iter().rev() {
            let result = match &snapshot.contents {
                Some(contents) => replace_file_contents(&snapshot.path, contents, false),
                None => remove_if_created(&snapshot.path),
            };
            if let Err(error) = result {
                failures.push(error);
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }
}

fn remove_if_created(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(format!(
                    "refusing to remove non-regular rollback path {}",
                    path.display()
                ));
            }
            fs::remove_file(path)
                .map_err(|error| format!("cannot remove rollback-created {}: {error}", path.display()))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot inspect rollback path {}: {error}", path.display())),
    }
}
