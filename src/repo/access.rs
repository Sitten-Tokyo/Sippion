use super::*;

impl RepositoryAccess {
    #[cfg(test)]
    pub fn open(root_path: impl AsRef<Path>) -> Result<Self, RepositoryAccessError> {
        Self::open_with_scan_budget(root_path, MAX_SCAN_BYTES)
    }

    pub fn open_with_scan_budget(
        root_path: impl AsRef<Path>,
        scan_budget_bytes: usize,
    ) -> Result<Self, RepositoryAccessError> {
        let canonical_root = std::fs::canonicalize(root_path.as_ref()).map_err(map_io)?;
        let metadata = std::fs::metadata(&canonical_root).map_err(map_io)?;
        if !metadata.is_dir() {
            return Err(RepositoryAccessError::NotRegularFile);
        }
        let root_dir =
            Dir::open_ambient_dir(&canonical_root, ambient_authority()).map_err(map_io)?;
        let root_identity = root_identity_from_dir(&root_dir)?;
        // Close the canonicalize/open race: the ambient path must still resolve to the exact
        // directory handle that will be used for all capability-scoped source reads.
        let current_root =
            Dir::open_ambient_dir(&canonical_root, ambient_authority()).map_err(map_io)?;
        if root_identity_from_dir(&current_root)? != root_identity {
            return Err(RepositoryAccessError::ConcurrentModification);
        }
        Ok(Self {
            root_path: canonical_root,
            root_dir,
            root_identity,
            max_scan_budget_bytes: scan_budget_bytes
                .clamp(MIN_CONFIGURED_SCAN_BYTES, MAX_CONFIGURED_SCAN_BYTES),
            ram_index: Mutex::new(RamIndex::default()),
            #[cfg(windows)]
            windows_search_serial: Mutex::new(()),
            #[cfg(windows)]
            windows_map_serial: Mutex::new(()),
            index_inflight: Mutex::new(HashSet::new()),
            index_ready: Condvar::new(),
            session_memory: Mutex::new(VecDeque::new()),
            analysis_cache: Mutex::new(AnalysisCacheState::default()),
            analysis_ready: Condvar::new(),
            graph_cache: Mutex::new(GraphCacheState::default()),
        })
    }

    pub(super) fn ensure_root_path_identity(&self) -> Result<(), RepositoryAccessError> {
        let current = Dir::open_ambient_dir(&self.root_path, ambient_authority())
            .map_err(|_| RepositoryAccessError::ConcurrentModification)?;
        let current_identity = root_identity_from_dir(&current)
            .map_err(|_| RepositoryAccessError::ConcurrentModification)?;
        if current_identity != self.root_identity {
            return Err(RepositoryAccessError::ConcurrentModification);
        }
        Ok(())
    }

    pub(super) fn verified_metadata_stamp(
        &self,
        relative_path: &str,
    ) -> Result<SourceStamp, RepositoryAccessError> {
        let normalized = normalize_relative(Path::new(relative_path))?;
        let relative = Path::new(&normalized);
        if is_denied(relative) {
            return Err(RepositoryAccessError::DeniedPath);
        }
        if is_pruned(relative) {
            return Err(RepositoryAccessError::PrunedPath);
        }
        let file = self.open_file_nofollow(relative)?;
        let metadata = file.metadata().map_err(map_io)?;
        if !metadata.is_file() {
            return Err(RepositoryAccessError::NotRegularFile);
        }
        if file_has_multiple_hard_links(&file, &metadata)? {
            return Err(RepositoryAccessError::HardLinkedFile);
        }
        if metadata.len() > MAX_SOURCE_BYTES as u64 {
            return Err(RepositoryAccessError::TooLarge);
        }
        cap_source_stamp(&file, &metadata)
    }

