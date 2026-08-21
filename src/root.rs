use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const PROJECT_MARKERS: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "Gemfile",
    "composer.json",
];

pub(crate) fn infer_project_root_from_cwd() -> Result<PathBuf, String> {
    let cwd = env::current_dir()
        .map_err(|error| format!("cannot determine current directory: {error}"))?;
    let home = home_dir().and_then(|path| fs::canonicalize(path).ok());
    infer_project_root(&cwd, home.as_deref())
}

pub(crate) fn secure_explicit_root(
    root: impl AsRef<Path>,
    allow_broad_root: bool,
) -> Result<PathBuf, String> {
    let home = home_dir().and_then(|path| fs::canonicalize(path).ok());
    secure_explicit_root_with_home(root.as_ref(), allow_broad_root, home.as_deref())
}

fn secure_explicit_root_with_home(
    root: &Path,
    allow_broad_root: bool,
    home: Option<&Path>,
) -> Result<PathBuf, String> {
    let canonical =
        fs::canonicalize(root).map_err(|error| format!("cannot resolve project root: {error}"))?;
    if !canonical.is_dir() {
        return Err("project root must be a directory".to_string());
    }
    if !allow_broad_root && is_broad_root(&canonical, home) {
        return Err(
            "refusing an over-broad project root (home directory, an ancestor of home, or filesystem root); pass --allow-broad-root only for an intentional manual scan"
                .to_string(),
        );
    }
    Ok(canonical)
}

fn infer_project_root(start: &Path, home: Option<&Path>) -> Result<PathBuf, String> {
    let start = fs::canonicalize(start)
        .map_err(|error| format!("cannot resolve current project directory: {error}"))?;
    if !start.is_dir() {
        return Err("current project path is not a directory".to_string());
    }

    let mut current = start;
    loop {
        // Automatic discovery must never trust a boundary marker placed in a directory that is
        // writable by other local users/groups. In particular, a fake /tmp/.git must not expand a
        // marker-less project into a broad shared tree. An intentional scan can still use explicit
        // --root together with the existing broad-root opt-in when appropriate.
        if is_shared_writable_directory(&current) {
            break;
        }

        // The nearest recognized project boundary wins. Continuing past a nearer manifest in
        // search of an outer .git marker lets an unrelated ancestor silently widen read scope.
        if has_git_marker(&current) || has_project_marker(&current) {
            if is_broad_root(&current, home) {
                return Err(
                    "automatic project-root discovery resolved to an over-broad directory; open the AI client inside a project or configure an explicit project root"
                        .to_string(),
                );
            }
            return Ok(current);
        }

        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current.as_path() {
            break;
        }
        current = parent.to_path_buf();
    }

    Err(
        "cannot infer a safe project root from the current directory; open the AI client inside a Git/project directory or run `sippion mcp --root <project>` manually"
            .to_string(),
    )
}

fn has_git_marker(path: &Path) -> bool {
    fs::symlink_metadata(path.join(".git")).is_ok_and(|metadata| {
        !metadata.file_type().is_symlink() && (metadata.is_dir() || metadata.is_file())
    })
}

fn has_project_marker(path: &Path) -> bool {
    PROJECT_MARKERS.iter().any(|marker| {
        fs::symlink_metadata(path.join(marker))
            .is_ok_and(|metadata| !metadata.file_type().is_symlink() && metadata.is_file())
    })
}

#[cfg(unix)]
fn is_shared_writable_directory(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path).is_ok_and(|metadata| {
        metadata.is_dir() && metadata.permissions().mode() & 0o022 != 0
    })
}

#[cfg(not(unix))]
fn is_shared_writable_directory(_path: &Path) -> bool {
    false
}

fn is_broad_root(path: &Path, home: Option<&Path>) -> bool {
    path.parent().is_none() || home.is_some_and(|home| home.starts_with(path))
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if let Some(path) = env::var_os("USERPROFILE") {
            return Some(PathBuf::from(path));
        }
        if let (Some(drive), Some(path)) = (env::var_os("HOMEDRIVE"), env::var_os("HOMEPATH")) {
            let mut home = PathBuf::from(drive);
            home.push(path);
            return Some(home);
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(path) = env::var_os("HOME") {
            return Some(PathBuf::from(path));
        }
    }
    None
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
        let path = env::temp_dir().join(format!("sippion-root-{label}-{nonce}"));
        fs::create_dir_all(&path).expect("temp root");
        path
    }

    #[test]
    fn auto_root_uses_nearest_boundary_before_outer_git_marker() {
        let root = temp_dir("nearest-boundary");
        fs::create_dir(root.join(".git")).expect("git marker");
        let nested = root.join("crates").join("child");
        fs::create_dir_all(&nested).expect("nested");
        fs::write(nested.join("Cargo.toml"), "[package]\nname='child'\n").expect("manifest");

        let resolved = infer_project_root(&nested, None).expect("root");
        assert_eq!(resolved, fs::canonicalize(&nested).unwrap());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn auto_root_uses_nearest_project_marker_without_git() {
        let root = temp_dir("marker");
        fs::write(root.join("pyproject.toml"), "[project]\nname='demo'\n").expect("manifest");
        let nested = root.join("src").join("pkg");
        fs::create_dir_all(&nested).expect("nested");

        let resolved = infer_project_root(&nested, None).expect("root");
        assert_eq!(resolved, fs::canonicalize(&root).unwrap());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn auto_root_does_not_trust_shared_writable_ancestor_marker() {
        use std::os::unix::fs::PermissionsExt;

        let shared = temp_dir("shared-ancestor");
        fs::create_dir(shared.join(".git")).expect("fake git marker");
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o777)).expect("shared mode");
        let nested = shared.join("unmarked-project").join("src");
        fs::create_dir_all(&nested).expect("nested");

        let error = infer_project_root(&nested, None).expect_err("shared marker rejected");
        assert!(error.contains("cannot infer a safe project root"));
        fs::remove_dir_all(shared).expect("cleanup");
    }

    #[test]
    fn auto_root_refuses_home_even_if_it_looks_like_a_project() {
        let home = temp_dir("home");
        fs::create_dir(home.join(".git")).expect("git marker");
        let canonical_home = fs::canonicalize(&home).unwrap();

        let error = infer_project_root(&home, Some(&canonical_home)).expect_err("home rejected");
        assert!(error.contains("over-broad"));
        fs::remove_dir_all(home).expect("cleanup");
    }

    #[test]
    fn explicit_home_requires_broad_root_opt_in() {
        let home = temp_dir("explicit-home");
        let canonical_home = fs::canonicalize(&home).unwrap();

        let error = secure_explicit_root_with_home(&home, false, Some(&canonical_home))
            .expect_err("home rejected");
        assert!(error.contains("over-broad"));
        assert_eq!(
            secure_explicit_root_with_home(&home, true, Some(&canonical_home)).unwrap(),
            canonical_home
        );
        fs::remove_dir_all(home).expect("cleanup");
    }

    #[test]
    fn explicit_parent_of_home_is_also_broad() {
        let parent = temp_dir("parent-home");
        let home = parent.join("user");
        fs::create_dir(&home).unwrap();
        let canonical_parent = fs::canonicalize(&parent).unwrap();
        let canonical_home = fs::canonicalize(&home).unwrap();

        assert!(is_broad_root(&canonical_parent, Some(&canonical_home)));
        fs::remove_dir_all(parent).expect("cleanup");
    }
}
