use super::*;

pub(super) fn root_identity_from_dir(
    directory: &Dir,
) -> Result<RootIdentity, RepositoryAccessError> {
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt;
        let metadata = directory.dir_metadata().map_err(map_io)?;
        Ok(RootIdentity {
            dev: metadata.dev(),
            ino: metadata.ino(),
        })
    }
    #[cfg(windows)]
    {
        let std_directory = directory.try_clone().map_err(map_io)?.into_std_file();
        let information = winapi_util::file::information(&std_directory).map_err(map_io)?;
        Ok(RootIdentity {
            volume_serial_number: information.volume_serial_number(),
            file_index: information.file_index(),
        })
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        let metadata = directory.dir_metadata().map_err(map_io)?;
        let created_nanos = metadata
            .created()
            .ok()
            .and_then(|created| {
                created
                    .into_std()
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
            })
            .map(|duration| duration.as_nanos());
        Ok(RootIdentity { created_nanos })
    }
}

#[cfg(any(not(windows), test))]
pub(super) fn source_stamp(metadata: &std::fs::Metadata) -> SourceStamp {
    #[cfg(not(windows))]
    let modified_nanos = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        SourceStamp {
            len: metadata.len(),
            modified_nanos,
            dev: metadata.dev(),
            ino: metadata.ino(),
            ctime: metadata.ctime(),
            ctime_nsec: metadata.ctime_nsec(),
            nlink: metadata.nlink(),
        }
    }
    #[cfg(windows)]
    {
        SourceStamp {
            len: metadata.len(),
            modified_nanos: metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos()),
            volume_serial_number: None,
            file_index: None,
            last_write_time: None,
        }
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        SourceStamp {
            len: metadata.len(),
            modified_nanos,
        }
    }
}

pub(super) fn cap_source_stamp(
    _file: &cap_std::fs::File,
    metadata: &cap_std::fs::Metadata,
) -> Result<SourceStamp, RepositoryAccessError> {
    #[cfg(not(windows))]
    let modified_nanos = metadata
        .modified()
        .ok()
        .and_then(|modified| {
            modified
                .into_std()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
        })
        .map(|duration| duration.as_nanos());
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt;
        Ok(SourceStamp {
            len: metadata.len(),
            modified_nanos,
            dev: metadata.dev(),
            ino: metadata.ino(),
            ctime: metadata.ctime(),
            ctime_nsec: metadata.ctime_nsec(),
            nlink: metadata.nlink(),
        })
    }
    #[cfg(windows)]
    {
        let std_file = _file.try_clone().map_err(map_io)?.into_std();
        let information = winapi_util::file::information(&std_file).map_err(map_io)?;
        Ok(SourceStamp {
            len: metadata.len(),
            modified_nanos: None,
            volume_serial_number: Some(information.volume_serial_number()),
            file_index: Some(information.file_index()),
            last_write_time: information.last_write_time(),
        })
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        Ok(SourceStamp {
            len: metadata.len(),
            modified_nanos,
        })
    }
}

#[cfg(unix)]
pub(super) fn metadata_has_multiple_hard_links(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.nlink() > 1
}

#[cfg(not(unix))]
pub(super) fn metadata_has_multiple_hard_links(_metadata: &std::fs::Metadata) -> bool {
    // Directory-walk metadata does not expose a portable hard-link count. Windows performs the
    // authoritative check on the already-open file handle before and after reading instead.
    false
}

#[cfg(unix)]
pub(super) fn file_has_multiple_hard_links(
    _file: &cap_std::fs::File,
    metadata: &cap_std::fs::Metadata,
) -> Result<bool, RepositoryAccessError> {
    use cap_std::fs::MetadataExt;
    Ok(metadata.nlink() > 1)
}

#[cfg(windows)]
pub(super) fn file_has_multiple_hard_links(
    file: &cap_std::fs::File,
    _metadata: &cap_std::fs::Metadata,
) -> Result<bool, RepositoryAccessError> {
    // Rust's stable std API still does not expose nNumberOfLinks. Query the exact capability-opened
    // handle through winapi-util's safe GetFileInformationByHandle wrapper, avoiding a path reopen
    // and the TOCTOU window that would create.
    let std_file = file.try_clone().map_err(map_io)?.into_std();
    let information = winapi_util::file::information(&std_file).map_err(map_io)?;
    Ok(information.number_of_links() > 1)
}

#[cfg(all(not(unix), not(windows)))]
pub(super) fn file_has_multiple_hard_links(
    _file: &cap_std::fs::File,
    _metadata: &cap_std::fs::Metadata,
) -> Result<bool, RepositoryAccessError> {
    // No portable stable link-count API exists on the remaining targets. Keep the trusted-root
    // requirement there rather than making every regular file unreadable.
    Ok(false)
}

pub(super) fn policy_excluded_by_metadata(metadata: &std::fs::Metadata) -> bool {
    metadata.len() > MAX_SOURCE_BYTES as u64 || metadata_has_multiple_hard_links(metadata)
}