    pub(super) fn open_file_nofollow(
        &self,
        relative: &Path,
    ) -> Result<cap_std::fs::File, RepositoryAccessError> {
        let components = relative
            .components()
            .map(|component| match component {
                Component::Normal(part) => Ok(part.to_owned()),
                _ => Err(RepositoryAccessError::InvalidRelativePath),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let Some((file_name, parents)) = components.split_last() else {
            return Err(RepositoryAccessError::InvalidRelativePath);
        };

        let mut directory = self.root_dir.try_clone().map_err(map_io)?;
        for parent in parents {
            directory = directory
                .open_dir_nofollow(Path::new(parent))
                .map_err(map_io)?;
        }

        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        // A path discovered as a regular file can be replaced with a FIFO or another special file
        // before this open. On Unix, a blocking read-only open of a FIFO can wait indefinitely for
        // a writer, so make the open nonblocking and reject non-regular files immediately afterward
        // in read_source(). O_NONBLOCK has no meaningful effect on ordinary regular-file reads.
        #[cfg(unix)]
        options.nonblock(true);
        directory
            .open_with(Path::new(file_name), &options)
            .map_err(map_io)
    }

    pub(super) fn discover_files(
        &self,
        cancellation: Option<&AtomicBool>,
        started: &Instant,
        policy_skips: &HashMap<String, SourceStamp>,
    ) -> Result<DiscoveryOutcome, RepositoryAccessError> {
        // `ignore::WalkBuilder` is path-based, while source reads are capability-handle-based. Refuse
        // to walk if the configured path was renamed/replaced after startup, otherwise discovery and
        // verification could observe different directory trees.
        self.ensure_root_path_identity()?;
        let mut builder = WalkBuilder::new(&self.root_path);
        builder
            .hidden(false)
            .parents(false)
            .ignore(true)
            .follow_links(false)
            .git_ignore(true)
            .git_global(false)
            .git_exclude(true)
            // Apply repository-local ignore rules even when a trusted project root is a
            // standalone directory rather than a checked-out Git worktree.
            .require_git(false);

        let root_path = self.root_path.clone();
        // filter_entry() removes denied/pruned/binary paths (and whole subtrees) before the main
        // discovery loop can observe them. Keep a conservative count so an otherwise empty search
        // can never be reported as an absolute NO_MATCH when policy hid repository content. A
        // pruned directory counts as one exclusion sentinel even though its subtree size is unknown.
        //
        // The ignore walker can also hide entries before the discovery loop sees them. Treat every
        // visible directory containing a .gitignore/.ignore control file as one conservative policy
        // exclusion sentinel. This deliberately does not inspect the ignored subtree: privacy and
        // performance semantics stay unchanged, while repository-wide absence claims stay sound.
        let has_ignore_control = |directory: &Path| {
            [".gitignore", ".ignore"]
                .into_iter()
                .any(|name| std::fs::symlink_metadata(directory.join(name)).is_ok())
        };
        let root_ignore_sentinel = if has_ignore_control(&root_path) { 1 } else { 0 };
        let prefiltered_policy_exclusions = Arc::new(AtomicUsize::new(root_ignore_sentinel));
        let filter_exclusions = Arc::clone(&prefiltered_policy_exclusions);
        builder.filter_entry(move |entry| {
            if entry.path() == root_path {
                return true;
            }
            if entry.file_type().is_some_and(|kind| kind.is_dir())
                && has_ignore_control(entry.path())
            {
                filter_exclusions.fetch_add(1, AtomicOrdering::Relaxed);
            }
            let Ok(relative) = entry.path().strip_prefix(&root_path) else {
                return false;
            };
            let excluded = entry.file_type().is_some_and(|kind| kind.is_symlink())
                || is_pruned(relative)
                || is_denied(relative)
                || is_obvious_binary(relative);
            if excluded {
                filter_exclusions.fetch_add(1, AtomicOrdering::Relaxed);
            }
            !excluded
        });

        let mut files = Vec::new();
        let mut policy_excluded_files = 0usize;
        let mut truncated = false;
        let mut visited_entries = 0usize;
        let mut retained_path_bytes = 0usize;
        for item in builder.build() {
            if is_cancelled(cancellation) {
                return Err(RepositoryAccessError::Cancelled);
            }
            if search_timed_out(started) {
                truncated = true;
                break;
            }
            visited_entries = visited_entries.saturating_add(1);
            if visited_entries > MAX_DISCOVERED_ENTRIES {
                truncated = true;
                break;
            }
            // Fail closed for disclosure, but report that discovery was incomplete instead of
            // presenting a false complete NO_MATCH when an unreadable/transient entry was skipped.
            let entry = match item {
                Ok(entry) => entry,
                Err(_) => {
                    truncated = true;
                    continue;
                }
            };
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let relative = match entry.path().strip_prefix(&self.root_path) {
                Ok(relative) => relative,
                Err(_) => {
                    truncated = true;
                    continue;
                }
            };
            if is_denied(relative) || is_pruned(relative) || is_obvious_binary(relative) {
                continue;
            }
            let normalized = match normalize_relative(relative) {
                Ok(normalized) => normalized,
                Err(_) => {
                    truncated = true;
                    continue;
                }
            };
            let metadata = entry.metadata().ok();
            if metadata.as_ref().is_some_and(policy_excluded_by_metadata) {
                policy_excluded_files = policy_excluded_files.saturating_add(1);
                continue;
            }
            // On Windows, the ignore-walker's ambient metadata and cap-std's verified metadata
            // can expose different timestamp precision. Use the same capability-checked stamp
            // used by read_source so unchanged files remain reusable across searches.
            #[cfg(windows)]
            let stamp = self.verified_metadata_stamp(&normalized).ok();
            #[cfg(not(windows))]
            let stamp = metadata.as_ref().map(source_stamp);
            if stamp
                .as_ref()
                .is_some_and(|current| policy_skips.get(&normalized) == Some(current))
            {
                policy_excluded_files = policy_excluded_files.saturating_add(1);
                continue;
            }
            if retained_path_bytes.saturating_add(normalized.len()) > MAX_DISCOVERED_PATH_BYTES {
                truncated = true;
                break;
            }
            retained_path_bytes = retained_path_bytes.saturating_add(normalized.len());
            files.push(DiscoveredFile {
                path: normalized,
                stamp,
            });
            if files.len() >= MAX_DISCOVERED_FILES {
                truncated = true;
                break;
            }
        }
        policy_excluded_files = policy_excluded_files
            .saturating_add(prefiltered_policy_exclusions.load(AtomicOrdering::Relaxed));
        // Catch a rename/replacement that happened while the ambient walker was active.
        self.ensure_root_path_identity()?;
        Ok(DiscoveryOutcome {
            files,
            policy_excluded_files,
            truncated,
        })
    }

    pub(super) fn read_source(
        &self,
        relative_path: &str,
    ) -> Result<VerifiedSource, (RepositoryAccessError, usize)> {
        let no_bytes = |error| (error, 0usize);
        let normalized = normalize_relative(Path::new(relative_path)).map_err(no_bytes)?;
        let relative = Path::new(&normalized);
        if is_denied(relative) {
            return Err((RepositoryAccessError::DeniedPath, 0));
        }
        if is_pruned(relative) {
            return Err((RepositoryAccessError::PrunedPath, 0));
        }

        // Never canonicalize and then open by path: that creates a TOCTOU window where an allowed
        // entry can be swapped for a symlink after policy validation. Walk parent directories through
        // already-open capability handles and refuse symlinks on every component, including the file.
        let mut file = self.open_file_nofollow(relative).map_err(no_bytes)?;
        let before = file.metadata().map_err(|error| (map_io(error), 0))?;
        if !before.is_file() {
            return Err((RepositoryAccessError::NotRegularFile, 0));
        }
        if file_has_multiple_hard_links(&file, &before).map_err(no_bytes)? {
            return Err((RepositoryAccessError::HardLinkedFile, 0));
        }
        if before.len() > MAX_SOURCE_BYTES as u64 {
            return Err((RepositoryAccessError::TooLarge, 0));
        }
        let before_stamp = cap_source_stamp(&file, &before).map_err(|error| (error, 0))?;

        // Never trust metadata length as a memory bound: a concurrently growing regular file could
        // otherwise make an unbounded read allocate beyond MAX_SOURCE_BYTES.
        let mut bytes = Vec::with_capacity(before.len() as usize);
        {
            let mut limited = (&mut file).take((MAX_SOURCE_BYTES + 1) as u64);
            if let Err(error) = limited.read_to_end(&mut bytes) {
                let consumed = bytes.len();
                return Err((map_io(error), consumed));
            }
        }
        let source_bytes = bytes.len();
        if source_bytes > MAX_SOURCE_BYTES {
            return Err((RepositoryAccessError::TooLarge, source_bytes));
        }

        let after = file
            .metadata()
            .map_err(|error| (map_io(error), source_bytes))?;
        let after_stamp = cap_source_stamp(&file, &after).map_err(|error| (error, source_bytes))?;
        if file_has_multiple_hard_links(&file, &after).map_err(|error| (error, source_bytes))?
            || before_stamp != after_stamp
            || source_bytes as u64 != after.len()
        {
            return Err((RepositoryAccessError::ConcurrentModification, source_bytes));
        }

        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => return Err((RepositoryAccessError::NonUtf8Source, source_bytes)),
        };
        Ok(VerifiedSource {
            text,
            source_bytes,
            stamp: after_stamp,
        })
    }

    #[cfg(any(windows, test))]
    pub(super) fn reset_ram_index(&self) -> Result<(), RepositoryAccessError> {
        let mut index = self
            .ram_index
            .lock()
            .map_err(|_| RepositoryAccessError::Io)?;
        index.files.clear();
        index.total_entries = 0;
        index.saturated = false;
        Ok(())
    }

    #[cfg(windows)]
    pub(super) fn reset_structural_caches(&self) -> Result<(), RepositoryAccessError> {
        {
            let mut analysis = self
                .analysis_cache
                .lock()
                .map_err(|_| RepositoryAccessError::Io)?;
            analysis.entries.clear();
            analysis.tick = 0;
        }
        {
            let mut graph = self
                .graph_cache
                .lock()
                .map_err(|_| RepositoryAccessError::Io)?;
            graph.entries.clear();
            graph.tick = 0;
        }
        Ok(())
    }
}
