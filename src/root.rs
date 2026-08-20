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
    let cwd = env::current_dir().map_err(|error| format!("cannot determine current directory: {error}"))?;
    let home = home_dir().and_then(|path| fs::canonicalize(path).ok());
    infer_project_root(&cwd, home.as_deref())
}

pub(crate) fn secure_explicit_root(
    root: impl AsRef<Path>,
    allow_broad_root: bool,
) -> Result<PathBuf, String> {
    let canonical = fs::canonicalize(root.as_ref())
        .map_err(|error| format!("cannot resolve project root: {error}"))?;
    if !canonical.is_dir() {
        return Err("project root must be a directory".to_string());
    }
    if !allow_broad_root {
        let home = home_dir().and_then(|path| fs::canonicalize(path).ok());
        if is_broad_root(&canonical, home.as_deref()) {
            return Err(
                "refusing an over-broad project root (home directory or filesystem root); pass --allow-broad-root only for an intentional manual scan"
                    .to_string(),
            );
        }
    }
    Ok(canonical)
}

fn infer_project_root(start: &Path, home: Option<&Path>) -> Result<PathBuf, String> {
    let start = fs::canonicalize(start)
        .map_err(|error| format!("cannot resolve current project directory: {error}"))?;
    if !start.is_dir() {
        return Err("current project path is not a directory".to_string());
    }

    let mut current = start.clone();
    let mut marker_fallback = None;
    loop {
        if has_git_marker(&current) {
            if is_broad_root(&current, home) {
                return Err(
                    "automatic project-root discovery resolved to an over-broad directory; open the AI client inside a project or configure an explicit project root"
                        .to_string(),
                );
            }
            return Ok(current);
        }

        if marker_fallback.is_none() && has_project_marker(&current) {
            marker_fallback = Some(current.clone());
        }

        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent.to_path_buf();
    }

    if let Some(root) = marker_fallback {
        if is_broad_root(&root, home) {
            return Err(
                "automatic project-root discovery resolved to an over-broad directory; open the AI client inside a project or configure an explicit project root"
                    .to_string(),
            );
        }
        return Ok(root);
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

fn is_broad_root(path: &Path, home: Option<&Path>) -> bool {
    path.parent().is_none() || home.is_some_and(|home| path == home)
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
    fn auto_root_prefers_git_root_over_nested_manifest() {
        let root = temp_dir("git");
        fs::create_dir(root.join(".git")).expect("git marker");
        let nested = root.join("crates").join("child");
        fs::create_dir_all(&nested).expect("nested");
        fs::write(nested.join("Cargo.toml"), "[package]\nname='child'\n").expect("manifest");

        let resolved = infer_project_root(&nested, None).expect("root");
        assert_eq!(resolved, fs::canonicalize(&root).unwrap());
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
        assert!(is_broad_root(&canonical_home, Some(&canonical_home)));
        assert!(!is_broad_root(&canonical_home.join("project"), Some(&canonical_home)));
        fs::remove_dir_all(home).expect("cleanup");
    }
}