pub(super) fn normalize_relative(path: &Path) -> Result<String, RepositoryAccessError> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(RepositoryAccessError::InvalidRelativePath);
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_str().ok_or(RepositoryAccessError::NonUtf8Path)?;
                if part.chars().any(char::is_control) {
                    return Err(RepositoryAccessError::InvalidRelativePath);
                }
                parts.push(part.to_string());
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(RepositoryAccessError::InvalidRelativePath);
            }
        }
    }
    if parts.is_empty() {
        return Err(RepositoryAccessError::InvalidRelativePath);
    }
    Ok(parts.join("/"))
}

pub(super) fn is_obvious_binary(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            let extension = extension.to_ascii_lowercase();
            OBVIOUS_BINARY_EXTENSIONS.contains(&extension.as_str())
        })
}

pub(super) fn is_pruned(path: &Path) -> bool {
    // Non-UTF-8 paths are not lossy-normalized here. Discovery will reach the file and
    // normalize_relative() will mark the scan incomplete instead of collapsing distinct names.
    // Policy matching is ASCII case-insensitive because Windows and common macOS filesystems can
    // resolve differently-cased spellings to the same entry.
    let Some(parts) = path_parts(path) else {
        return false;
    };
    let parts = parts
        .into_iter()
        .map(|part| part.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if parts.iter().any(|part| {
        BUILTIN_PRUNED_DIRS.contains(&part.as_str()) || part.starts_with("cmake-build-")
    }) {
        return true;
    }
    parts
        .last()
        .is_some_and(|name| BUILTIN_PRUNED_FILES.contains(&name.as_str()))
}

pub(super) fn is_denied(path: &Path) -> bool {
    // Do not convert invalid OS strings with U+FFFD: that can alias distinct filesystem paths.
    // Let strict normalization reject them later so completeness is reported accurately. For
    // valid Unicode names, ASCII-fold policy tokens so case-insensitive filesystems cannot bypass
    // credential/config denials with spellings such as `.SSH`, `.ENV`, or `ID_RSA`.
    let Some(parts) = path_parts(path) else {
        return false;
    };
    let parts = parts
        .into_iter()
        .map(|part| part.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if parts.iter().any(|part| {
        matches!(
            part.as_str(),
            ".git"
                | ".hg"
                | ".svn"
                | ".codex"
                | ".claude"
                | ".agents"
                | ".agent"
                | ".gemini"
                | ".ssh"
                | ".aws"
                | ".azure"
                | ".kube"
                | ".docker"
                | ".gnupg"
                | ".direnv"
                | ".password-store"
                | ".sippion"
        )
    }) {
        return true;
    }
    if parts
        .windows(2)
        .any(|pair| pair[0] == ".config" && pair[1] == "gcloud")
    {
        return true;
    }
    if parts.windows(2).any(|pair| {
        pair[0] == ".cargo" && matches!(pair[1].as_str(), "credentials" | "credentials.toml")
    }) {
        return true;
    }

    let Some(name) = parts.last().map(String::as_str) else {
        return true;
    };
    if matches!(
        name,
        ".npmrc"
            | ".pypirc"
            | ".netrc"
            | ".git-credentials"
            | "id_rsa"
            | "id_ed25519"
            | "id_dsa"
            | "id_ecdsa"
            | ".envrc"
            | ".terraformrc"
            | "terraform.rc"
            | ".vault-token"
            | "auth.json"
            | ".secrets"
            | "secrets.json"
            | "secrets.yaml"
            | "secrets.yml"
            | "secrets.toml"
            | "credentials"
            | "credentials.toml"
            | "application_default_credentials.json"
            | "service-account.json"
            | "service_account.json"
            | "kubeconfig"
    ) || name.ends_with(".pem")
        || name.ends_with(".key")
        || name.ends_with(".p12")
        || name.ends_with(".pfx")
        || name.ends_with(".jks")
        || name.ends_with(".keystore")
        || name.ends_with(".tfstate")
        || name.contains(".tfstate.")
    {
        return true;
    }
    if name == ".env" || name.starts_with(".env.") {
        return !matches!(
            name,
            ".env.example" | ".env.sample" | ".env.template" | ".env.defaults"
        );
    }
    false
}

pub(super) fn map_io(error: std::io::Error) -> RepositoryAccessError {
    match error.kind() {
        std::io::ErrorKind::NotFound => RepositoryAccessError::NotFound,
        _ => RepositoryAccessError::Io,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_path_policy_is_ascii_case_insensitive() {
        assert!(is_denied(Path::new(".SSH/ID_RSA")));
        assert!(is_denied(Path::new("nested/.ENV.Production")));
        assert!(is_denied(Path::new("Secrets.JSON")));
        assert!(is_denied(Path::new(".GIT/config")));
        assert!(is_denied(Path::new(".terraformrc")));
        assert!(is_denied(Path::new("terraform.rc")));
        assert!(is_denied(Path::new(".vault-token")));
        assert!(is_denied(Path::new("auth.json")));
        assert!(is_denied(Path::new(".cargo/credentials.toml")));
        assert!(!is_denied(Path::new(".cargo/config.toml")));
        assert!(!is_denied(Path::new(".ENV.Example")));
    }

    #[test]
    fn prune_policy_is_ascii_case_insensitive() {
        assert!(is_pruned(Path::new("TARGET/debug/sippion")));
        assert!(is_pruned(Path::new("Cargo.LOCK")));
        assert!(is_pruned(Path::new("CMAKE-BUILD-Debug/cache.txt")));
    }
}
