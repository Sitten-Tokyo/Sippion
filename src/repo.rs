use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use aho_corasick::AhoCorasick;
#[cfg(unix)]
use cap_fs_ext::OpenOptionsSyncExt;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
use ignore::WalkBuilder;

use crate::core::{CoordinationContext, MAX_QUERY_TERMS, NormalizedQuery};
use crate::hybrid::{
    bm25_score, extract_symbols, structural_line_bonus, term_statistics, weighted_pagerank,
};
use crate::syntax::{
    SemanticFacts, extract_ast_symbols_bounded, extract_semantic_facts_bounded,
    supports_tree_sitter_path,
};

pub const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_DISCOVERED_FILES: usize = 50_000;
pub const MAX_DISCOVERED_ENTRIES: usize = 100_000;
pub const MAX_DISCOVERED_PATH_BYTES: usize = 16 * 1024 * 1024;
/// Default hard ceiling for adaptive retrieval. A normal call starts at 32 MiB and expands only
/// when bounded evidence remains incomplete and confidence is low.
pub const MAX_SCAN_BYTES: usize = 512 * 1024 * 1024;
pub const ADAPTIVE_INITIAL_SCAN_BYTES: usize = 32 * 1024 * 1024;
pub const MIN_CONFIGURED_SCAN_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_CONFIGURED_SCAN_BYTES: usize = 512 * 1024 * 1024;
const MAX_INDEX_UNIQUE_TERMS_PER_FILE: usize = 32_768;
const MAX_INDEX_SUBSTRING_GRAMS_PER_FILE: usize = 16_384;
const MAX_INDEX_TOTAL_ENTRIES: usize = 4_000_000;
pub const MAX_SEARCH_RESULTS: usize = 32;
pub const MAX_SEARCH_CANDIDATES: usize = 512;
pub const DEFAULT_CONTEXT_LINES: usize = 8;
pub const MAX_SEARCH_EXCERPT_BYTES: usize = 2 * 1024;
pub const MAX_SEARCH_WALL_TIME: Duration = Duration::from_secs(20);
const ADAPTIVE_CONFIDENCE_STOP: f64 = 0.78;
const MAX_ANALYSIS_CACHE_FILES: usize = 256;
const MAX_GRAPH_CACHE_ENTRIES: usize = 64;
const MAX_SESSION_MEMORY_RECORDS: usize = 128;
// Structural-map redaction must not amplify a bounded source file into a much larger retained
// buffer. Bounded redaction also refuses to run the allocating per-line redactors over an
// attacker-controlled giant/minified line; that line is suppressed instead.
const MAX_REPOSITORY_MAP_SOURCE_BYTES: usize = 32 * 1024 * 1024;
const MAX_BOUNDED_REDACTION_LINE_BYTES: usize = 64 * 1024;
const REDACTED_OVERSIZE_LINE: &str = "[SIPPION_REDACTED_OVERSIZE_LINE]";
const REDACTED_MATCH_EXCERPT: &str = "[SIPPION_REDACTED_MATCH: matching source content suppressed]";

pub const BUILTIN_PRUNED_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "coverage",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    "out",
    "vendor",
    ".terraform",
    ".gradle",
    ".dart_tool",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".tox",
    ".turbo",
    ".parcel-cache",
    ".nuxt",
    ".svelte-kit",
    ".build",
    "pods",
    "deriveddata",
];

const BUILTIN_PRUNED_FILES: &[&str] = &[
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
    "cargo.lock",
    "poetry.lock",
    "pdm.lock",
    "uv.lock",
    "pipfile.lock",
    "composer.lock",
    "gemfile.lock",
    "podfile.lock",
    "pubspec.lock",
];

const CONTENT_MATCH_BASE_SCORE: usize = MAX_QUERY_TERMS * 3 + 1;

const OBVIOUS_BINARY_EXTENSIONS: &[&str] = &[
    "7z", "a", "avif", "bin", "bmp", "bz2", "class", "db", "dll", "dylib", "eot", "exe", "flac",
    "gif", "gz", "heic", "heif", "ico", "jar", "jpeg", "jpg", "m4a", "mkv", "mov", "mp3", "mp4",
    "o", "obj", "ogg", "otf", "parquet", "pdf", "pdb", "pkl", "png", "pyc", "rar", "so", "sqlite",
    "sqlite3", "tar", "tgz", "ttf", "wav", "wasm", "webm", "webp", "woff", "woff2", "xz", "zip",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryAccessError {
    InvalidRelativePath,
    NonUtf8Path,
    DeniedPath,
    PrunedPath,
    NotRegularFile,
    NotFound,
    TooLarge,
    NonUtf8Source,
    HardLinkedFile,
    ConcurrentModification,
    Io,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VerifiedSource {
    pub text: String,
    /// Raw bytes actually read. Local scan budgets must use this value.
    pub source_bytes: usize,
    /// Stamp from the exact open file handle after the read completed.
    pub stamp: SourceStamp,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub relative_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub excerpt: String,
    pub score: f64,
    // Internal identity captured during exact verification. These fields are deliberately not
    // model-visible; structural mapping uses them to reject a file that changed after evidence
    // was collected. The content fingerprint also covers Windows same-size/same-mtime rewrites.
    source_stamp: Option<SourceStamp>,
    source_fingerprint: Option<(u64, u64)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchCoverage {
    /// True only when metadata discovery completed within the entry/path/time guards.
    pub discovery_complete: bool,
    pub eligible_files: usize,
    pub indexed_files: usize,
    pub partial_index_files: usize,
    /// Policy exclusions that can hide searchable content (for example ignore-rule-hidden content,
    /// pruned/denied/binary paths, symlinks, >2 MiB files, stable non-UTF-8 sources, or hard-linked
    /// files). Directories with .gitignore/.ignore controls and pruned directory subtrees contribute
    /// conservative sentinels because the exact number of hidden files is intentionally not scanned.
    pub policy_excluded_files: usize,
    pub scanned_files: usize,
    pub scanned_bytes: usize,
    /// Cumulative adaptive scan allowance granted to this tool call.
    pub scan_budget_bytes: usize,
    /// Configured hard ceiling for adaptive expansion.
    pub scan_budget_cap_bytes: usize,
    /// Number of adaptive retrieval rounds used.
    pub adaptive_rounds: usize,
    /// 0..=1000 confidence score for deterministic model-visible reporting.
    pub confidence_milli: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchOutcome {
    pub hits: Vec<SearchHit>,
    /// True when discovery, RAM-index coverage, verification, candidate generation, or source
    /// scanning did not cover the full eligible search space.
    pub truncated: bool,
    pub coverage: SearchCoverage,
    /// True only when granting more adaptive scan bytes can plausibly improve completeness.
    /// Candidate-cap truncation alone is incomplete but not scan-budget-expandable.
    adaptive_expandable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SourceStamp {
    len: u64,
    modified_nanos: Option<u128>,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    #[cfg(unix)]
    ctime: i64,
    #[cfg(unix)]
    ctime_nsec: i64,
    #[cfg(unix)]
    nlink: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RootIdentity {
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    #[cfg(windows)]
    volume_serial_number: u64,
    #[cfg(windows)]
    file_index: u64,
    #[cfg(all(not(unix), not(windows)))]
    created_nanos: Option<u128>,
}

#[derive(Debug, Clone)]
struct DiscoveredFile {
    path: String,
    stamp: Option<SourceStamp>,
}

struct DiscoveryOutcome {
    files: Vec<DiscoveredFile>,
    policy_excluded_files: usize,
    truncated: bool,
}

#[derive(Debug, Clone)]
struct IndexedDocument {
    stamp: Option<SourceStamp>,
    document_len: usize,
    terms: Vec<(u64, u16)>,
    substring_grams: Vec<u32>,
    term_truncated: bool,
}

#[derive(Debug, Default)]
struct RamIndex {
    files: HashMap<String, IndexedDocument>,
    total_entries: usize,
    saturated: bool,
}

#[derive(Debug, Clone)]
struct PendingFile {
    file: DiscoveredFile,
    path_bonus: usize,
    changed: bool,
}

#[derive(Debug, Clone)]
struct RankedCandidate {
    relative_path: String,
    term_frequencies: Vec<usize>,
    document_len: usize,
    path_bonus: usize,
    has_content: bool,
    score: f64,
}

#[derive(Debug, Clone)]
enum VerifiedEvidence {
    None,
    Visible {
        start_line: u32,
        end_line: u32,
        excerpt: String,
        exact_bonus: usize,
        structure_bonus: f64,
    },
    Redacted,
}

#[derive(Debug, Clone)]
struct VerifiedCandidate {
    stamp: SourceStamp,
    fingerprint: (u64, u64),
    document_len: usize,
    term_frequencies: Vec<usize>,
    evidence: VerifiedEvidence,
}

#[derive(Debug, Default)]
struct ScanLaneOutcome {
    bytes: usize,
    files: usize,
    incomplete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndexFlightClaim {
    Leader,
    AlreadyIndexed,
    TimedOut,
}

#[derive(Debug, Clone)]
struct SearchMemory {
    session_id: String,
    agent_id: Option<String>,
    terms: Vec<String>,
    paths: Vec<String>,
}

#[derive(Debug, Clone)]
struct CachedRepoMapSymbol {
    name: String,
    kind: String,
    line: u32,
}

#[derive(Debug, Clone)]
struct CachedAnalysis {
    stamp: SourceStamp,
    // Deliberately structural only: never retain source-line signatures in the shared cache.
    symbols: Vec<CachedRepoMapSymbol>,
    semantics: SemanticFacts,
    cacheable: bool,
    last_used: u64,
}

#[derive(Debug, Default)]
struct AnalysisCacheState {
    entries: HashMap<String, CachedAnalysis>,
    inflight: HashSet<String>,
    tick: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct GraphCacheNode {
    path: String,
    stamp: SourceStamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct GraphCacheKey(Vec<GraphCacheNode>);

#[derive(Debug, Clone)]
struct CachedGraph {
    edge_maps: Vec<HashMap<usize, (f64, String)>>,
    centrality: Vec<f64>,
    last_used: u64,
}

#[derive(Debug, Default)]
struct GraphCacheState {
    entries: HashMap<GraphCacheKey, CachedGraph>,
    tick: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RepoMapSymbol {
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RepoMapLink {
    pub relative_path: String,
    pub kind: String,
    pub weight: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RepoMapEntry {
    pub relative_path: String,
    pub score: f64,
    pub symbols: Vec<RepoMapSymbol>,
    pub links_to: Vec<String>,
    pub semantic_links: Vec<RepoMapLink>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RepositoryMapOutcome {
    pub entries: Vec<RepoMapEntry>,
    pub truncated: bool,
    /// Search evidence that could not be confirmed against the current file generation while
    /// building this context. Callers must not render excerpts for these paths.
    pub invalidated_evidence_paths: Vec<String>,
}

#[derive(Debug, Clone)]
struct MapCandidate {
    relative_path: String,
    stamp: SourceStamp,
    search_score: f64,
    source_lower: String,
    symbols: Vec<RepoMapSymbol>,
    definition_names: Vec<String>,
    semantics: SemanticFacts,
    analysis_cacheable: bool,
    semantic_query_bonus: f64,
}

/// One process owns one trusted project root. Model paths are resolved component-by-component
/// through capability directory handles, and symlinks are refused rather than followed.
pub struct RepositoryAccess {
    root_path: PathBuf,
    root_dir: Dir,
    // Stable identity of the capability-opened project root. Ambient path walking is permitted only
    // while the configured root path still resolves to this exact directory.
    root_identity: RootIdentity,
    max_scan_budget_bytes: usize,
    // Search index is deliberately RAM-only. It stores hashed lexical statistics and file stamps,
    // never source bodies, and disappears with the process.
    ram_index: Mutex<RamIndex>,
    // Windows cannot safely reuse metadata-only freshness across top-level searches at this MSRV.
    // Serialize those searches so one request cannot clear another request's in-progress index.
    #[cfg(windows)]
    windows_search_serial: Mutex<()>,
    // The same metadata limitation applies to the structural analysis and graph caches. Serialize
    // top-level map construction before discarding those cross-request caches on Windows so a
    // same-size/same-mtime replacement can never reuse stale structural facts.
    #[cfg(windows)]
    windows_map_serial: Mutex<()>,
    // File-level single-flight prevents concurrent cold-start queries from reading/indexing the
    // same unchanged file multiple times.
    index_inflight: Mutex<HashSet<String>>,
    index_ready: Condvar,
    // Engram-inspired memory is deliberately volatile: paths/query terms plus bounded coordination
    // identifiers only, never source bodies or secrets, and never written to disk.
    session_memory: Mutex<VecDeque<SearchMemory>>,
    // Parsed structural facts are safe to share across subagents because the cache stores only
    // symbols/semantic facts keyed by a verified source stamp; source bodies are never retained.
    analysis_cache: Mutex<AnalysisCacheState>,
    analysis_ready: Condvar,
    graph_cache: Mutex<GraphCacheState>,
}

struct IndexFlightGuard<'a> {
    repository: &'a RepositoryAccess,
    path: String,
}

impl<'a> IndexFlightGuard<'a> {
    fn new(repository: &'a RepositoryAccess, path: String) -> Self {
        Self { repository, path }
    }
}

impl Drop for IndexFlightGuard<'_> {
    fn drop(&mut self) {
        self.repository.release_index_flight(&self.path);
    }
}

struct AnalysisFlightGuard<'a> {
    repository: &'a RepositoryAccess,
    path: String,
}

impl<'a> AnalysisFlightGuard<'a> {
    fn new(repository: &'a RepositoryAccess, path: String) -> Self {
        Self { repository, path }
    }
}

impl Drop for AnalysisFlightGuard<'_> {
    fn drop(&mut self) {
        let mut state = match self.repository.analysis_cache.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.inflight.remove(&self.path);
        drop(state);
        self.repository.analysis_ready.notify_all();
    }
}

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

    fn ensure_root_path_identity(&self) -> Result<(), RepositoryAccessError> {
        let current = Dir::open_ambient_dir(&self.root_path, ambient_authority())
            .map_err(|_| RepositoryAccessError::ConcurrentModification)?;
        let current_identity = root_identity_from_dir(&current)
            .map_err(|_| RepositoryAccessError::ConcurrentModification)?;
        if current_identity != self.root_identity {
            return Err(RepositoryAccessError::ConcurrentModification);
        }
        Ok(())
    }

    fn verified_metadata_stamp(
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
        Ok(cap_source_stamp(&metadata))
    }

    fn open_file_nofollow(
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

    fn discover_files(
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
            let stamp = metadata.as_ref().map(source_stamp);
            if metadata.as_ref().is_some_and(policy_excluded_by_metadata)
                || stamp
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

    fn read_source(
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
        let before_stamp = cap_source_stamp(&before);

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
        let after_stamp = cap_source_stamp(&after);
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
    fn reset_ram_index(&self) -> Result<(), RepositoryAccessError> {
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
    fn reset_structural_caches(&self) -> Result<(), RepositoryAccessError> {
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

    /// Incremental bounded hybrid retrieval. Repository contents are indexed into RAM as hashed
    /// lexical statistics, not retained source bodies. On non-Windows targets, discovery metadata
    /// invalidates changed files so only changed/unindexed files consume the broad indexing budget
    /// on later calls. Windows rebuilds the RAM index once per top-level search because its stable
    /// MSRV metadata surface is not a sufficient cross-request content identity.
    /// Adaptive bounded hybrid retrieval. Calls begin with a 32 MiB allowance (or a lower
    /// configured ceiling) and expand 32 -> 64 -> 128 -> 256 -> 512 MiB only while coverage is
    /// incomplete and confidence remains below the stop threshold. The RAM index persists across
    /// rounds, so already indexed unchanged files are not repeatedly retained as source bodies.
    #[cfg(test)]
    pub fn search(
        &self,
        query: &NormalizedQuery,
        max_results: usize,
        cancellation: Option<&AtomicBool>,
    ) -> Result<SearchOutcome, RepositoryAccessError> {
        self.search_coordinated(query, max_results, cancellation, None)
    }

    #[cfg(test)]
    pub fn search_coordinated(
        &self,
        query: &NormalizedQuery,
        max_results: usize,
        cancellation: Option<&AtomicBool>,
        context: Option<&CoordinationContext>,
    ) -> Result<SearchOutcome, RepositoryAccessError> {
        let started = Instant::now();
        self.search_coordinated_since(query, max_results, cancellation, context, &started)
    }

    pub fn search_coordinated_since(
        &self,
        query: &NormalizedQuery,
        max_results: usize,
        cancellation: Option<&AtomicBool>,
        context: Option<&CoordinationContext>,
        started: &Instant,
    ) -> Result<SearchOutcome, RepositoryAccessError> {
        if is_cancelled(cancellation) {
            return Err(RepositoryAccessError::Cancelled);
        }
        // On Windows the stable std::fs metadata surface available to the MSRV does not expose a
        // stable file identity/change counter. Size + mtime can therefore be preserved across a
        // same-length replacement. Serialize top-level searches, then discard the previous RAM
        // index before discovery. Adaptive rounds within this one search still share the rebuilt
        // index, so completeness can accumulate normally without stale cross-request reuse.
        #[cfg(windows)]
        let _windows_search_guard = self
            .windows_search_serial
            .lock()
            .map_err(|_| RepositoryAccessError::Io)?;
        #[cfg(windows)]
        self.reset_ram_index()?;

        let cap = self.max_scan_budget_bytes;
        let mut target = ADAPTIVE_INITIAL_SCAN_BYTES.min(cap);
        let mut granted_allowance = 0usize;
        let mut total_scanned_bytes = 0usize;
        let mut total_scanned_files = 0usize;
        let mut rounds = 0usize;
        // Per-call cache for stable policy exclusions discovered only after reading (notably non-UTF-8).
        // This prevents adaptive rounds from repeatedly retrying the same deliberately excluded file.
        let mut policy_skips = HashMap::<String, SourceStamp>::new();
        // Exact verification is cumulative within one adaptive search. Cache only derived evidence
        // and identity, never full source bodies, so each additional round spends its byte grant on
        // candidates that have not already been verified at the same file generation.
        let mut verification_cache = HashMap::<String, VerifiedCandidate>::new();

        loop {
            let round_allowance = target.saturating_sub(granted_allowance);
            rounds = rounds.saturating_add(1);
            let mut outcome = self.search_once(
                query,
                max_results,
                cancellation,
                started,
                round_allowance.max(1),
                &mut policy_skips,
                &mut verification_cache,
                context,
            )?;
            granted_allowance = target;
            total_scanned_bytes =
                total_scanned_bytes.saturating_add(outcome.coverage.scanned_bytes);
            total_scanned_files =
                total_scanned_files.saturating_add(outcome.coverage.scanned_files);

            let confidence = search_confidence(query, &outcome);
            outcome.coverage.scanned_bytes = total_scanned_bytes;
            outcome.coverage.scanned_files = total_scanned_files;
            outcome.coverage.scan_budget_bytes = granted_allowance;
            outcome.coverage.scan_budget_cap_bytes = cap;
            outcome.coverage.adaptive_rounds = rounds;
            outcome.coverage.confidence_milli =
                (confidence.clamp(0.0, 1.0) * 1000.0).round() as u16;

            let complete_no_match = outcome.hits.is_empty()
                && !outcome.truncated
                && outcome.coverage.policy_excluded_files == 0;
            let should_expand = !complete_no_match
                && outcome.truncated
                && outcome.adaptive_expandable
                && confidence < ADAPTIVE_CONFIDENCE_STOP
                && target < cap
                && !search_timed_out(started);
            if !should_expand {
                self.remember_search(query, &outcome.hits, context);
                return Ok(outcome);
            }

            let next = target.saturating_mul(2).min(cap);
            if next <= target {
                self.remember_search(query, &outcome.hits, context);
                return Ok(outcome);
            }
            target = next;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn search_once(
        &self,
        query: &NormalizedQuery,
        max_results: usize,
        cancellation: Option<&AtomicBool>,
        started: &Instant,
        round_budget_bytes: usize,
        policy_skips: &mut HashMap<String, SourceStamp>,
        verification_cache: &mut HashMap<String, VerifiedCandidate>,
        context: Option<&CoordinationContext>,
    ) -> Result<SearchOutcome, RepositoryAccessError> {
        if is_cancelled(cancellation) {
            return Err(RepositoryAccessError::Cancelled);
        }
        let terms = &query.terms;
        if max_results == 0 {
            return Ok(SearchOutcome {
                hits: Vec::new(),
                truncated: false,
                coverage: SearchCoverage {
                    scan_budget_bytes: round_budget_bytes,
                    scan_budget_cap_bytes: self.max_scan_budget_bytes,
                    adaptive_rounds: 1,
                    ..SearchCoverage::default()
                },
                adaptive_expandable: false,
            });
        }

        let requested_results = max_results.min(MAX_SEARCH_RESULTS);
        let candidate_limit = requested_results
            .saturating_mul(16)
            .clamp(64, MAX_SEARCH_CANDIDATES);
        let discovery = self.discover_files(cancellation, started, policy_skips)?;
        let eligible_paths = discovery
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<HashSet<_>>();

        // Reconcile the volatile index with current discovery metadata. A truncated discovery is
        // only a lower bound on the live repository, so previously-indexed paths not observed in
        // that partial walk are retained rather than incorrectly treated as deleted.
        let mut pending = Vec::new();
        {
            let mut index = self
                .ram_index
                .lock()
                .map_err(|_| RepositoryAccessError::Io)?;
            if !discovery.truncated {
                index.files.retain(|path, _| eligible_paths.contains(path));
            }
            index.total_entries = index
                .files
                .values()
                .map(|doc| doc.terms.len().saturating_add(doc.substring_grams.len()))
                .sum();
            index.saturated = false;

            for file in &discovery.files {
                let path_bonus = path_match_score(&file.path, terms);
                let changed = index
                    .files
                    .get(&file.path)
                    .is_some_and(|doc| doc.stamp != file.stamp);
                let missing = !index.files.contains_key(&file.path);
                if changed {
                    if let Some(old) = index.files.remove(&file.path) {
                        index.total_entries = index.total_entries.saturating_sub(
                            old.terms.len().saturating_add(old.substring_grams.len()),
                        );
                    }
                }
                if missing || changed {
                    pending.push(PendingFile {
                        file: file.clone(),
                        path_bonus,
                        changed,
                    });
                }
            }
        }

        let (priority_lane, sample_lane, broad_lane) = stratified_pending_lanes(pending);
        // Reserve one quarter of the configured budget for verifying model-visible evidence from
        // the index. The remaining three quarters grow/refresh index coverage.
        let index_budget = round_budget_bytes.saturating_mul(3) / 4;
        let verify_budget = round_budget_bytes.saturating_sub(index_budget);
        let priority_cap = index_budget / 2;
        let sample_cap = index_budget / 8;
        let mut indexed_read_bytes = 0usize;
        let mut scanned_files = 0usize;
        let mut scan_incomplete = false;
        let mut scanned_paths = HashSet::new();

        let priority = self.scan_index_lane(
            &priority_lane,
            priority_cap,
            started,
            cancellation,
            &mut scanned_paths,
            policy_skips,
        )?;
        indexed_read_bytes = indexed_read_bytes.saturating_add(priority.bytes);
        scanned_files = scanned_files.saturating_add(priority.files);
        scan_incomplete |= priority.incomplete;

        let remaining_after_priority = index_budget.saturating_sub(indexed_read_bytes);
        let sample = self.scan_index_lane(
            &sample_lane,
            sample_cap.min(remaining_after_priority),
            started,
            cancellation,
            &mut scanned_paths,
            policy_skips,
        )?;
        indexed_read_bytes = indexed_read_bytes.saturating_add(sample.bytes);
        scanned_files = scanned_files.saturating_add(sample.files);
        scan_incomplete |= sample.incomplete;

        let remaining_for_broad = index_budget.saturating_sub(indexed_read_bytes);
        let broad = self.scan_index_lane(
            &broad_lane,
            remaining_for_broad,
            started,
            cancellation,
            &mut scanned_paths,
            policy_skips,
        )?;
        indexed_read_bytes = indexed_read_bytes.saturating_add(broad.bytes);
        scanned_files = scanned_files.saturating_add(broad.files);
        scan_incomplete |= broad.incomplete;

        if is_cancelled(cancellation) {
            return Err(RepositoryAccessError::Cancelled);
        }

        // Corpus statistics come from the current RAM index. Query matching uses stable hashes of
        // full identifiers plus common identifier subterms; final evidence is always re-read and
        // checked with the original text matcher before becoming model-visible.
        let indexed_query = terms
            .iter()
            .map(|term| (stable_term_hash(term), query_substring_grams(term)))
            .collect::<Vec<_>>();
        let mut document_frequencies = vec![0usize; terms.len()];
        let mut document_count = 0usize;
        let mut total_document_len = 0usize;
        let mut ranked = Vec::new();
        let mut indexed_paths = HashSet::new();
        {
            let index = self
                .ram_index
                .lock()
                .map_err(|_| RepositoryAccessError::Io)?;
            for (path, document) in &index.files {
                if !eligible_paths.contains(path) {
                    continue;
                }
                document_count = document_count.saturating_add(1);
                total_document_len = total_document_len.saturating_add(document.document_len);
                let frequencies = indexed_query_frequencies(document, &indexed_query);
                for (position, frequency) in frequencies.iter().enumerate() {
                    if *frequency > 0 {
                        document_frequencies[position] =
                            document_frequencies[position].saturating_add(1);
                    }
                }
                let path_bonus = path_match_score(path, terms);
                let has_content = frequencies.iter().any(|frequency| *frequency > 0);
                indexed_paths.insert(path.clone());
                if has_content || path_bonus > 0 {
                    ranked.push(RankedCandidate {
                        relative_path: path.clone(),
                        term_frequencies: frequencies,
                        document_len: document.document_len,
                        path_bonus,
                        has_content,
                        score: 0.0,
                    });
                }
            }
        }

        // A path match is still useful even when its body has not yet entered the incremental index.
        for file in &discovery.files {
            if indexed_paths.contains(&file.path) {
                continue;
            }
            if file
                .stamp
                .as_ref()
                .is_some_and(|stamp| policy_skips.get(&file.path) == Some(stamp))
            {
                continue;
            }
            let path_bonus = path_match_score(&file.path, terms);
            if path_bonus > 0 {
                ranked.push(RankedCandidate {
                    relative_path: file.path.clone(),
                    term_frequencies: vec![0; terms.len()],
                    document_len: 1,
                    path_bonus,
                    has_content: false,
                    score: (path_bonus * 3) as f64,
                });
            }
        }

        let average_document_len = if document_count == 0 {
            1.0
        } else {
            total_document_len as f64 / document_count as f64
        };
        for candidate in &mut ranked {
            let memory_bonus = self.memory_adjustment(terms, &candidate.relative_path, context);
            if candidate.has_content {
                let matched = candidate
                    .term_frequencies
                    .iter()
                    .filter(|frequency| **frequency > 0)
                    .count();
                let bm25 = bm25_score(
                    &candidate.term_frequencies,
                    candidate.document_len,
                    average_document_len,
                    &document_frequencies,
                    document_count,
                );
                candidate.score =
                    (CONTENT_MATCH_BASE_SCORE + matched * 10 + candidate.path_bonus * 3) as f64
                        + bm25 * 12.0
                        + memory_bonus;
            } else {
                candidate.score = (candidate.path_bonus * 3) as f64 + memory_bonus.min(4.0);
            }
        }
        ranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.relative_path.cmp(&b.relative_path))
        });
        // Candidate generation may contain false positives (for example n-gram collisions).
        // If we discard any candidates before exact verification, the search space is incomplete and
        // must never be upgraded to a complete NO_MATCH.
        let candidate_generation_truncated = ranked.len() > candidate_limit;
        ranked.truncate(candidate_limit);

        let stamps = discovery
            .files
            .iter()
            .map(|file| (file.path.as_str(), file.stamp.clone()))
            .collect::<HashMap<_, _>>();
        let query_lower = &query.raw_lower;
        let mut verified_bytes = 0usize;
        let mut verification_incomplete = false;
        let mut hits = Vec::new();

        // Verify the whole bounded candidate set (subject to time/byte guards), then rank and cut
        // Top-N. Exact results are cached across adaptive rounds at the same SourceStamp so later
        // byte grants advance into new candidates instead of rereading the same leading files.
        for candidate in ranked {
            if is_cancelled(cancellation) {
                return Err(RepositoryAccessError::Cancelled);
            }
            if search_timed_out(started) {
                verification_incomplete = true;
                break;
            }

            if !candidate.has_content {
                let Some(Some(discovered_stamp)) = stamps.get(candidate.relative_path.as_str())
                else {
                    verification_incomplete = true;
                    continue;
                };
                match self.verified_metadata_stamp(&candidate.relative_path) {
                    Ok(verified_stamp) if &verified_stamp == discovered_stamp => {
                        hits.push(SearchHit {
                            relative_path: candidate.relative_path,
                            start_line: 0,
                            end_line: 0,
                            excerpt: String::new(),
                            score: candidate.score,
                            source_stamp: Some(verified_stamp),
                            source_fingerprint: None,
                        });
                    }
                    Ok(_) => {
                        verification_incomplete = true;
                    }
                    Err(error) => {
                        if matches!(error, RepositoryAccessError::HardLinkedFile) {
                            policy_skips
                                .insert(candidate.relative_path.clone(), discovered_stamp.clone());
                        }
                        if read_failure_makes_scan_incomplete(&error) {
                            verification_incomplete = true;
                        }
                    }
                }
                continue;
            }

            let Some(Some(discovered_stamp)) = stamps.get(candidate.relative_path.as_str()) else {
                verification_incomplete = true;
                continue;
            };

            // A cached exact result costs no bytes in this round. If discovery observed a different
            // generation, drop the stale derived evidence and verify the new generation normally.
            let cache_is_current = verification_cache
                .get(candidate.relative_path.as_str())
                .is_some_and(|verified| &verified.stamp == discovered_stamp);
            if cache_is_current {
                if let Some(verified) = verification_cache.get(candidate.relative_path.as_str()) {
                    if let Some(hit) = self.hit_from_verified_candidate(
                        &candidate,
                        verified,
                        average_document_len,
                        &document_frequencies,
                        document_count,
                        terms,
                        context,
                    ) {
                        hits.push(hit);
                    }
                }
                continue;
            }
            verification_cache.remove(candidate.relative_path.as_str());

            if verified_bytes >= verify_budget {
                verification_incomplete = true;
                break;
            }
            let remaining = verify_budget.saturating_sub(verified_bytes);
            if discovered_stamp.len > remaining as u64 {
                // This candidate cannot fit in the remaining verification budget, but a later
                // smaller candidate may still fit. Skip only this candidate instead of
                // truncating the rest of the ranked candidate set.
                verification_incomplete = true;
                continue;
            }

            let source = match self.read_source(&candidate.relative_path) {
                Ok(source) => source,
                Err((error, consumed_bytes)) => {
                    verified_bytes = verified_bytes.saturating_add(consumed_bytes);
                    if matches!(
                        error,
                        RepositoryAccessError::NonUtf8Source
                            | RepositoryAccessError::HardLinkedFile
                    ) {
                        policy_skips
                            .insert(candidate.relative_path.clone(), discovered_stamp.clone());
                    }
                    if read_failure_makes_scan_incomplete(&error) {
                        verification_incomplete = true;
                    }
                    continue;
                }
            };
            verified_bytes = verified_bytes.saturating_add(source.source_bytes);
            scanned_files = scanned_files.saturating_add(1);
            if verified_bytes > verify_budget {
                verification_incomplete = true;
            }
            if &source.stamp != discovered_stamp {
                // Discovery and capability-scoped verification did not observe the same file object.
                verification_incomplete = true;
                continue;
            }

            let indexed = build_indexed_document(&source.text, Some(source.stamp.clone()));
            self.insert_index_document(candidate.relative_path.clone(), indexed)?;

            let (document_len, term_frequencies) = term_statistics(&source.text, terms);
            let has_content_match = term_frequencies.iter().any(|frequency| *frequency > 0);
            let fingerprint = source_content_fingerprint(&source.text);
            let mut evidence_scan_complete = true;
            let evidence = if !has_content_match {
                VerifiedEvidence::None
            } else {
                let safe_text = redact_high_confidence_secrets(&source.text);
                let safe_lower = safe_text.to_ascii_lowercase();
                let redaction_suppressed_match = safe_text != source.text
                    && !terms.iter().any(|term| safe_lower.contains(term.as_str()));
                let lines = safe_text.lines().collect::<Vec<_>>();
                let mut best: Option<(SearchHit, usize, f64)> = None;
                for (index, lower) in safe_lower.lines().enumerate() {
                    if index % 256 == 0 {
                        if is_cancelled(cancellation) {
                            return Err(RepositoryAccessError::Cancelled);
                        }
                        if search_timed_out(started) {
                            verification_incomplete = true;
                            evidence_scan_complete = false;
                            break;
                        }
                    }
                    let Some(match_byte) = first_term_match_byte(lower, terms) else {
                        continue;
                    };
                    let (excerpt, start, end) = bounded_search_excerpt(&lines, index, match_byte);
                    let evidence_lower = excerpt.to_ascii_lowercase();
                    let matched = terms
                        .iter()
                        .filter(|term| evidence_lower.contains(term.as_str()))
                        .count();
                    let exact_bonus = usize::from(evidence_lower.contains(query_lower));
                    let structure_bonus = structural_line_bonus(lines[index], terms);
                    let preliminary = SearchHit {
                        relative_path: candidate.relative_path.clone(),
                        start_line: (start + 1) as u32,
                        end_line: end as u32,
                        excerpt,
                        score: (CONTENT_MATCH_BASE_SCORE
                            + matched * 10
                            + exact_bonus * 8
                            + candidate.path_bonus * 3) as f64
                            + structure_bonus,
                        source_stamp: Some(source.stamp.clone()),
                        source_fingerprint: Some(fingerprint),
                    };
                    if best
                        .as_ref()
                        .is_none_or(|(current, _, _)| hit_is_better(&preliminary, current))
                    {
                        best = Some((preliminary, exact_bonus, structure_bonus));
                    }
                }

                if let Some((hit, exact_bonus, structure_bonus)) = best {
                    VerifiedEvidence::Visible {
                        start_line: hit.start_line,
                        end_line: hit.end_line,
                        excerpt: hit.excerpt,
                        exact_bonus,
                        structure_bonus,
                    }
                } else if redaction_suppressed_match {
                    // Exact verification found the query in the original source, but the model-visible
                    // redacted form no longer contains any query term. Preserve the existence signal
                    // without revealing the matching secret value or its source line.
                    VerifiedEvidence::Redacted
                } else {
                    VerifiedEvidence::None
                }
            };

            let verified = VerifiedCandidate {
                stamp: source.stamp,
                fingerprint,
                document_len,
                term_frequencies,
                evidence,
            };
            if let Some(hit) = self.hit_from_verified_candidate(
                &candidate,
                &verified,
                average_document_len,
                &document_frequencies,
                document_count,
                terms,
                context,
            ) {
                hits.push(hit);
            }
            // Do not memoize a partially scanned evidence result at the shared deadline. A future
            // implementation with a refreshed deadline must be able to rescan it completely.
            if evidence_scan_complete {
                verification_cache.insert(candidate.relative_path, verified);
            }
        }

        sort_hits(&mut hits);
        hits.truncate(requested_results);

        // Files learned to be stable non-UTF-8 during this round are deliberate policy exclusions,
        // not unfinished retrieval. Remove them from the effective searchable set immediately.
        let newly_policy_excluded = eligible_paths
            .iter()
            .filter(|path| {
                let Some(stamp) = stamps.get(path.as_str()).cloned().flatten() else {
                    return false;
                };
                policy_skips.get(path.as_str()) == Some(&stamp)
            })
            .cloned()
            .collect::<HashSet<_>>();
        let effective_eligible_paths = eligible_paths
            .iter()
            .filter(|path| !newly_policy_excluded.contains(*path))
            .cloned()
            .collect::<HashSet<_>>();
        let eligible_files = effective_eligible_paths.len();
        let policy_excluded_files = discovery
            .policy_excluded_files
            .saturating_add(newly_policy_excluded.len());

        let (indexed_files, partial_index_files, saturated) = {
            let index = self
                .ram_index
                .lock()
                .map_err(|_| RepositoryAccessError::Io)?;
            let indexed_files = effective_eligible_paths
                .iter()
                .filter(|path| index.files.contains_key(path.as_str()))
                .count();
            let partial_index_files = effective_eligible_paths
                .iter()
                .filter_map(|path| index.files.get(path.as_str()))
                .filter(|doc| doc.term_truncated)
                .count();
            (indexed_files, partial_index_files, index.saturated)
        };
        let coverage = SearchCoverage {
            discovery_complete: !discovery.truncated,
            eligible_files,
            indexed_files,
            partial_index_files,
            policy_excluded_files,
            scanned_files,
            scanned_bytes: indexed_read_bytes.saturating_add(verified_bytes),
            scan_budget_bytes: round_budget_bytes,
            scan_budget_cap_bytes: self.max_scan_budget_bytes,
            adaptive_rounds: 1,
            confidence_milli: 0,
        };
        let adaptive_expandable = discovery.truncated
            || scan_incomplete
            || verification_incomplete
            || saturated
            || partial_index_files > 0
            || indexed_files < eligible_files;
        let truncated = candidate_generation_truncated || adaptive_expandable;
        Ok(SearchOutcome {
            hits,
            truncated,
            coverage,
            adaptive_expandable,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn hit_from_verified_candidate(
        &self,
        candidate: &RankedCandidate,
        verified: &VerifiedCandidate,
        average_document_len: f64,
        document_frequencies: &[usize],
        document_count: usize,
        terms: &[String],
        context: Option<&CoordinationContext>,
    ) -> Option<SearchHit> {
        let has_content_match = verified
            .term_frequencies
            .iter()
            .any(|frequency| *frequency > 0);
        if !has_content_match {
            return (candidate.path_bonus > 0).then(|| SearchHit {
                relative_path: candidate.relative_path.clone(),
                start_line: 0,
                end_line: 0,
                excerpt: String::new(),
                score: (candidate.path_bonus * 3) as f64,
                source_stamp: Some(verified.stamp.clone()),
                source_fingerprint: Some(verified.fingerprint),
            });
        }

        let matched = verified
            .term_frequencies
            .iter()
            .filter(|frequency| **frequency > 0)
            .count();
        let bm25 = bm25_score(
            &verified.term_frequencies,
            verified.document_len,
            average_document_len,
            document_frequencies,
            document_count.max(1),
        );
        let memory_bonus = self.memory_adjustment(terms, &candidate.relative_path, context);
        match &verified.evidence {
            VerifiedEvidence::Visible {
                start_line,
                end_line,
                excerpt,
                exact_bonus,
                structure_bonus,
            } => Some(SearchHit {
                relative_path: candidate.relative_path.clone(),
                start_line: *start_line,
                end_line: *end_line,
                excerpt: excerpt.clone(),
                score: (CONTENT_MATCH_BASE_SCORE
                    + matched * 10
                    + *exact_bonus * 8
                    + candidate.path_bonus * 3) as f64
                    + bm25 * 12.0
                    + *structure_bonus
                    + memory_bonus,
                source_stamp: Some(verified.stamp.clone()),
                source_fingerprint: Some(verified.fingerprint),
            }),
            VerifiedEvidence::Redacted => Some(SearchHit {
                relative_path: candidate.relative_path.clone(),
                start_line: 0,
                end_line: 0,
                excerpt: REDACTED_MATCH_EXCERPT.to_string(),
                score: (CONTENT_MATCH_BASE_SCORE + matched * 10 + candidate.path_bonus * 3) as f64
                    + bm25 * 12.0
                    + memory_bonus,
                source_stamp: Some(verified.stamp.clone()),
                source_fingerprint: Some(verified.fingerprint),
            }),
            VerifiedEvidence::None => None,
        }
    }

    fn index_document_is_current(
        &self,
        path: &str,
        expected: Option<&SourceStamp>,
    ) -> Result<bool, RepositoryAccessError> {
        let index = self
            .ram_index
            .lock()
            .map_err(|_| RepositoryAccessError::Io)?;
        Ok(index.files.get(path).is_some_and(|document| {
            match (document.stamp.as_ref(), expected) {
                (_, None) => true,
                (Some(actual), Some(expected)) => actual == expected,
                (None, Some(_)) => false,
            }
        }))
    }

    fn claim_index_flight(
        &self,
        path: &str,
        expected: Option<&SourceStamp>,
        started: &Instant,
        cancellation: Option<&AtomicBool>,
    ) -> Result<IndexFlightClaim, RepositoryAccessError> {
        loop {
            if is_cancelled(cancellation) {
                return Err(RepositoryAccessError::Cancelled);
            }
            if search_timed_out(started) {
                return Ok(IndexFlightClaim::TimedOut);
            }
            let mut inflight = self
                .index_inflight
                .lock()
                .map_err(|_| RepositoryAccessError::Io)?;
            if !inflight.contains(path) {
                // Recheck index state while owning the flight registry. This closes the race where
                // another worker finishes indexing between an earlier index check and flight claim.
                if self.index_document_is_current(path, expected)? {
                    return Ok(IndexFlightClaim::AlreadyIndexed);
                }
                inflight.insert(path.to_string());
                return Ok(IndexFlightClaim::Leader);
            }
            let (guard, _) = self
                .index_ready
                .wait_timeout(inflight, Duration::from_millis(20))
                .map_err(|_| RepositoryAccessError::Io)?;
            drop(guard);
        }
    }

    fn release_index_flight(&self, path: &str) {
        let mut inflight = match self.index_inflight.lock() {
            Ok(inflight) => inflight,
            Err(poisoned) => poisoned.into_inner(),
        };
        inflight.remove(path);
        drop(inflight);
        self.index_ready.notify_all();
    }

    fn scan_index_lane(
        &self,
        lane: &[PendingFile],
        byte_budget: usize,
        started: &Instant,
        cancellation: Option<&AtomicBool>,
        scanned_paths: &mut HashSet<String>,
        policy_skips: &mut HashMap<String, SourceStamp>,
    ) -> Result<ScanLaneOutcome, RepositoryAccessError> {
        let mut outcome = ScanLaneOutcome::default();
        if byte_budget == 0 {
            return Ok(outcome);
        }
        for pending in lane {
            if scanned_paths.contains(&pending.file.path) {
                continue;
            }
            if outcome.bytes >= byte_budget {
                break;
            }
            if is_cancelled(cancellation) {
                return Err(RepositoryAccessError::Cancelled);
            }
            if search_timed_out(started) {
                outcome.incomplete = true;
                break;
            }
            // Keep the configured source-read budget strict when discovery metadata is available.
            // Unknown metadata can still overshoot by at most one bounded source read (2 MiB).
            if let Some(stamp) = &pending.file.stamp {
                let remaining = byte_budget.saturating_sub(outcome.bytes);
                if stamp.len > remaining as u64 {
                    continue;
                }
            }
            match self.claim_index_flight(
                &pending.file.path,
                pending.file.stamp.as_ref(),
                started,
                cancellation,
            )? {
                IndexFlightClaim::AlreadyIndexed => {
                    scanned_paths.insert(pending.file.path.clone());
                    continue;
                }
                IndexFlightClaim::TimedOut => {
                    outcome.incomplete = true;
                    break;
                }
                IndexFlightClaim::Leader => {}
            }
            let _index_flight = IndexFlightGuard::new(self, pending.file.path.clone());

            scanned_paths.insert(pending.file.path.clone());
            let source = match self.read_source(&pending.file.path) {
                Ok(source) => source,
                Err((error, consumed_bytes)) => {
                    outcome.bytes = outcome.bytes.saturating_add(consumed_bytes);
                    if matches!(
                        error,
                        RepositoryAccessError::NonUtf8Source
                            | RepositoryAccessError::HardLinkedFile
                    ) {
                        if let Some(stamp) = pending.file.stamp.clone() {
                            policy_skips.insert(pending.file.path.clone(), stamp);
                        }
                    }
                    if read_failure_makes_scan_incomplete(&error) {
                        outcome.incomplete = true;
                    }
                    continue;
                }
            };
            outcome.bytes = outcome.bytes.saturating_add(source.source_bytes);
            outcome.files = outcome.files.saturating_add(1);
            let document = build_indexed_document(&source.text, Some(source.stamp.clone()));
            self.insert_index_document(pending.file.path.clone(), document)?;
        }
        Ok(outcome)
    }

    fn insert_index_document(
        &self,
        path: String,
        document: IndexedDocument,
    ) -> Result<(), RepositoryAccessError> {
        let mut index = self
            .ram_index
            .lock()
            .map_err(|_| RepositoryAccessError::Io)?;
        if let Some(old) = index.files.remove(&path) {
            index.total_entries = index
                .total_entries
                .saturating_sub(old.terms.len().saturating_add(old.substring_grams.len()));
        }
        let document_entries = document
            .terms
            .len()
            .saturating_add(document.substring_grams.len());
        if index.total_entries.saturating_add(document_entries) > MAX_INDEX_TOTAL_ENTRIES {
            index.saturated = true;
            return Ok(());
        }
        index.total_entries = index.total_entries.saturating_add(document_entries);
        index.files.insert(path, document);
        Ok(())
    }

    fn coordination_session_key(context: Option<&CoordinationContext>) -> String {
        context
            .and_then(|context| context.session_id.as_deref())
            .unwrap_or("__legacy__")
            .to_string()
    }

    fn memory_adjustment(
        &self,
        terms: &[String],
        path: &str,
        context: Option<&CoordinationContext>,
    ) -> f64 {
        let Ok(memory) = self.session_memory.lock() else {
            return 0.0;
        };
        let session_key = Self::coordination_session_key(context);
        let agent_id = context.and_then(|context| context.agent_id.as_deref());
        let session_scoped = context.is_some_and(|context| context.session_id.is_some());
        let agent_only = context
            .is_some_and(|context| context.session_id.is_none() && context.agent_id.is_some());
        let mut total = 0.0;
        let mut age = 0usize;
        for record in memory.iter().rev() {
            if record.session_id != session_key {
                continue;
            }
            if age >= 24 {
                break;
            }
            if !record.paths.iter().any(|saved| saved == path) {
                age = age.saturating_add(1);
                continue;
            }
            let overlap = terms
                .iter()
                .filter(|term| {
                    record
                        .terms
                        .iter()
                        .any(|saved| saved.as_str() == term.as_str())
                })
                .count();
            if overlap > 0 {
                let decay = 1.0 + age as f64 * 0.2;
                if session_scoped && record.agent_id.as_deref() != agent_id {
                    // Sibling agents should preferentially expose complementary evidence. Keep
                    // this penalty modest so strong lexical/semantic evidence still wins.
                    total -= (overlap as f64 * 0.9) / decay;
                } else if agent_only && record.agent_id.as_deref() != agent_id {
                    // An agent_id without a session_id gets continuity only for itself; it must not
                    // accidentally turn unrelated agent histories into positive reinforcement.
                    continue;
                } else {
                    // Same-agent continuity remains useful for follow-up queries.
                    total += (overlap as f64 * 0.55) / decay;
                }
            }
            age = age.saturating_add(1);
        }
        total.clamp(-4.0, 4.0)
    }

    fn remember_search(
        &self,
        query: &NormalizedQuery,
        hits: &[SearchHit],
        context: Option<&CoordinationContext>,
    ) {
        let Ok(mut memory) = self.session_memory.lock() else {
            return;
        };
        let paths = hits
            .iter()
            .take(8)
            .map(|hit| hit.relative_path.clone())
            .collect::<Vec<_>>();
        if paths.is_empty() {
            return;
        }
        memory.push_back(SearchMemory {
            session_id: Self::coordination_session_key(context),
            agent_id: context.and_then(|context| context.agent_id.clone()),
            terms: query.terms.clone(),
            paths,
        });
        while memory.len() > MAX_SESSION_MEMORY_RECORDS {
            memory.pop_front();
        }
    }

    fn analyze_source_cached(
        &self,
        path: &str,
        safe_source: &str,
        stamp: &SourceStamp,
        cancellation: Option<&AtomicBool>,
        deadline: Instant,
    ) -> Result<Option<CachedAnalysis>, RepositoryAccessError> {
        loop {
            if is_cancelled(cancellation) {
                return Err(RepositoryAccessError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            let mut state = self
                .analysis_cache
                .lock()
                .map_err(|_| RepositoryAccessError::Io)?;
            state.tick = state.tick.saturating_add(1);
            let tick = state.tick;
            if state
                .entries
                .get(path)
                .is_some_and(|entry| &entry.stamp == stamp)
            {
                if let Some(entry) = state.entries.get_mut(path) {
                    entry.last_used = tick;
                    return Ok(Some(entry.clone()));
                }
            }
            if state.entries.contains_key(path) {
                state.entries.remove(path);
            }
            if state.inflight.contains(path) {
                let (guard, _) = self
                    .analysis_ready
                    .wait_timeout(state, Duration::from_millis(20))
                    .map_err(|_| RepositoryAccessError::Io)?;
                drop(guard);
                continue;
            }
            state.inflight.insert(path.to_string());
            break;
        }
        let _analysis_flight = AnalysisFlightGuard::new(self, path.to_string());

        let computed = (|| {
            let syntax_supported = supports_tree_sitter_path(path);
            let ast_symbols =
                extract_ast_symbols_bounded(path, safe_source, 64, cancellation, Some(deadline));
            let ast_cacheable = ast_symbols.is_some() || !syntax_supported;
            let mut symbols = ast_symbols
                .unwrap_or_default()
                .into_iter()
                .map(|symbol| CachedRepoMapSymbol {
                    name: symbol.name,
                    kind: symbol.kind,
                    line: symbol.line,
                })
                .collect::<Vec<_>>();
            if is_cancelled(cancellation) {
                return Err(RepositoryAccessError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            let mut seen = symbols
                .iter()
                .map(|symbol| symbol.name.clone())
                .collect::<HashSet<_>>();
            for symbol in extract_symbols(safe_source, 64) {
                if symbols.len() >= 64 {
                    break;
                }
                if seen.insert(symbol.name.clone()) {
                    symbols.push(CachedRepoMapSymbol {
                        name: symbol.name,
                        kind: symbol.kind,
                        line: symbol.line,
                    });
                }
            }
            let semantic_facts = extract_semantic_facts_bounded(
                path,
                safe_source,
                512,
                64,
                cancellation,
                Some(deadline),
            );
            let semantic_cacheable = semantic_facts.is_some() || !syntax_supported;
            let semantics = semantic_facts.unwrap_or_default();
            if is_cancelled(cancellation) {
                return Err(RepositoryAccessError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            Ok(Some(CachedAnalysis {
                stamp: stamp.clone(),
                symbols,
                semantics,
                cacheable: ast_cacheable && semantic_cacheable,
                last_used: 0,
            }))
        })();

        let mut state = self
            .analysis_cache
            .lock()
            .map_err(|_| RepositoryAccessError::Io)?;
        if let Ok(Some(mut entry)) = computed.clone() {
            if entry.cacheable {
                state.tick = state.tick.saturating_add(1);
                entry.last_used = state.tick;
                if state.entries.len() >= MAX_ANALYSIS_CACHE_FILES
                    && !state.entries.contains_key(path)
                {
                    if let Some(evict) = state
                        .entries
                        .iter()
                        .min_by_key(|(_, cached)| cached.last_used)
                        .map(|(path, _)| path.clone())
                    {
                        state.entries.remove(&evict);
                    }
                }
                state.entries.insert(path.to_string(), entry);
            }
        }
        drop(state);
        computed
    }

    fn graph_cache_get(
        &self,
        key: &GraphCacheKey,
    ) -> Result<Option<CachedGraph>, RepositoryAccessError> {
        let mut cache = self
            .graph_cache
            .lock()
            .map_err(|_| RepositoryAccessError::Io)?;
        cache.tick = cache.tick.saturating_add(1);
        let tick = cache.tick;
        if let Some(entry) = cache.entries.get_mut(key) {
            entry.last_used = tick;
            return Ok(Some(entry.clone()));
        }
        Ok(None)
    }

    fn graph_cache_put(
        &self,
        key: GraphCacheKey,
        mut graph: CachedGraph,
    ) -> Result<(), RepositoryAccessError> {
        let mut cache = self
            .graph_cache
            .lock()
            .map_err(|_| RepositoryAccessError::Io)?;
        cache.tick = cache.tick.saturating_add(1);
        graph.last_used = cache.tick;
        if cache.entries.len() >= MAX_GRAPH_CACHE_ENTRIES && !cache.entries.contains_key(&key) {
            if let Some(evict) = cache
                .entries
                .iter()
                .min_by_key(|(_, cached)| cached.last_used)
                .map(|(key, _)| key.clone())
            {
                cache.entries.remove(&evict);
            }
        }
        cache.entries.insert(key, graph);
        Ok(())
    }

    /// Builds a query-focused structural graph from an already-ranked bounded candidate set.
    /// This avoids a second repository-wide search when `repo_context` needs both retrieval and
    /// structural evidence in one MCP call.
    #[cfg(test)]
    pub fn map_from_hits(
        &self,
        query: &NormalizedQuery,
        hits: &[SearchHit],
        max_files: usize,
        cancellation: Option<&AtomicBool>,
    ) -> Result<RepositoryMapOutcome, RepositoryAccessError> {
        let started = Instant::now();
        self.map_from_hits_since(query, hits, max_files, cancellation, &started)
    }

    pub fn map_from_hits_since(
        &self,
        query: &NormalizedQuery,
        hits: &[SearchHit],
        max_files: usize,
        cancellation: Option<&AtomicBool>,
        started: &Instant,
    ) -> Result<RepositoryMapOutcome, RepositoryAccessError> {
        // Windows' stable metadata surface at this MSRV cannot distinguish every same-size,
        // same-mtime replacement. The verified open-handle stamp still protects each individual
        // read from concurrent mutation, but it is not a safe cross-request content identity.
        // Serialize top-level map construction and discard prior structural caches before reading
        // candidates so stale symbols, semantic facts, or graph edges cannot cross requests.
        #[cfg(windows)]
        let _windows_map_guard = self
            .windows_map_serial
            .lock()
            .map_err(|_| RepositoryAccessError::Io)?;
        #[cfg(windows)]
        self.reset_structural_caches()?;

        let mut truncated = false;
        let mut candidates = Vec::<MapCandidate>::new();
        let mut map_source_bytes = 0usize;
        let mut map_redacted_bytes = 0usize;
        let mut invalidated_evidence_paths = Vec::<String>::new();
        let structural_limit = max_files.min(16);
        let mut structural_collection_enabled = true;

        // Revalidate every returned search hit before any excerpt is rendered. This is especially
        // important on Windows: size + mtime can be preserved across a same-length rewrite, so an
        // adaptive-round verification cache can otherwise carry a stale excerpt into the final
        // context. Structural analysis remains limited to `structural_limit`; lower-ranked hits are
        // read only for generation/fingerprint validation and are not retained as source bodies.
        for (hit_index, hit) in hits.iter().enumerate() {
            if is_cancelled(cancellation) {
                return Err(RepositoryAccessError::Cancelled);
            }
            if search_timed_out(started) {
                truncated = true;
                invalidated_evidence_paths.extend(
                    hits[hit_index..]
                        .iter()
                        .map(|remaining| remaining.relative_path.clone()),
                );
                break;
            }
            let source = match self.read_source(&hit.relative_path) {
                Ok(source) => source,
                Err((_error, _)) => {
                    // If the current file cannot be re-opened and re-verified, the previously
                    // collected excerpt is no longer safe to present as current evidence.
                    invalidated_evidence_paths.push(hit.relative_path.clone());
                    truncated = true;
                    continue;
                }
            };
            if hit
                .source_stamp
                .as_ref()
                .is_some_and(|expected| expected != &source.stamp)
            {
                // Evidence and structure must describe one file generation. A changed candidate is
                // omitted rather than mixing stale evidence with fresh structural analysis.
                truncated = true;
                invalidated_evidence_paths.push(hit.relative_path.clone());
                continue;
            }
            if hit
                .source_fingerprint
                .is_some_and(|expected| expected != source_content_fingerprint(&source.text))
            {
                // SourceStamp is intentionally conservative but Windows can preserve size + mtime
                // across a rewrite. The content fingerprint closes that within-call consistency gap.
                truncated = true;
                invalidated_evidence_paths.push(hit.relative_path.clone());
                continue;
            }

            // Evidence for this hit is current. Lower-ranked hits need no structural work, and once
            // the structural budget/deadline is exhausted we keep validating evidence without
            // retaining or analyzing additional source bodies.
            if hit_index >= structural_limit || !structural_collection_enabled {
                continue;
            }
            map_source_bytes = map_source_bytes.saturating_add(source.source_bytes);
            if map_source_bytes > MAX_REPOSITORY_MAP_SOURCE_BYTES {
                truncated = true;
                structural_collection_enabled = false;
                continue;
            }

            // Redaction markers can be longer than the secret they replace (for example
            // `token="x"`). Bound the redacted representation before analysis so a crafted
            // repository cannot turn the 32 MiB raw-source budget into hundreds of MiB of
            // retained `source_lower` buffers. The bounded redactor also suppresses giant single
            // lines before any allocating per-line redactor sees them.
            let redaction = redact_high_confidence_secrets_bounded(&source.text, MAX_SOURCE_BYTES);
            if redaction.truncated {
                truncated = true;
            }
            map_redacted_bytes = map_redacted_bytes.saturating_add(redaction.text.len());
            if map_redacted_bytes > MAX_REPOSITORY_MAP_SOURCE_BYTES {
                truncated = true;
                structural_collection_enabled = false;
                continue;
            }
            let mut safe = redaction.text;
            let Some(analysis) = self.analyze_source_cached(
                &hit.relative_path,
                &safe,
                &source.stamp,
                cancellation,
                *started + MAX_SEARCH_WALL_TIME,
            )?
            else {
                truncated = true;
                structural_collection_enabled = false;
                continue;
            };
            let mut definition_names = analysis
                .symbols
                .iter()
                .map(|symbol| symbol.name.to_ascii_lowercase())
                .filter(|name| name.len() >= 2)
                .collect::<Vec<_>>();
            definition_names.sort();
            definition_names.dedup();

            // Shared analysis caches contain structural metadata only. Rehydrate display/ranking
            // signatures from the freshly verified, redacted source for this call so source-line
            // text never persists in the cross-agent cache.
            let safe_lines = safe.lines().collect::<Vec<_>>();
            let mut symbols = analysis
                .symbols
                .iter()
                .map(|symbol| RepoMapSymbol {
                    name: symbol.name.clone(),
                    kind: symbol.kind.clone(),
                    line: symbol.line,
                    signature: signature_from_lines(&safe_lines, symbol.line),
                })
                .collect::<Vec<_>>();
            symbols.sort_by(|a, b| {
                let score = |symbol: &RepoMapSymbol| {
                    let name = symbol.name.to_ascii_lowercase();
                    let signature = symbol.signature.to_ascii_lowercase();
                    query
                        .terms
                        .iter()
                        .map(|term| {
                            if name.as_str() == term.as_str() {
                                6usize
                            } else if name.contains(term.as_str()) {
                                4usize
                            } else if signature.contains(term.as_str()) {
                                2usize
                            } else {
                                0usize
                            }
                        })
                        .sum::<usize>()
                };
                score(b)
                    .cmp(&score(a))
                    .then_with(|| a.line.cmp(&b.line))
                    .then_with(|| a.name.cmp(&b.name))
            });
            symbols.truncate(12);

            // Tier 2 is source-only semantic analysis. The parsed facts are shared across agents
            // only while the verified source stamp remains unchanged.
            let semantics = analysis.semantics.clone();
            let semantic_query_bonus = semantics
                .references
                .iter()
                .map(|reference| {
                    let name = reference.name.to_ascii_lowercase();
                    let overlap = query
                        .terms
                        .iter()
                        .filter(|term| name.contains(term.as_str()))
                        .count() as f64;
                    let weight = match reference.kind.as_str() {
                        "implementation" => 1.0,
                        "call" => 0.9,
                        "type" => 0.85,
                        _ => 0.6,
                    };
                    overlap * weight
                })
                .sum::<f64>()
                .min(8.0);

            // `safe` is no longer needed with original casing. Lowercase it in place instead of
            // allocating a second file-sized String; the retained map representation therefore
            // stays within the same redacted-byte budget.
            drop(safe_lines);
            safe.make_ascii_lowercase();

            candidates.push(MapCandidate {
                relative_path: hit.relative_path.clone(),
                stamp: source.stamp,
                search_score: hit.score,
                source_lower: safe,
                symbols,
                definition_names,
                semantics,
                analysis_cacheable: analysis.cacheable,
                semantic_query_bonus,
            });
        }

        // Canonicalize candidate order so sibling agents that discover the same file set in a
        // different ranking order can share the exact same structural graph cache entry.
        candidates.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

        let graph_key = GraphCacheKey(
            candidates
                .iter()
                .map(|candidate| GraphCacheNode {
                    path: candidate.relative_path.clone(),
                    stamp: candidate.stamp.clone(),
                })
                .collect(),
        );
        let graph_cacheable = !candidates.is_empty()
            && candidates
                .iter()
                .all(|candidate| candidate.analysis_cacheable);
        let cached_graph = if graph_cacheable {
            self.graph_cache_get(&graph_key)?
        } else {
            None
        };
        let (edge_maps, centrality) = if let Some(cached) = cached_graph {
            (cached.edge_maps, cached.centrality)
        } else {
            let mut definition_targets = HashMap::<String, Vec<usize>>::new();
            for (to, candidate) in candidates.iter().enumerate() {
                for name in &candidate.definition_names {
                    definition_targets.entry(name.clone()).or_default().push(to);
                }
            }

            // Strongest evidence wins for each file pair: implementation .95, call .90, type .85,
            // exact reference .80, import .40, lexical coincidence .15.
            let mut edge_maps = vec![HashMap::<usize, (f64, String)>::new(); candidates.len()];
            for (from, candidate) in candidates.iter().enumerate() {
                if is_cancelled(cancellation) {
                    return Err(RepositoryAccessError::Cancelled);
                }
                if search_timed_out(started) {
                    truncated = true;
                    break;
                }
                for reference in &candidate.semantics.references {
                    let key = reference.name.to_ascii_lowercase();
                    let Some(targets) = definition_targets.get(&key) else {
                        continue;
                    };
                    let weight = match reference.kind.as_str() {
                        "implementation" => 0.95,
                        "call" => 0.90,
                        "type" => 0.85,
                        _ => 0.80,
                    };
                    for &to in targets {
                        upsert_repo_edge(&mut edge_maps, from, to, weight, reference.kind.as_str());
                    }
                }

                for import_path in &candidate.semantics.import_paths {
                    let import_lower = import_path.to_ascii_lowercase();
                    for (to, target) in candidates.iter().enumerate() {
                        if to == from {
                            continue;
                        }
                        let path = target.relative_path.to_ascii_lowercase();
                        let stem = Path::new(&path)
                            .file_stem()
                            .and_then(|value| value.to_str())
                            .unwrap_or("");
                        let path_no_ext = Path::new(&path)
                            .with_extension("")
                            .to_string_lossy()
                            .replace('\\', "/");
                        let module_path = path_no_ext.trim_end_matches("/mod");
                        if (!stem.is_empty() && import_lower.ends_with(stem))
                            || (!module_path.is_empty()
                                && (import_lower.ends_with(module_path)
                                    || module_path.ends_with(import_lower.as_str())))
                        {
                            upsert_repo_edge(&mut edge_maps, from, to, 0.40, "import");
                        }
                    }
                }
            }

            // Preserve RC25's Aho-Corasick structural hint as a weak fallback only.
            let mut patterns = Vec::<String>::new();
            let mut pattern_targets = Vec::<Vec<usize>>::new();
            let mut pattern_ids = HashMap::<String, usize>::new();
            for (to, candidate) in candidates.iter().enumerate() {
                for symbol_lower in &candidate.definition_names {
                    if symbol_lower.len() < 4 {
                        continue;
                    }
                    if let Some(&pattern_id) = pattern_ids.get(symbol_lower) {
                        if !pattern_targets[pattern_id].contains(&to) {
                            pattern_targets[pattern_id].push(to);
                        }
                    } else {
                        let pattern_id = patterns.len();
                        pattern_ids.insert(symbol_lower.clone(), pattern_id);
                        patterns.push(symbol_lower.clone());
                        pattern_targets.push(vec![to]);
                    }
                }
            }
            if !truncated && !patterns.is_empty() {
                match AhoCorasick::new(patterns.iter().map(String::as_str)) {
                    Ok(matcher) => {
                        for (from, candidate) in candidates.iter().enumerate() {
                            if is_cancelled(cancellation) {
                                return Err(RepositoryAccessError::Cancelled);
                            }
                            if search_timed_out(started) {
                                truncated = true;
                                break;
                            }
                            for (match_count, found) in matcher
                                .find_overlapping_iter(candidate.source_lower.as_bytes())
                                .enumerate()
                            {
                                if match_count % 1024 == 0 {
                                    if is_cancelled(cancellation) {
                                        return Err(RepositoryAccessError::Cancelled);
                                    }
                                    if search_timed_out(started) {
                                        truncated = true;
                                        break;
                                    }
                                }
                                for &to in &pattern_targets[found.pattern().as_usize()] {
                                    upsert_repo_edge(&mut edge_maps, from, to, 0.15, "lexical");
                                }
                            }
                            if truncated {
                                break;
                            }
                        }
                    }
                    Err(_) => truncated = true,
                }
            }

            let weighted_edges = edge_maps
                .iter()
                .map(|targets| {
                    let mut edges = targets
                        .iter()
                        .map(|(to, (weight, _))| (*to, *weight))
                        .collect::<Vec<_>>();
                    edges.sort_by_key(|(to, _)| *to);
                    edges
                })
                .collect::<Vec<Vec<(usize, f64)>>>();
            let centrality = weighted_pagerank(&weighted_edges, 12);
            if truncated || !graph_cacheable {
                (edge_maps, centrality)
            } else {
                self.graph_cache_put(
                    graph_key,
                    CachedGraph {
                        edge_maps: edge_maps.clone(),
                        centrality: centrality.clone(),
                        last_used: 0,
                    },
                )?;
                (edge_maps, centrality)
            }
        };
        let candidate_paths = candidates
            .iter()
            .map(|candidate| candidate.relative_path.clone())
            .collect::<Vec<_>>();
        let mut entries = candidates
            .into_iter()
            .enumerate()
            .map(|(index, candidate)| {
                let mut semantic_links = edge_maps[index]
                    .iter()
                    .filter_map(|(target, (weight, kind))| {
                        candidate_paths.get(*target).map(|path| RepoMapLink {
                            relative_path: path.clone(),
                            kind: kind.clone(),
                            weight: *weight,
                        })
                    })
                    .collect::<Vec<_>>();
                semantic_links.sort_by(|a, b| {
                    b.weight
                        .partial_cmp(&a.weight)
                        .unwrap_or(Ordering::Equal)
                        .then_with(|| a.relative_path.cmp(&b.relative_path))
                });
                semantic_links.truncate(6);
                let links_to = semantic_links
                    .iter()
                    .map(|link| link.relative_path.clone())
                    .collect();
                RepoMapEntry {
                    relative_path: candidate.relative_path,
                    score: candidate.search_score
                        + candidate.semantic_query_bonus * 3.0
                        + centrality.get(index).copied().unwrap_or(0.0) * 50.0,
                    symbols: candidate.symbols,
                    links_to,
                    semantic_links,
                }
            })
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.relative_path.cmp(&b.relative_path))
        });
        entries.truncate(max_files.min(16));
        invalidated_evidence_paths.sort();
        invalidated_evidence_paths.dedup();
        Ok(RepositoryMapOutcome {
            entries,
            truncated,
            invalidated_evidence_paths,
        })
    }
}

fn upsert_repo_edge(
    edges: &mut [HashMap<usize, (f64, String)>],
    from: usize,
    to: usize,
    weight: f64,
    kind: &str,
) {
    if from == to
        || from >= edges.len()
        || to >= edges.len()
        || !weight.is_finite()
        || weight <= 0.0
    {
        return;
    }
    let slot = edges[from]
        .entry(to)
        .or_insert_with(|| (weight, kind.to_string()));
    if weight > slot.0 {
        *slot = (weight, kind.to_string());
    }
}

fn signature_from_lines(lines: &[&str], line: u32) -> String {
    let Some(index) = line.checked_sub(1).map(|value| value as usize) else {
        return String::new();
    };
    lines
        .get(index)
        .map(|value| value.trim_start().chars().take(220).collect::<String>())
        .unwrap_or_default()
}

fn search_confidence(query: &NormalizedQuery, outcome: &SearchOutcome) -> f64 {
    if outcome.hits.is_empty() {
        // A policy-excluded file is intentionally not adaptive-scan-expandable, but it still
        // prevents a repository-wide absence claim because its contents were never inspected.
        return if outcome.truncated {
            0.05
        } else if outcome.coverage.policy_excluded_files > 0 {
            0.35
        } else {
            0.98
        };
    }

    let top = outcome.hits.iter().take(3).collect::<Vec<_>>();
    let mut covered = HashSet::<&str>::new();
    for hit in &top {
        let path = hit.relative_path.to_ascii_lowercase();
        let excerpt = hit.excerpt.to_ascii_lowercase();
        for term in &query.terms {
            if path.contains(term.as_str()) || excerpt.contains(term.as_str()) {
                covered.insert(term.as_str());
            }
        }
    }
    let query_coverage = covered.len() as f64 / query.terms.len().max(1) as f64;
    let top_score = top.first().map_or(0.0, |hit| hit.score.max(0.0));
    let second_score = top.get(1).map_or(0.0, |hit| hit.score.max(0.0));
    let margin = if top_score <= f64::EPSILON {
        0.0
    } else {
        ((top_score - second_score).max(0.0) / top_score).min(1.0)
    };
    let evidence_depth = (outcome.hits.len().min(6) as f64 / 6.0).min(1.0);
    let index_coverage = if outcome.coverage.eligible_files == 0 {
        1.0
    } else {
        outcome.coverage.indexed_files as f64 / outcome.coverage.eligible_files as f64
    };
    let completion_bonus = if outcome.truncated || outcome.coverage.policy_excluded_files > 0 {
        0.0
    } else {
        0.08
    };
    (query_coverage * 0.48
        + margin * 0.20
        + evidence_depth * 0.12
        + index_coverage * 0.20
        + completion_bonus)
        .clamp(0.0, 1.0)
}

fn root_identity_from_dir(directory: &Dir) -> Result<RootIdentity, RepositoryAccessError> {
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

fn source_stamp(metadata: &std::fs::Metadata) -> SourceStamp {
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
    #[cfg(not(unix))]
    {
        SourceStamp {
            len: metadata.len(),
            modified_nanos,
        }
    }
}

fn cap_source_stamp(metadata: &cap_std::fs::Metadata) -> SourceStamp {
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
    #[cfg(not(unix))]
    {
        SourceStamp {
            len: metadata.len(),
            modified_nanos,
        }
    }
}

#[cfg(unix)]
fn metadata_has_multiple_hard_links(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.nlink() > 1
}

#[cfg(not(unix))]
fn metadata_has_multiple_hard_links(_metadata: &std::fs::Metadata) -> bool {
    // Directory-walk metadata does not expose a portable hard-link count. Windows performs the
    // authoritative check on the already-open file handle before and after reading instead.
    false
}

#[cfg(unix)]
fn file_has_multiple_hard_links(
    _file: &cap_std::fs::File,
    metadata: &cap_std::fs::Metadata,
) -> Result<bool, RepositoryAccessError> {
    use cap_std::fs::MetadataExt;
    Ok(metadata.nlink() > 1)
}

#[cfg(windows)]
fn file_has_multiple_hard_links(
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
fn file_has_multiple_hard_links(
    _file: &cap_std::fs::File,
    _metadata: &cap_std::fs::Metadata,
) -> Result<bool, RepositoryAccessError> {
    // No portable stable link-count API exists on the remaining targets. Keep the trusted-root
    // requirement there rather than making every regular file unreadable.
    Ok(false)
}

fn policy_excluded_by_metadata(metadata: &std::fs::Metadata) -> bool {
    metadata.len() > MAX_SOURCE_BYTES as u64 || metadata_has_multiple_hard_links(metadata)
}

fn stable_term_hash(text: &str) -> u64 {
    // Stable FNV-1a keeps the RAM index compact and avoids retaining repository tokens verbatim.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(byte.to_ascii_lowercase());
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn identifier_fragments(identifier: &str) -> Vec<String> {
    let mut fragments = Vec::new();
    for coarse in identifier.split(['_', '-']) {
        if coarse.len() < 2 {
            continue;
        }
        let chars = coarse.chars().collect::<Vec<_>>();
        if chars.is_empty() {
            continue;
        }
        let mut start = 0usize;
        for index in 1..chars.len() {
            let previous = chars[index - 1];
            let current = chars[index];
            let next = chars.get(index + 1).copied();
            let camel_boundary = current.is_ascii_uppercase()
                && (previous.is_ascii_lowercase()
                    || previous.is_ascii_digit()
                    || (previous.is_ascii_uppercase()
                        && next.is_some_and(|value| value.is_ascii_lowercase())));
            if camel_boundary {
                let fragment = chars[start..index].iter().collect::<String>();
                if fragment.len() >= 2 {
                    fragments.push(fragment.to_ascii_lowercase());
                }
                start = index;
            }
        }
        let fragment = chars[start..].iter().collect::<String>();
        if fragment.len() >= 2 {
            fragments.push(fragment.to_ascii_lowercase());
        }
    }
    fragments
}

fn substring_gram_key(window: &[u8]) -> u32 {
    let mut key = (window.len() as u32) << 24;
    for (index, byte) in window.iter().enumerate() {
        let shift = 16usize.saturating_sub(index * 8);
        key |= u32::from(byte.to_ascii_lowercase()) << shift;
    }
    key
}

fn unicode_scalar_gram_key(ch: char) -> u32 {
    // ASCII substring keys use top-byte namespaces 2 and 3. Reserve the high bit for Unicode
    // scalar sketches so the key families cannot collide structurally.
    let mut hash = 0x811c9dc5u32;
    let mut encoded = [0u8; 4];
    for byte in ch.encode_utf8(&mut encoded).as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    0x8000_0000 | (hash & 0x7fff_ffff)
}

fn query_substring_grams(term: &str) -> Vec<u32> {
    if term.is_ascii() {
        if term.len() < 2 {
            return Vec::new();
        }
        let lower = term.to_ascii_lowercase();
        let bytes = lower.as_bytes();
        let width = if bytes.len() == 2 { 2 } else { 3 };
        let mut grams = bytes
            .windows(width)
            .map(substring_gram_key)
            .collect::<Vec<_>>();
        grams.sort_unstable();
        grams.dedup();
        return grams;
    }

    // Query normalization folds ASCII case only, so non-ASCII scalar values remain exact here too.
    // Requiring every scalar preserves candidate recall for Unicode substrings; source verification
    // still removes hash/order false positives before any evidence becomes model-visible.
    let mut grams = term
        .chars()
        .map(unicode_scalar_gram_key)
        .collect::<Vec<_>>();
    grams.sort_unstable();
    grams.dedup();
    grams
}

fn add_index_term(counts: &mut HashMap<u64, u16>, term: &str, term_truncated: &mut bool) {
    if term.len() < 2 {
        return;
    }
    let hash = stable_term_hash(term);
    if let Some(value) = counts.get_mut(&hash) {
        *value = value.saturating_add(1);
    } else if counts.len() < MAX_INDEX_UNIQUE_TERMS_PER_FILE {
        counts.insert(hash, 1);
    } else {
        *term_truncated = true;
    }
}

fn build_indexed_document(text: &str, stamp: Option<SourceStamp>) -> IndexedDocument {
    let mut counts = HashMap::<u64, u16>::new();
    let mut substring_grams = HashSet::<u32>::new();
    let mut document_len = 0usize;
    let mut term_truncated = false;

    for part in text
        .split(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '-'))
        .filter(|part| part.len() >= 2)
    {
        document_len = document_len.saturating_add(1);
        let lower = part.to_ascii_lowercase();
        add_index_term(&mut counts, &lower, &mut term_truncated);
        for fragment in identifier_fragments(part) {
            add_index_term(&mut counts, &fragment, &mut term_truncated);
        }

        // Candidate sketches never retain source bodies or plaintext tokens. ASCII keeps compact
        // two/three-byte grams. Tokens containing Unicode additionally get hashed scalar sketches,
        // so queries such as "認証" can nominate "ユーザー認証処理" for exact source verification.
        // ASCII runs inside mixed identifiers also keep ordinary substring recall.
        for ascii_run in lower.split(|ch: char| !ch.is_ascii()) {
            let bytes = ascii_run.as_bytes();
            for width in [2usize, 3usize] {
                if bytes.len() < width {
                    continue;
                }
                for window in bytes.windows(width) {
                    if substring_grams.len() >= MAX_INDEX_SUBSTRING_GRAMS_PER_FILE {
                        term_truncated = true;
                        break;
                    }
                    substring_grams.insert(substring_gram_key(window));
                }
            }
        }
        if !lower.is_ascii() {
            for ch in lower.chars() {
                if substring_grams.len() >= MAX_INDEX_SUBSTRING_GRAMS_PER_FILE {
                    term_truncated = true;
                    break;
                }
                substring_grams.insert(unicode_scalar_gram_key(ch));
            }
        }
    }

    let mut terms = counts.into_iter().collect::<Vec<_>>();
    terms.sort_unstable_by_key(|(hash, _)| *hash);
    let mut substring_grams = substring_grams.into_iter().collect::<Vec<_>>();
    substring_grams.sort_unstable();
    IndexedDocument {
        stamp,
        document_len: document_len.max(1),
        terms,
        substring_grams,
        term_truncated,
    }
}

fn indexed_query_frequencies(
    document: &IndexedDocument,
    query_terms: &[(u64, Vec<u32>)],
) -> Vec<usize> {
    query_terms
        .iter()
        .map(|(hash, grams)| {
            let exact = document
                .terms
                .binary_search_by_key(hash, |(term_hash, _)| *term_hash)
                .ok()
                .and_then(|index| document.terms.get(index))
                .map(|(_, count)| usize::from(*count))
                .unwrap_or(0);
            if exact > 0 {
                return exact;
            }
            if !grams.is_empty()
                && grams
                    .iter()
                    .all(|gram| document.substring_grams.binary_search(gram).is_ok())
            {
                // This is deliberately a one-hit fallback, matching the pre-index substring
                // behavior. False positives are removed by bounded source verification.
                1
            } else {
                0
            }
        })
        .collect()
}

fn stratified_pending_lanes(
    pending: Vec<PendingFile>,
) -> (Vec<PendingFile>, Vec<PendingFile>, Vec<PendingFile>) {
    let mut priority = Vec::new();
    let mut ordinary = Vec::new();
    for file in pending {
        if file.changed || file.path_bonus > 0 {
            priority.push(file);
        } else {
            ordinary.push(file);
        }
    }
    priority.sort_by(|a, b| {
        b.changed
            .cmp(&a.changed)
            .then_with(|| b.path_bonus.cmp(&a.path_bonus))
            .then_with(|| a.file.path.cmp(&b.file.path))
    });

    // Roughly one eighth of ordinary files form a deterministic cross-repository sample. Files
    // already consumed by this lane are skipped when the round-robin broad lane is reached.
    let mut sample = ordinary
        .iter()
        .filter(|file| stable_term_hash(&file.file.path) % 8 == 0)
        .cloned()
        .collect::<Vec<_>>();
    sample.sort_by_key(|file| stable_term_hash(&file.file.path));

    let mut buckets = BTreeMap::<String, VecDeque<PendingFile>>::new();
    ordinary.sort_by(|a, b| a.file.path.cmp(&b.file.path));
    for file in ordinary {
        let bucket = file.file.path.split('/').next().unwrap_or("").to_string();
        buckets.entry(bucket).or_default().push_back(file);
    }
    let mut broad = Vec::new();
    loop {
        let mut progressed = false;
        for bucket in buckets.values_mut() {
            if let Some(file) = bucket.pop_front() {
                broad.push(file);
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    (priority, sample, broad)
}

fn is_cancelled(cancellation: Option<&AtomicBool>) -> bool {
    cancellation.is_some_and(|flag| flag.load(AtomicOrdering::Relaxed))
}

fn search_timed_out(started: &Instant) -> bool {
    started.elapsed() >= MAX_SEARCH_WALL_TIME
}

fn first_term_match_byte(lower_line: &str, terms: &[String]) -> Option<usize> {
    terms
        .iter()
        .filter_map(|term| lower_line.find(term.as_str()))
        .min()
}

fn bounded_search_excerpt(
    lines: &[&str],
    focus: usize,
    focus_match_byte: usize,
) -> (String, usize, usize) {
    let half = DEFAULT_CONTEXT_LINES / 2;
    let mut start = focus.saturating_sub(half);
    let mut end = (focus + half + 1).min(lines.len());

    loop {
        let joined = lines[start..end].join("\n");
        if joined.len() <= MAX_SEARCH_EXCERPT_BYTES {
            return (joined, start, end);
        }

        // Remove the larger outer neighbor first while always retaining the matched line. This
        // prevents a huge adjacent line from consuming the excerpt and hiding the actual match.
        if start < focus || end > focus + 1 {
            let left_bytes = if start < focus {
                lines[start].len().saturating_add(1)
            } else {
                0
            };
            let right_bytes = if end > focus + 1 {
                lines[end - 1].len().saturating_add(1)
            } else {
                0
            };
            if start < focus && (end <= focus + 1 || left_bytes >= right_bytes) {
                start += 1;
            } else if end > focus + 1 {
                end -= 1;
            }
            continue;
        }

        return (
            bounded_focus_line(lines[focus], focus_match_byte),
            focus,
            focus + 1,
        );
    }
}

fn bounded_focus_line(line: &str, match_byte: usize) -> String {
    const PREFIX: &str = "[SIPPION_EXCERPT_TRUNCATED] ";
    const SUFFIX: &str = " [SIPPION_EXCERPT_TRUNCATED]";
    if line.len() <= MAX_SEARCH_EXCERPT_BYTES {
        return line.to_string();
    }

    let payload_budget = MAX_SEARCH_EXCERPT_BYTES
        .saturating_sub(PREFIX.len())
        .saturating_sub(SUFFIX.len());
    let mut start = match_byte.saturating_sub(payload_budget / 2);
    while start < line.len() && !line.is_char_boundary(start) {
        start += 1;
    }
    let mut end = start.saturating_add(payload_budget).min(line.len());
    while end > start && !line.is_char_boundary(end) {
        end -= 1;
    }

    // If clamping at EOF left unused budget, shift left without ever exceeding the byte budget.
    if end == line.len() && end.saturating_sub(start) < payload_budget {
        start = end.saturating_sub(payload_budget);
        while start < end && !line.is_char_boundary(start) {
            start += 1;
        }
    }

    let mut bounded = String::with_capacity(MAX_SEARCH_EXCERPT_BYTES);
    if start > 0 {
        bounded.push_str(PREFIX);
    }
    bounded.push_str(&line[start..end]);
    if end < line.len() {
        bounded.push_str(SUFFIX);
    }
    debug_assert!(bounded.len() <= MAX_SEARCH_EXCERPT_BYTES);
    bounded
}

fn source_content_fingerprint(text: &str) -> (u64, u64) {
    // Two independently seeded FNV-1a lanes provide a compact, allocation-free content identity.
    // This is a consistency guard, not an authentication primitive.
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut left = 0xcbf2_9ce4_8422_2325u64;
    let mut right = 0x8422_2325_cbf2_9ce4u64;
    for &byte in text.as_bytes() {
        left ^= u64::from(byte);
        left = left.wrapping_mul(FNV_PRIME);
        right ^= u64::from(byte).wrapping_add(0x9d);
        right = right.wrapping_mul(FNV_PRIME);
    }
    (left, right)
}

fn sort_hits(hits: &mut [SearchHit]) {
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.relative_path.cmp(&b.relative_path))
            .then_with(|| a.start_line.cmp(&b.start_line))
    });
}

fn path_match_score(path: &str, terms: &[String]) -> usize {
    let path_lower = path.to_ascii_lowercase();
    terms
        .iter()
        .filter(|term| path_lower.contains(term.as_str()))
        .count()
}

fn hit_is_better(candidate: &SearchHit, current: &SearchHit) -> bool {
    candidate
        .score
        .partial_cmp(&current.score)
        .unwrap_or(Ordering::Equal)
        .then_with(|| current.start_line.cmp(&candidate.start_line))
        == Ordering::Greater
}

#[cfg(test)]
fn prune_candidates_if_needed(hits: &mut Vec<SearchHit>, candidate_limit: usize) {
    if hits.len() < candidate_limit.saturating_mul(2) {
        return;
    }
    sort_hits(hits);
    if hits.len() > candidate_limit {
        hits.truncate(candidate_limit);
    }
}

fn normalize_relative(path: &Path) -> Result<String, RepositoryAccessError> {
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

fn path_parts(path: &Path) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    for component in path.components() {
        if let Component::Normal(part) = component {
            parts.push(part.to_str()?.to_ascii_lowercase());
        }
    }
    Some(parts)
}

fn is_obvious_binary(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            let extension = extension.to_ascii_lowercase();
            OBVIOUS_BINARY_EXTENSIONS.contains(&extension.as_str())
        })
}

fn read_failure_makes_scan_incomplete(error: &RepositoryAccessError) -> bool {
    matches!(
        error,
        RepositoryAccessError::InvalidRelativePath
            | RepositoryAccessError::NonUtf8Path
            | RepositoryAccessError::NotRegularFile
            | RepositoryAccessError::NotFound
            | RepositoryAccessError::TooLarge
            | RepositoryAccessError::HardLinkedFile
            | RepositoryAccessError::ConcurrentModification
            | RepositoryAccessError::Io
    )
}

fn is_pruned(path: &Path) -> bool {
    // Non-UTF-8 paths are not lossy-normalized here. Discovery will reach the file and
    // normalize_relative() will mark the scan incomplete instead of collapsing distinct names.
    let Some(parts) = path_parts(path) else {
        return false;
    };
    if parts.iter().any(|part| {
        BUILTIN_PRUNED_DIRS.contains(&part.as_str()) || part.starts_with("cmake-build-")
    }) {
        return true;
    }
    parts
        .last()
        .is_some_and(|name| BUILTIN_PRUNED_FILES.contains(&name.as_str()))
}

fn is_denied(path: &Path) -> bool {
    // Do not convert invalid OS strings with U+FFFD: that can alias distinct filesystem paths.
    // Let strict normalization reject them later so completeness is reported accurately.
    let Some(parts) = path_parts(path) else {
        return false;
    };
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

const SENSITIVE_LITERAL_KEYS: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "private_key",
    "credential",
    "credentials",
    "secret_access_key",
    "awssecretaccesskey",
    "clientsecret",
    "accesstoken",
    "refreshtoken",
    "authtoken",
    "dbpassword",
    "databasepassword",
    "connection_string",
    "connectionstring",
    "database_url",
    "databaseurl",
    "authorization",
    "proxy_authorization",
    "proxyauthorization",
    "cookie",
    "set_cookie",
    "setcookie",
    "session_id",
    "sessionid",
    "session_token",
    "sessiontoken",
];

/// Defense in depth only. Path denial is the primary policy. Inline redaction is deliberately
/// limited to high-confidence credential forms so ordinary auth code is not destroyed.
#[must_use]
fn redact_high_confidence_secrets(text: &str) -> String {
    redact_high_confidence_secrets_with_limit(text, None).text
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RedactionOutcome {
    text: String,
    truncated: bool,
}

#[must_use]
fn redact_high_confidence_secrets_bounded(text: &str, max_output_bytes: usize) -> RedactionOutcome {
    redact_high_confidence_secrets_with_limit(text, Some(max_output_bytes))
}

fn redact_high_confidence_secrets_with_limit(
    text: &str,
    max_output_bytes: Option<usize>,
) -> RedactionOutcome {
    let initial_capacity = max_output_bytes.map_or(text.len(), |limit| text.len().min(limit));
    let mut output = String::with_capacity(initial_capacity);
    let mut truncated = false;
    let mut in_private_key = false;
    let mut pending_sensitive_value: Option<PendingSensitiveValue> = None;
    let mut sensitive_block_parent_indent: Option<usize> = None;

    for original_line in text.lines() {
        // A single minified line can contain hundreds of thousands of tiny secret assignments.
        // Passing such a line through the allocating redaction pipeline would create a large
        // transient buffer before the outer output limit had a chance to stop it. Suppress the
        // whole line for bounded callers instead. This is conservative (no secret can escape),
        // and `truncated` prevents the caller from treating the resulting analysis as complete.
        if max_output_bytes.is_some() && original_line.len() > MAX_BOUNDED_REDACTION_LINE_BYTES {
            truncated = true;
            if !push_redacted_line(&mut output, REDACTED_OVERSIZE_LINE, max_output_bytes) {
                break;
            }
            pending_sensitive_value = None;
            sensitive_block_parent_indent = None;
            continue;
        }

        let mut line = std::borrow::Cow::Borrowed(original_line);

        if let Some(parent_indent) = sensitive_block_parent_indent {
            let trimmed = line.as_ref().trim_start_matches([' ', '\t']);
            let indent = line.len().saturating_sub(trimmed.len());
            if trimmed.is_empty() {
                // Preserve source line numbers while suppressing block-scalar material.
                line = std::borrow::Cow::Borrowed("");
            } else if indent > parent_indent {
                line = std::borrow::Cow::Borrowed("");
            } else {
                sensitive_block_parent_indent = None;
            }
        }

        if sensitive_block_parent_indent.is_none() {
            if let Some(pending) = pending_sensitive_value {
                let trimmed = line.as_ref().trim_start_matches([' ', '\t']);
                let indent = line.len().saturating_sub(trimmed.len());
                let upper_trimmed = trimmed.to_ascii_uppercase();
                let begins_private_key =
                    upper_trimmed.contains("-----BEGIN ") && upper_trimmed.contains("PRIVATE KEY");
                if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
                    // YAML permits comments/blank lines between a key and its scalar value.
                } else if begins_private_key {
                    // Leave PEM/PGP begin markers intact for the existing whole-block redactor;
                    // replacing only the begin line here would prevent it from suppressing the
                    // following private-key material.
                    pending_sensitive_value = None;
                } else if indent <= pending.indent && !pending.allow_same_indent {
                    pending_sensitive_value = None;
                } else if is_yaml_block_scalar_indicator(trimmed) {
                    let leading = line.as_ref()[..indent].to_string();
                    line = std::borrow::Cow::Owned(format!(
                        "{leading}[SIPPION_REDACTED_MULTILINE_LITERAL]"
                    ));
                    pending_sensitive_value = None;
                    sensitive_block_parent_indent = Some(pending.indent);
                } else {
                    if let Some(redacted) = redact_indented_sensitive_scalar(
                        line.as_ref(),
                        pending.indent,
                        pending.allow_same_indent,
                    ) {
                        line = std::borrow::Cow::Owned(redacted);
                    }
                    // The first significant child decides whether this was a scalar. Nested maps,
                    // lists, or computed expressions are left to normal per-line redaction.
                    pending_sensitive_value = None;
                }
            }
        }

        if sensitive_block_parent_indent.is_none() {
            if let Some((parent_indent, redacted)) =
                redact_sensitive_block_scalar_declaration(line.as_ref())
            {
                line = std::borrow::Cow::Owned(redacted);
                pending_sensitive_value = None;
                sensitive_block_parent_indent = Some(parent_indent);
            }
        }

        let upper = line.as_ref().to_ascii_uppercase();
        let begins_private_key = upper.contains("-----BEGIN ") && upper.contains("PRIVATE KEY");
        let ends_private_key = upper.contains("-----END ") && upper.contains("PRIVATE KEY");

        let redacted_line = if begins_private_key {
            // One visible marker for the block. Subsequent private-key lines become empty lines,
            // preserving source line numbers without allowing redaction to amplify a 2 MiB input
            // into tens of MiB of repeated marker text.
            in_private_key = !ends_private_key;
            std::borrow::Cow::Borrowed("[SIPPION_REDACTED_PRIVATE_KEY]")
        } else if in_private_key {
            if ends_private_key {
                in_private_key = false;
            }
            std::borrow::Cow::Borrowed("")
        } else {
            let url_redacted = redact_url_userinfo_credentials(line.as_ref());
            let cookie_redacted = redact_cookie_header_values(&url_redacted);
            let header_auth_redacted =
                redact_explicit_authorization_header_credentials(&cookie_redacted);
            let auth_redacted = redact_auth_scheme_credentials(&header_auth_redacted);
            let jwt_redacted = redact_jwt_substrings(&auth_redacted);
            let token_redacted = redact_token_substrings(&jwt_redacted);
            std::borrow::Cow::Owned(redact_sensitive_literal_assignments(&token_redacted))
        };

        if pending_sensitive_value.is_none() && sensitive_block_parent_indent.is_none() {
            pending_sensitive_value = dangling_sensitive_key(original_line);
        }
        if !push_redacted_line(&mut output, redacted_line.as_ref(), max_output_bytes) {
            truncated = true;
            break;
        }
    }

    if !text.ends_with('\n') && output.ends_with('\n') {
        output.pop();
    }
    RedactionOutcome {
        text: output,
        truncated,
    }
}

fn push_redacted_line(output: &mut String, line: &str, max_output_bytes: Option<usize>) -> bool {
    let required = line.len().saturating_add(1);
    if let Some(limit) = max_output_bytes {
        if output.len().saturating_add(required) > limit {
            return false;
        }
    }
    output.push_str(line);
    output.push('\n');
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingSensitiveValue {
    indent: usize,
    allow_same_indent: bool,
}

fn dangling_sensitive_key(line: &str) -> Option<PendingSensitiveValue> {
    let trimmed = line.trim_start_matches([' ', '\t']);
    if trimmed.starts_with('#') || trimmed.starts_with("//") {
        return None;
    }
    let leading_indent = line.len().saturating_sub(trimmed.len());
    let lower = line.to_ascii_lowercase();
    let bytes = line.as_bytes();
    let lower_bytes = lower.as_bytes();

    for key in SENSITIVE_LITERAL_KEYS {
        let mut offset = 0usize;
        while offset < lower.len() {
            let Some(found) = lower[offset..].find(key) else {
                break;
            };
            let start = offset + found;
            let end = start + key.len();
            // Match the inline assignment boundary rule: allow a sensitive key to be
            // embedded after an underscore in prefixed names such as OPENAI_API_KEY or
            // DATABASE_PASSWORD, while still rejecting alphanumeric-prefix substrings.
            let previous_ok = start == 0 || !lower_bytes[start - 1].is_ascii_alphanumeric();
            let next_ok = end == lower.len()
                || !(lower_bytes[end].is_ascii_alphanumeric() || lower_bytes[end] == b'_');
            if previous_ok && next_ok {
                let mut tail_start = end;
                let quoted_key = start > 0
                    && end < bytes.len()
                    && matches!(bytes[start - 1], b'\'' | b'"')
                    && bytes[end] == bytes[start - 1];
                if quoted_key {
                    tail_start += 1;
                }
                while tail_start < bytes.len() && bytes[tail_start].is_ascii_whitespace() {
                    tail_start += 1;
                }
                if tail_start < bytes.len() && matches!(bytes[tail_start], b':' | b'=') {
                    let after = line[tail_start + 1..].trim();
                    if after.is_empty() || after.starts_with('#') || after.starts_with("//") {
                        return Some(PendingSensitiveValue {
                            indent: leading_indent,
                            // JSON/JS formatting is not indentation-sensitive. A quoted key or
                            // syntax before the key is enough evidence to accept a same-indent
                            // scalar on the next significant line.
                            allow_same_indent: quoted_key || start > leading_indent,
                        });
                    }
                }
            }
            offset = end.max(start + 1);
        }
    }
    None
}

fn is_yaml_block_scalar_indicator(trimmed: &str) -> bool {
    let token = trimmed
        .split_ascii_whitespace()
        .next()
        .unwrap_or_default()
        .trim_end_matches(',');
    if token.is_empty() {
        return false;
    }
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !matches!(first, '|' | '>') {
        return false;
    }
    chars.all(|ch| matches!(ch, '+' | '-' | '1'..='9'))
}

fn redact_sensitive_block_scalar_declaration(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start_matches([' ', '\t']);
    if trimmed.starts_with('#') || trimmed.starts_with("//") {
        return None;
    }
    let leading_indent = line.len().saturating_sub(trimmed.len());
    let lower = line.to_ascii_lowercase();
    let lower_bytes = lower.as_bytes();
    let bytes = line.as_bytes();

    for key in SENSITIVE_LITERAL_KEYS {
        let mut offset = 0usize;
        while offset < lower.len() {
            let Some(found) = lower[offset..].find(key) else {
                break;
            };
            let start = offset + found;
            let end = start + key.len();
            // Match the inline assignment boundary rule: allow a sensitive key to be
            // embedded after an underscore in prefixed names such as OPENAI_API_KEY or
            // DATABASE_PASSWORD, while still rejecting alphanumeric-prefix substrings.
            let previous_ok = start == 0 || !lower_bytes[start - 1].is_ascii_alphanumeric();
            let next_ok = end == lower.len()
                || !(lower_bytes[end].is_ascii_alphanumeric() || lower_bytes[end] == b'_');
            if previous_ok && next_ok {
                let mut tail_start = end;
                let quoted_key = start > 0
                    && end < bytes.len()
                    && matches!(bytes[start - 1], b'\'' | b'"')
                    && bytes[end] == bytes[start - 1];
                if quoted_key {
                    tail_start += 1;
                }
                while tail_start < bytes.len() && bytes[tail_start].is_ascii_whitespace() {
                    tail_start += 1;
                }
                if tail_start < bytes.len() && bytes[tail_start] == b':' {
                    let mut value_start = tail_start + 1;
                    while value_start < bytes.len() && bytes[value_start].is_ascii_whitespace() {
                        value_start += 1;
                    }
                    let mut token_end = value_start;
                    while token_end < bytes.len()
                        && !bytes[token_end].is_ascii_whitespace()
                        && bytes[token_end] != b'#'
                    {
                        token_end += 1;
                    }
                    if value_start < token_end
                        && is_yaml_block_scalar_indicator(&line[value_start..token_end])
                    {
                        let mut redacted = String::with_capacity(line.len() + 32);
                        redacted.push_str(&line[..value_start]);
                        redacted.push_str("[SIPPION_REDACTED_MULTILINE_LITERAL]");
                        redacted.push_str(&line[token_end..]);
                        return Some((leading_indent, redacted));
                    }
                }
            }
            offset = end.max(start + 1);
        }
    }
    None
}

fn redact_indented_sensitive_scalar(
    line: &str,
    parent_indent: usize,
    allow_same_indent: bool,
) -> Option<String> {
    const MARKER: &str = "[SIPPION_REDACTED_MULTILINE_LITERAL]";
    let trimmed = line.trim_start_matches([' ', '\t']);
    let indent = line.len().saturating_sub(trimmed.len());
    if (indent <= parent_indent && !allow_same_indent) || trimmed.is_empty() {
        return None;
    }

    if matches!(trimmed.as_bytes().first().copied(), Some(b'{') | Some(b'['))
        || trimmed.starts_with("- ")
        || trimmed.starts_with("${")
        || trimmed.starts_with('$')
    {
        return None;
    }

    let leading = &line[..indent];
    if matches!(
        trimmed.as_bytes().first().copied(),
        Some(b'\'') | Some(b'"')
    ) {
        let quote = trimmed.as_bytes()[0];
        let mut escaped = false;
        let mut end = None;
        for (offset, byte) in trimmed.as_bytes()[1..].iter().copied().enumerate() {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == quote {
                end = Some(offset + 1);
                break;
            }
        }
        let end = end?;
        if end <= 1 {
            return None;
        }
        let mut out = String::with_capacity(line.len() + MARKER.len());
        out.push_str(leading);
        out.push(quote as char);
        out.push_str(MARKER);
        out.push_str(&trimmed[end..]);
        return Some(out);
    }

    // Avoid treating an indented nested object (`type: string`) as the scalar value of the
    // sensitive parent key. A colon followed by whitespace is structural YAML, not a password.
    if trimmed
        .as_bytes()
        .windows(2)
        .any(|pair| pair[0] == b':' && pair[1].is_ascii_whitespace())
    {
        return None;
    }

    let comment_start = trimmed
        .as_bytes()
        .windows(2)
        .position(|pair| pair[0].is_ascii_whitespace() && pair[1] == b'#')
        .map(|position| position + 1);
    let value_end = comment_start.unwrap_or(trimmed.len());
    let value = trimmed[..value_end].trim_end();
    if value.is_empty()
        || value.starts_with('$')
        || value.contains("${")
        || value.contains('(')
        || value.contains(')')
        || value.contains("=>")
        || value.contains("::")
        || value.contains("&&")
        || value.contains("||")
    {
        return None;
    }

    let suffix = &trimmed[value.len()..];
    Some(format!("{leading}{MARKER}{suffix}"))
}

fn redact_explicit_authorization_header_credentials(line: &str) -> String {
    const HEADERS: &[&str] = &["authorization:", "proxy-authorization:"];

    fn credential_byte(byte: u8) -> bool {
        byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'+' | b'/' | b'=')
    }

    let lower = line.to_ascii_lowercase();
    let mut out = String::with_capacity(line.len());
    let mut cursor = 0usize;
    while cursor < line.len() {
        let next = HEADERS
            .iter()
            .filter_map(|header| {
                lower[cursor..]
                    .find(header)
                    .map(|offset| (cursor + offset, *header))
            })
            .filter(|(start, _)| {
                *start == 0
                    || !(line.as_bytes()[start - 1].is_ascii_alphanumeric()
                        || line.as_bytes()[start - 1] == b'_')
            })
            .min_by_key(|(start, _)| *start);
        let Some((start, header)) = next else {
            out.push_str(&line[cursor..]);
            break;
        };

        let bytes = line.as_bytes();
        let mut scheme_start = start + header.len();
        while scheme_start < bytes.len() && bytes[scheme_start].is_ascii_whitespace() {
            scheme_start += 1;
        }
        let mut scheme_end = scheme_start;
        while scheme_end < bytes.len() && bytes[scheme_end].is_ascii_alphabetic() {
            scheme_end += 1;
        }
        let scheme = &lower[scheme_start..scheme_end];
        if !matches!(scheme, "bearer" | "basic") {
            out.push_str(&line[cursor..scheme_end]);
            cursor = scheme_end.max(start + header.len());
            continue;
        }

        let mut token_start = scheme_end;
        while token_start < bytes.len() && bytes[token_start].is_ascii_whitespace() {
            token_start += 1;
        }
        let mut token_end = token_start;
        while token_end < bytes.len() && credential_byte(bytes[token_end]) {
            token_end += 1;
        }

        if token_end == token_start {
            out.push_str(&line[cursor..token_start]);
            cursor = token_start;
            continue;
        }

        out.push_str(&line[cursor..token_start]);
        out.push_str("[SIPPION_REDACTED_AUTH_CREDENTIAL]");
        cursor = token_end;
    }
    out
}

fn redact_auth_scheme_credentials(line: &str) -> String {
    // Raw HTTP header strings and shell snippets often contain `Bearer <token>` or `Basic <blob>`
    // without a key/value assignment that the literal redactor can recognize. Keep the scheme and
    // surrounding code, but replace only a credential-shaped following token.
    const SCHEMES: &[(&str, usize)] = &[("bearer", 16), ("basic", 12)];

    fn credential_byte(byte: u8) -> bool {
        byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'+' | b'/' | b'=')
    }

    let lower = line.to_ascii_lowercase();
    let mut out = String::with_capacity(line.len());
    let mut cursor = 0usize;
    while cursor < line.len() {
        let next = SCHEMES
            .iter()
            .filter_map(|(scheme, min_len)| {
                lower[cursor..].find(scheme).and_then(|offset| {
                    let start = cursor + offset;
                    let end = start + scheme.len();
                    let bytes = line.as_bytes();
                    let previous_ok = start == 0
                        || !(bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_');
                    let next_ok = end < bytes.len() && bytes[end].is_ascii_whitespace();
                    (previous_ok && next_ok).then_some((start, end, *min_len))
                })
            })
            .min_by_key(|(start, _, _)| *start);

        let Some((start, scheme_end, min_len)) = next else {
            out.push_str(&line[cursor..]);
            break;
        };

        let bytes = line.as_bytes();
        let mut token_start = scheme_end;
        while token_start < bytes.len() && bytes[token_start].is_ascii_whitespace() {
            token_start += 1;
        }
        let mut token_end = token_start;
        while token_end < bytes.len() && credential_byte(bytes[token_end]) {
            token_end += 1;
        }

        out.push_str(&line[cursor..token_start]);
        if token_end.saturating_sub(token_start) >= min_len {
            out.push_str("[SIPPION_REDACTED_AUTH_CREDENTIAL]");
            cursor = token_end;
        } else {
            // Not credential-shaped (for example `Bearer {token}` or prose). Preserve it and keep
            // scanning after the scheme so a later real credential on the line can still be found.
            cursor = token_start;
        }

        // `start` is used to choose the earliest scheme. The prefix through token_start was copied
        // above; keep this assertion as a guard against accidental cursor regressions.
        debug_assert!(start < scheme_end && cursor >= scheme_end);
    }
    out
}

fn redact_jwt_substrings(line: &str) -> String {
    fn jwt_byte(byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
    }

    let mut out = String::with_capacity(line.len());
    let mut cursor = 0usize;
    while cursor < line.len() {
        let Some(offset) = line[cursor..].find("eyJ") else {
            out.push_str(&line[cursor..]);
            break;
        };
        let start = cursor + offset;
        let bytes = line.as_bytes();
        if start > 0 && jwt_byte(bytes[start - 1]) {
            out.push_str(&line[cursor..start + 3]);
            cursor = start + 3;
            continue;
        }

        let mut end = start;
        while end < bytes.len() && jwt_byte(bytes[end]) {
            end += 1;
        }
        let candidate = &line[start..end];
        let segments = candidate.split('.').collect::<Vec<_>>();
        let jwt_shape = matches!(segments.len(), 3 | 5)
            && candidate.len() >= 32
            && segments.first().is_some_and(|segment| segment.len() >= 8)
            && segments.get(1).is_some_and(|segment| segment.len() >= 8)
            && segments.iter().all(|segment| !segment.is_empty());

        out.push_str(&line[cursor..start]);
        if jwt_shape {
            out.push_str("[SIPPION_REDACTED_JWT]");
            cursor = end;
        } else {
            out.push_str("eyJ");
            cursor = start + 3;
        }
    }
    out
}

fn redact_url_userinfo_credentials(line: &str) -> String {
    fn scheme_byte(byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.')
    }

    let mut out = String::with_capacity(line.len());
    let mut cursor = 0usize;
    while cursor < line.len() {
        let Some(relative_marker) = line[cursor..].find("://") else {
            out.push_str(&line[cursor..]);
            break;
        };
        let marker = cursor + relative_marker;
        let bytes = line.as_bytes();
        let mut scheme_start = marker;
        while scheme_start > 0 && scheme_byte(bytes[scheme_start - 1]) {
            scheme_start -= 1;
        }
        let scheme = &line[scheme_start..marker];
        let valid_scheme =
            !scheme.is_empty() && scheme.len() <= 32 && scheme.as_bytes()[0].is_ascii_alphabetic();
        if !valid_scheme {
            out.push_str(&line[cursor..marker + 3]);
            cursor = marker + 3;
            continue;
        }

        let authority_start = marker + 3;
        let mut authority_end = authority_start;
        while authority_end < bytes.len()
            && !bytes[authority_end].is_ascii_whitespace()
            && !matches!(
                bytes[authority_end],
                b'/' | b'?' | b'#' | b'\'' | b'"' | b'<' | b'>'
            )
        {
            authority_end += 1;
        }
        let authority = &line[authority_start..authority_end];
        let Some(at) = authority.rfind('@') else {
            out.push_str(&line[cursor..authority_start]);
            cursor = authority_start;
            continue;
        };
        let userinfo = &authority[..at];
        if !userinfo.contains(':') || userinfo.len() < 3 {
            out.push_str(&line[cursor..authority_start]);
            cursor = authority_start;
            continue;
        }

        out.push_str(&line[cursor..authority_start]);
        out.push_str("[SIPPION_REDACTED_URL_CREDENTIAL]");
        out.push('@');
        cursor = authority_start + at + 1;
    }
    out
}

fn redact_cookie_header_values(line: &str) -> String {
    const HEADERS: &[&str] = &["cookie:", "set-cookie:"];

    fn quote_context(line: &str, position: usize) -> Option<u8> {
        let bytes = line.as_bytes();
        let mut quote = None;
        let mut escaped = false;
        for &byte in &bytes[..position] {
            if escaped {
                escaped = false;
                continue;
            }
            if byte == b'\\' {
                escaped = true;
                continue;
            }
            if matches!(byte, b'\'' | b'"') {
                quote = if quote == Some(byte) {
                    None
                } else if quote.is_none() {
                    Some(byte)
                } else {
                    quote
                };
            }
        }
        quote
    }

    let lower = line.to_ascii_lowercase();
    let mut out = String::with_capacity(line.len());
    let mut cursor = 0usize;
    while cursor < line.len() {
        let next = HEADERS
            .iter()
            .filter_map(|header| {
                lower[cursor..]
                    .find(header)
                    .map(|offset| (cursor + offset, *header))
            })
            .filter(|(start, _)| *start == 0 || !line.as_bytes()[start - 1].is_ascii_alphanumeric())
            .min_by_key(|(start, _)| *start);
        let Some((start, header)) = next else {
            out.push_str(&line[cursor..]);
            break;
        };
        let mut value_start = start + header.len();
        let bytes = line.as_bytes();
        while value_start < bytes.len() && bytes[value_start].is_ascii_whitespace() {
            value_start += 1;
        }
        if value_start >= bytes.len() {
            out.push_str(&line[cursor..]);
            break;
        }

        let mut value_end = bytes.len();
        if let Some(quote) = quote_context(line, start) {
            let mut escaped = false;
            let mut i = value_start;
            while i < bytes.len() {
                if escaped {
                    escaped = false;
                } else if bytes[i] == b'\\' {
                    escaped = true;
                } else if bytes[i] == quote {
                    value_end = i;
                    break;
                }
                i += 1;
            }
        }

        out.push_str(&line[cursor..value_start]);
        out.push_str("[SIPPION_REDACTED_COOKIE]");
        cursor = value_end;
    }
    out
}

fn redact_token_substrings(line: &str) -> String {
    // Prefix + conservative minimum total token length. Replace only the token, never the whole line.
    const PREFIXES: &[(&str, usize)] = &[
        ("github_pat_", 32),
        ("ghp_", 32),
        ("glpat-", 24),
        ("npm_", 24),
        ("pypi-", 24),
        ("xapp-", 32),
        ("xoxb-", 32),
        ("xoxp-", 32),
        ("AIza", 30),
        ("AKIA", 20),
        ("ASIA", 20),
        ("sk-", 24),
    ];

    let mut out = String::with_capacity(line.len());
    let mut cursor = 0usize;
    while cursor < line.len() {
        let next = PREFIXES
            .iter()
            .filter_map(|(prefix, min_len)| {
                line[cursor..]
                    .find(prefix)
                    .map(|offset| (cursor + offset, *prefix, *min_len))
            })
            .min_by_key(|(start, _, _)| *start);
        let Some((start, prefix, min_len)) = next else {
            out.push_str(&line[cursor..]);
            break;
        };

        out.push_str(&line[cursor..start]);
        let bytes = line.as_bytes();
        let mut end = start + prefix.len();
        while end < bytes.len()
            && (bytes[end].is_ascii_alphanumeric() || matches!(bytes[end], b'_' | b'-'))
        {
            end += 1;
        }
        if end - start >= min_len {
            out.push_str("[SIPPION_REDACTED_TOKEN]");
            cursor = end;
        } else {
            out.push_str(prefix);
            cursor = start + prefix.len();
        }
    }
    out
}

fn redact_sensitive_literal_assignments(line: &str) -> String {
    const MARKER: &str = "[SIPPION_REDACTED_LITERAL]";

    fn sensitive_key_positions(line: &str) -> Vec<(usize, usize)> {
        // Lowercase once per source line. Re-lowercasing and rescanning the whole suffix after each
        // credential on a minified line turns multi-secret redaction into quadratic work. A single
        // pass per key keeps the work bounded by O(number_of_keys * line_bytes).
        let lower = line.to_ascii_lowercase();
        let lower_bytes = lower.as_bytes();
        let mut positions = Vec::new();
        for key in SENSITIVE_LITERAL_KEYS {
            let mut offset = 0usize;
            while offset < lower.len() {
                let Some(found) = lower[offset..].find(key) else {
                    break;
                };
                let start = offset + found;
                let end = start + key.len();
                let previous_ok = start == 0 || !lower_bytes[start - 1].is_ascii_alphanumeric();
                let next_ok = end == lower.len()
                    || !(lower_bytes[end].is_ascii_alphanumeric() || lower_bytes[end] == b'_');
                let inside_redaction_marker = line[..start]
                    .rfind("[SIPPION_REDACTED_")
                    .and_then(|marker_start| {
                        line[marker_start..]
                            .find(']')
                            .map(|marker_end| marker_start + marker_end >= start)
                    })
                    .unwrap_or(false);
                if previous_ok && next_ok && !inside_redaction_marker {
                    positions.push((start, end));
                }
                offset = end.max(start + 1);
            }
        }
        positions.sort_unstable();
        positions.dedup();
        positions
    }

    fn literal_span_after_key(line: &str, key_end: usize) -> Option<(usize, usize)> {
        fn literal_span_after_separator(
            line: &str,
            key_end: usize,
            separator: usize,
        ) -> Option<(usize, usize)> {
            let mut value_start = key_end + separator + 1;
            let bytes = line.as_bytes();
            while value_start < bytes.len() && bytes[value_start].is_ascii_whitespace() {
                value_start += 1;
            }
            if value_start >= bytes.len() {
                return None;
            }

            if matches!(bytes[value_start], b'\'' | b'"') {
                let quote = bytes[value_start];
                let mut cursor = value_start + 1;
                let mut escaped = false;
                while cursor < bytes.len() {
                    let byte = bytes[cursor];
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == quote {
                        let candidate = &line[value_start + 1..cursor];
                        // A sensitive key is high-confidence context by itself. Do not use a
                        // minimum credential length here: short values such as `password="x"`
                        // are still secrets and must not be disclosed. Preserve only a genuinely
                        // empty value or an existing redaction marker.
                        if candidate.contains("[SIPPION_REDACTED_URL_CREDENTIAL") {
                            return Some((value_start + 1, cursor));
                        }
                        if candidate.contains("[SIPPION_REDACTED_") || candidate.is_empty() {
                            return None;
                        }
                        return Some((value_start + 1, cursor));
                    }
                    cursor += 1;
                }
                return None;
            }

            let mut end = value_start;
            while end < bytes.len()
                && !bytes[end].is_ascii_whitespace()
                && !matches!(bytes[end], b',' | b'}' | b']' | b';')
            {
                end += 1;
            }
            let candidate = &line[value_start..end];
            let candidate_lower = candidate.to_ascii_lowercase();
            let trailing = line[end..].trim_start();
            // As above, credential length is not a safety signal once the key itself is a
            // high-confidence secret key. Keep structural sentinel values readable, but redact
            // every non-empty literal regardless of length.
            if matches!(candidate_lower.as_str(), "bearer" | "basic")
                && (trailing.starts_with("[SIPPION_REDACTED_")
                    || trailing.starts_with('{')
                    || trailing.starts_with('$')
                    || trailing.starts_with('<'))
            {
                return None;
            }
            if candidate.contains("[SIPPION_REDACTED_URL_CREDENTIAL") {
                let mut url_end = end;
                while url_end < bytes.len()
                    && !bytes[url_end].is_ascii_whitespace()
                    && !matches!(bytes[url_end], b',' | b'}' | b';')
                {
                    url_end += 1;
                }
                return Some((value_start, url_end));
            }
            if candidate.contains("[SIPPION_REDACTED_")
                || candidate.is_empty()
                || matches!(candidate_lower.as_str(), "true" | "false" | "null" | "none")
            {
                return None;
            }

            // A high-confidence secret key may legitimately contain URL/password punctuation
            // (`:`, `@`, `!`, `#`, `%`, ...), so do not whitelist literal characters. Preserve
            // obvious computed expressions and variable references instead; those contain no secret
            // literal to disclose and redacting them would destroy ordinary auth code.
            let looks_computed = candidate.starts_with('$')
                || candidate.contains("${")
                || candidate.contains('(')
                || candidate.contains(')')
                || candidate.contains("=>")
                || candidate.contains("::")
                || candidate.contains("&&")
                || candidate.contains("||");
            if looks_computed {
                return None;
            }
            Some((value_start, end))
        }

        let tail = &line[key_end..];
        let colon = tail.find(':').filter(|position| *position <= 32);
        let equals = tail.find('=').filter(|position| *position <= 64);

        // If '=' appears before ':', the colon belongs to the value (for example a URL scheme in
        // `DATABASE_URL=postgres://...`) and must never be treated as the key/value separator.
        if let Some(equals_separator) = equals {
            if colon.is_none_or(|colon_separator| equals_separator < colon_separator) {
                return literal_span_after_separator(line, key_end, equals_separator);
            }
        }

        // Otherwise ':' may introduce a YAML/JSON/object literal. When an '=' also follows, reject
        // a colon candidate that runs directly into that '=' without a structural delimiter: that
        // shape is a type annotation such as `password: SecretString = "..."`. A comma/brace/etc.
        // means the later '=' belongs to another assignment, so the colon literal remains valid.
        if let Some(colon_separator) = colon {
            if let Some(span) = literal_span_after_separator(line, key_end, colon_separator) {
                if let Some(equals_separator) = equals {
                    let equals_absolute = key_end + equals_separator;
                    if span.1 <= equals_absolute {
                        let between = &line[span.1..equals_absolute];
                        let has_structural_delimiter = between
                            .bytes()
                            .any(|byte| matches!(byte, b',' | b'}' | b']' | b';'));
                        if !has_structural_delimiter {
                            return literal_span_after_separator(line, key_end, equals_separator);
                        }
                    }
                }
                return Some(span);
            }
        }
        equals.and_then(|separator| literal_span_after_separator(line, key_end, separator))
    }

    let mut out = String::with_capacity(line.len());
    let mut copy_from = 0usize;
    let mut changed = false;

    for (key_start, key_end) in sensitive_key_positions(line) {
        // Ignore key-looking text already consumed inside a redacted literal.
        if key_start < copy_from {
            continue;
        }
        if let Some((replace_start, replace_end)) = literal_span_after_key(line, key_end) {
            if replace_start >= copy_from {
                out.push_str(&line[copy_from..replace_start]);
                out.push_str(MARKER);
                copy_from = replace_end;
                changed = true;
            }
        }
        // A computed/empty/non-literal value is not a secret by this heuristic; all later keys were
        // pre-indexed, so it cannot prevent a later literal on the same minified/config line from
        // being inspected.
    }

    if !changed {
        return line.to_string();
    }
    out.push_str(&line[copy_from..]);
    out
}

fn map_io(error: std::io::Error) -> RepositoryAccessError {
    match error.kind() {
        std::io::ErrorKind::NotFound => RepositoryAccessError::NotFound,
        _ => RepositoryAccessError::Io,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalized_query(q: &str) -> NormalizedQuery {
        crate::core::McpToolInput {
            q: q.to_string(),
            ..Default::default()
        }
        .normalize()
        .expect("valid test query")
    }

    struct TestRoot {
        path: PathBuf,
    }

    impl TestRoot {
        fn new(path: PathBuf) -> Self {
            Self { path }
        }
    }

    impl AsRef<Path> for TestRoot {
        fn as_ref(&self) -> &Path {
            &self.path
        }
    }

    impl std::ops::Deref for TestRoot {
        type Target = Path;

        fn deref(&self) -> &Self::Target {
            &self.path
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn temp_root(label: &str) -> TestRoot {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        TestRoot::new(std::env::temp_dir().join(format!("sippion-{label}-{nonce}")))
    }

    #[test]
    fn final_ranking_verifies_candidates_beyond_provisional_top_n() {
        let root = temp_root("final-top-n");
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(root.join("a.rs"), "alpha gamma beta\n").expect("write a");
        std::fs::write(root.join("z.rs"), "alpha beta gamma\n").expect("write z");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let outcome = repository
            .search(&normalized_query("alpha beta"), 1, None)
            .expect("search");

        assert_eq!(outcome.hits.len(), 1);
        assert_eq!(outcome.hits[0].relative_path, "z.rs");
    }

    #[test]
    fn exact_verification_cache_advances_into_new_candidates_across_adaptive_rounds() {
        let root = temp_root("verification-cache");
        std::fs::create_dir_all(&root).expect("create root");
        let source = format!("fn alpha() {{}}\n{}", " ".repeat(100 * 1024));
        std::fs::write(root.join("a.rs"), &source).expect("write a");
        std::fs::write(root.join("b.rs"), &source).expect("write b");
        let repository = RepositoryAccess::open(&root).expect("open repository");
        let query = normalized_query("alpha");
        let started = Instant::now();
        let mut policy_skips = HashMap::new();
        let mut verification_cache = HashMap::new();

        // 512 KiB gives exact verification 128 KiB: enough for one ~100 KiB file, not both.
        let first = repository
            .search_once(
                &query,
                8,
                None,
                &started,
                512 * 1024,
                &mut policy_skips,
                &mut verification_cache,
                None,
            )
            .expect("first round");
        assert_eq!(first.hits.len(), 1);
        assert_eq!(verification_cache.len(), 1);

        let second = repository
            .search_once(
                &query,
                8,
                None,
                &started,
                512 * 1024,
                &mut policy_skips,
                &mut verification_cache,
                None,
            )
            .expect("second round");
        assert_eq!(
            second.hits.len(),
            2,
            "the second byte grant must advance past the cached leading candidate"
        );
        assert_eq!(verification_cache.len(), 2);
        assert_eq!(
            second.coverage.scanned_bytes,
            source.len(),
            "only the newly verified candidate should consume source bytes in round two"
        );
    }

    #[test]
    fn structural_mapping_rejects_content_change_even_when_stamp_appears_current() {
        let root = temp_root("evidence-generation");
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(root.join("main.rs"), "fn alpha() {}\n").expect("write alpha");
        let repository = RepositoryAccess::open(&root).expect("open repository");
        let query = normalized_query("alpha");
        let search = repository.search(&query, 8, None).expect("search");
        let mut hit = search.hits.into_iter().next().expect("alpha hit");
        let old_fingerprint = hit
            .source_fingerprint
            .expect("verified content fingerprint");

        // Same-length replacement models the Windows case where size + mtime can fail to expose a
        // rewrite. Force the hit stamp to the new stamp so this test specifically exercises the
        // content fingerprint guard rather than the metadata guard.
        std::fs::write(root.join("main.rs"), "fn bravo() {}\n").expect("replace source");
        let replacement = repository.read_source("main.rs").expect("read replacement");
        assert_ne!(
            source_content_fingerprint(&replacement.text),
            old_fingerprint
        );
        hit.source_stamp = Some(replacement.stamp);

        let map = repository
            .map_from_hits(&query, &[hit], 1, None)
            .expect("bounded map");
        assert!(map.truncated);
        assert!(map.entries.is_empty());
        assert_eq!(map.invalidated_evidence_paths, vec!["main.rs"]);
    }

    #[test]
    fn structural_mapping_revalidates_hits_beyond_structural_limit() {
        let root = temp_root("evidence-beyond-map-limit");
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(root.join("a.rs"), "fn alpha() {}\n").expect("write a");
        std::fs::write(root.join("z.rs"), "fn alpha() {}\n").expect("write z");
        let repository = RepositoryAccess::open(&root).expect("open repository");

        let fresh_a = repository.read_source("a.rs").expect("read a");
        let old_z = repository.read_source("z.rs").expect("read old z");
        let old_z_fingerprint = source_content_fingerprint(&old_z.text);
        std::fs::write(root.join("z.rs"), "fn bravo() {}\n").expect("replace z");
        let replacement_z = repository.read_source("z.rs").expect("read replacement z");
        assert_ne!(
            source_content_fingerprint(&replacement_z.text),
            old_z_fingerprint
        );

        let hits = vec![
            SearchHit {
                relative_path: "a.rs".to_string(),
                start_line: 1,
                end_line: 1,
                excerpt: "fn alpha() {}".to_string(),
                score: 2.0,
                source_stamp: Some(fresh_a.stamp),
                source_fingerprint: Some(source_content_fingerprint(&fresh_a.text)),
            },
            SearchHit {
                relative_path: "z.rs".to_string(),
                start_line: 1,
                end_line: 1,
                excerpt: "fn alpha() {}".to_string(),
                score: 1.0,
                // Model the Windows same-size/same-mtime gap by making metadata appear current
                // while retaining the fingerprint from the old content generation.
                source_stamp: Some(replacement_z.stamp),
                source_fingerprint: Some(old_z_fingerprint),
            },
        ];

        let map = repository
            .map_from_hits(&normalized_query("alpha"), &hits, 1, None)
            .expect("bounded map");
        assert!(map.truncated);
        assert_eq!(map.invalidated_evidence_paths, vec!["z.rs"]);
        assert!(
            map.entries
                .iter()
                .any(|entry| entry.relative_path == "a.rs")
        );
    }

    #[test]
    fn shared_start_time_can_expire_structural_mapping_before_it_restarts_a_budget() {
        let root = temp_root("shared-deadline");
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(root.join("main.rs"), "fn alpha() {}\n").expect("write source");
        let repository = RepositoryAccess::open(&root).expect("open repository");
        let hits = vec![SearchHit {
            relative_path: "main.rs".to_string(),
            start_line: 1,
            end_line: 1,
            excerpt: "fn alpha() {}".to_string(),
            score: 1.0,
            source_stamp: None,
            source_fingerprint: None,
        }];
        let started = Instant::now()
            .checked_sub(MAX_SEARCH_WALL_TIME)
            .expect("representable deadline");

        let outcome = repository
            .map_from_hits_since(&normalized_query("alpha"), &hits, 1, None, &started)
            .expect("bounded map");
        assert!(outcome.truncated);
        assert!(outcome.entries.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn replaced_root_path_is_rejected_before_ambient_discovery() {
        let root = temp_root("root-identity");
        let moved = TestRoot::new(root.with_extension("moved"));
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(root.join("old.rs"), "fn original_root() {}\n").expect("write old");
        let repository = RepositoryAccess::open(&root).expect("open repository");

        std::fs::rename(&root, &moved).expect("move original root");
        std::fs::create_dir_all(&root).expect("replace root path");
        std::fs::write(root.join("new.rs"), "fn replacement_root() {}\n").expect("write new");

        let error = repository
            .search(&normalized_query("replacement_root"), 8, None)
            .expect_err("replacement must be rejected");
        assert_eq!(error, RepositoryAccessError::ConcurrentModification);
    }

    #[test]
    fn parent_escape_is_rejected() {
        assert_eq!(
            normalize_relative(Path::new("../secret")),
            Err(RepositoryAccessError::InvalidRelativePath)
        );
    }

    #[test]
    fn control_character_path_is_rejected() {
        assert_eq!(
            normalize_relative(Path::new("src/x\nFAKE.rs")),
            Err(RepositoryAccessError::InvalidRelativePath)
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_path_is_rejected_without_lossy_aliasing() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let invalid = PathBuf::from("src").join(OsString::from_vec(vec![0xff, b'.', b'r', b's']));
        assert_eq!(
            normalize_relative(&invalid),
            Err(RepositoryAccessError::NonUtf8Path)
        );
        assert_eq!(path_parts(&invalid), None);
    }

    #[cfg(windows)]
    #[test]
    fn non_unicode_windows_path_is_rejected_without_lossy_aliasing() {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;

        let invalid = PathBuf::from("src").join(OsString::from_wide(&[0xd800]));
        assert_eq!(
            normalize_relative(&invalid),
            Err(RepositoryAccessError::NonUtf8Path)
        );
        assert_eq!(path_parts(&invalid), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn discovery_marks_non_utf8_paths_incomplete_instead_of_collapsing_them() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let root = temp_root("non-utf8-path");
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(root.join("safe.rs"), "fn safe() {}\n").expect("write safe file");
        let invalid_name = OsString::from_vec(vec![b'b', b'a', b'd', 0xff, b'.', b'r', b's']);
        std::fs::write(root.join(invalid_name), "fn hidden() {}\n").expect("write non-utf8 file");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let started = Instant::now();
        let outcome = repository
            .discover_files(None, &started, &HashMap::new())
            .expect("discover files");

        assert!(outcome.truncated);
        assert_eq!(outcome.files.len(), 1);
        assert_eq!(outcome.files[0].path, "safe.rs");
    }

    #[test]
    fn environment_files_are_denied_except_templates() {
        assert!(is_denied(Path::new(".env.production")));
        assert!(!is_denied(Path::new(".env.example")));
        assert!(is_denied(Path::new("terraform.tfstate")));
    }

    #[test]
    fn ignored_subtree_prevents_repository_wide_no_match_claim() {
        let root = temp_root("gitignore-completeness");
        std::fs::create_dir_all(root.join("generated")).expect("generated dir");
        std::fs::write(root.join(".gitignore"), "generated/\n").expect("gitignore");
        std::fs::write(
            root.join("generated/ignored.rs"),
            "fn ignored_sentinel_7b19d4() {}\n",
        )
        .expect("ignored source");
        std::fs::write(root.join("visible.rs"), "fn visible() {}\n").expect("visible source");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let outcome = repository
            .search(&normalized_query("ignored_sentinel_7b19d4"), 8, None)
            .expect("search succeeds");

        assert!(
            outcome.hits.is_empty(),
            "gitignored source must remain uninspected"
        );
        assert!(
            outcome.coverage.policy_excluded_files >= 1,
            "ignore rules must prevent an absolute repository-wide NO_MATCH"
        );
        assert!(!outcome.truncated);
    }

    #[test]
    fn spaces_and_unicode_in_directories_and_file_names_are_preserved() {
        let root = temp_root("unicode and spaces");
        let nested = root.join("project 日本語").join("src with spaces");
        let source_path = nested.join("認証 handler.rs");
        std::fs::create_dir_all(&nested).expect("nested directory");
        std::fs::write(&source_path, "fn unicode_path_marker() {}\n").expect("source");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let outcome = repository
            .search(&normalized_query("unicode_path_marker"), 8, None)
            .expect("search succeeds");
        let expected = "project 日本語/src with spaces/認証 handler.rs";
        assert!(outcome.hits.iter().any(|hit| hit.relative_path == expected));
        let source = repository.read_source(expected).expect("read source");
        assert!(source.text.contains("unicode_path_marker"));
    }

    #[test]
    fn lf_and_crlf_sources_are_read_and_searchable() {
        let root = temp_root("line-endings");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::write(root.join("lf.rs"), "fn lf_marker() {}\nsecond line\n").expect("LF source");
        std::fs::write(
            root.join("crlf.rs"),
            b"fn crlf_marker() {}\r\nsecond line\r\n",
        )
        .expect("CRLF source");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        for marker in ["lf_marker", "crlf_marker"] {
            let outcome = repository
                .search(&normalized_query(marker), 8, None)
                .expect("search succeeds");
            assert!(
                outcome
                    .hits
                    .iter()
                    .any(|hit| hit.relative_path.ends_with(".rs"))
            );
        }
        let crlf = repository.read_source("crlf.rs").expect("read CRLF source");
        assert!(crlf.text.contains("\r\n"));
    }

    #[test]
    #[allow(clippy::permissions_set_readonly_false)]
    fn read_only_file_remains_readable_without_write_authority() {
        let root = temp_root("read-only");
        std::fs::create_dir_all(&root).expect("root");
        let path = root.join("read-only.rs");
        std::fs::write(&path, "fn read_only_marker() {}\n").expect("source");
        let mut permissions = std::fs::metadata(&path).expect("metadata").permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).expect("set read-only");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let source = repository
            .read_source("read-only.rs")
            .expect("read-only source remains readable");
        assert!(source.text.contains("read_only_marker"));

        // Windows refuses to remove a read-only file until its attribute is restored.
        let mut writable = std::fs::metadata(&path).expect("metadata").permissions();
        writable.set_readonly(false);
        std::fs::set_permissions(&path, writable).expect("restore permissions");
    }

    #[cfg(windows)]
    #[test]
    fn windows_relative_paths_normalize_backslashes_and_reject_absolute_paths() {
        assert_eq!(
            normalize_relative(Path::new(r"src\日本語\file.rs")),
            Ok("src/日本語/file.rs".to_string())
        );
        assert_eq!(
            normalize_relative(Path::new(r"C:\project\file.rs")),
            Err(RepositoryAccessError::InvalidRelativePath)
        );
        assert_eq!(
            normalize_relative(Path::new(r"\\server\share\file.rs")),
            Err(RepositoryAccessError::InvalidRelativePath)
        );
    }

    #[test]
    fn token_redaction_preserves_surrounding_code() {
        let input = "let token = \"sk-abcdefghijklmnopqrstuvwxyz0123456789\";";
        let redacted = redact_high_confidence_secrets(input);
        assert_eq!(redacted, "let token = \"[SIPPION_REDACTED_TOKEN]\";");
    }

    #[test]
    fn bounded_redaction_caps_amplification_from_many_short_literals() {
        let input = "token=\"x\";\n".repeat(4_096);
        let limit = input.len();
        let outcome = redact_high_confidence_secrets_bounded(&input, limit);

        assert!(
            outcome.truncated,
            "expanded redaction must report truncation"
        );
        assert!(outcome.text.len() <= limit);
        assert!(!outcome.text.contains("token=\"x\""));
    }

    #[test]
    fn bounded_redaction_suppresses_oversize_minified_line_before_expansion() {
        let repeats = MAX_BOUNDED_REDACTION_LINE_BYTES / "token=\"x\";".len() + 2;
        let input = "token=\"x\";".repeat(repeats);
        assert!(input.len() > MAX_BOUNDED_REDACTION_LINE_BYTES);

        let outcome = redact_high_confidence_secrets_bounded(&input, MAX_SOURCE_BYTES);

        assert!(outcome.truncated);
        assert_eq!(outcome.text, REDACTED_OVERSIZE_LINE);
        assert!(outcome.text.len() <= MAX_SOURCE_BYTES);
    }

    #[test]
    fn repository_map_reports_truncation_for_oversize_redaction_line() {
        let root = temp_root("map-redaction-bound");
        std::fs::create_dir_all(&root).expect("root");
        let repeats = MAX_BOUNDED_REDACTION_LINE_BYTES / "token=\"x\";".len() + 2;
        std::fs::write(root.join("danger.rs"), "token=\"x\";".repeat(repeats)).expect("source");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let hits = vec![SearchHit {
            relative_path: "danger.rs".to_string(),
            start_line: 1,
            end_line: 1,
            excerpt: String::new(),
            score: 1.0,
            source_stamp: None,
            source_fingerprint: None,
        }];
        let outcome = repository
            .map_from_hits(&normalized_query("token"), &hits, 1, None)
            .expect("map succeeds");

        assert!(outcome.truncated);
    }

    #[test]
    fn redacted_secret_match_returns_suppressed_evidence_instead_of_no_match() {
        let root = temp_root("redacted-match");
        std::fs::create_dir_all(&root).expect("root");
        let secret = "sk-abcdefghijklmnopqrstuvwxyz0123456789";
        std::fs::write(root.join("safe.rs"), format!("let token = \"{secret}\";\n"))
            .expect("source");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let outcome = repository
            .search(&normalized_query(secret), 8, None)
            .expect("search succeeds");
        let hit = outcome
            .hits
            .iter()
            .find(|hit| hit.relative_path == "safe.rs")
            .expect("redacted match must still be represented");

        assert_eq!(hit.start_line, 0);
        assert_eq!(hit.end_line, 0);
        assert_eq!(hit.excerpt, REDACTED_MATCH_EXCERPT);
        assert!(!hit.excerpt.contains(secret));
    }

    #[test]
    fn ordinary_auth_assignment_is_not_destroyed() {
        let input = "let password = config.password.clone();";
        assert_eq!(redact_high_confidence_secrets(input), input);
    }
    #[test]
    fn authorization_bearer_and_basic_credentials_are_redacted() {
        let bearer = "Authorization: Bearer abcdefghijklmnopqrstuvwxyz0123456789";
        let basic = "Proxy-Authorization: Basic dXNlcjpwYXNzd29yZA==";

        let bearer_redacted = redact_high_confidence_secrets(bearer);
        assert!(bearer_redacted.contains("Authorization: Bearer "));
        assert!(bearer_redacted.contains("SIPPION_REDACTED_AUTH_CREDENTIAL"));
        assert!(!bearer_redacted.contains("abcdefghijklmnopqrstuvwxyz0123456789"));

        let basic_redacted = redact_high_confidence_secrets(basic);
        assert!(basic_redacted.contains("Proxy-Authorization: Basic "));
        assert!(basic_redacted.contains("SIPPION_REDACTED_AUTH_CREDENTIAL"));
        assert!(!basic_redacted.contains("dXNlcjpwYXNzd29yZA=="));
    }

    #[test]
    fn short_explicit_authorization_credentials_are_redacted() {
        for input in [
            "Authorization: Basic YTpi",
            "Authorization: Bearer x",
            "Proxy-Authorization: Basic YQ==",
            "curl -H 'Authorization: Bearer abc' https://example.test",
        ] {
            let redacted = redact_high_confidence_secrets(input);
            assert!(redacted.contains("SIPPION_REDACTED_AUTH_CREDENTIAL"));
            assert!(!redacted.ends_with("Bearer x"));
            assert!(!redacted.contains("Basic YTpi"));
            assert!(!redacted.contains("Basic YQ=="));
            assert!(!redacted.contains("Bearer abc'"));
        }
    }

    #[test]
    fn multiline_sensitive_scalars_are_redacted() {
        let yaml = "password:\n  correct-horse-battery\nnext: safe";
        let yaml_redacted = redact_high_confidence_secrets(yaml);
        assert!(yaml_redacted.contains("password:"));
        assert!(yaml_redacted.contains("SIPPION_REDACTED_MULTILINE_LITERAL"));
        assert!(!yaml_redacted.contains("correct-horse-battery"));
        assert_eq!(yaml.lines().count(), yaml_redacted.lines().count());

        let json = "{\n  \"password\":\n    \"abc\",\n  \"safe\": true\n}";
        let json_redacted = redact_high_confidence_secrets(json);
        assert!(json_redacted.contains("\"password\":"));
        assert!(json_redacted.contains("SIPPION_REDACTED_MULTILINE_LITERAL"));
        assert!(!json_redacted.contains("\"abc\""));
        assert_eq!(json.lines().count(), json_redacted.lines().count());

        let compact_json = "{\"password\":\n\"xyz\"}";
        let compact_redacted = redact_high_confidence_secrets(compact_json);
        assert!(compact_redacted.contains("SIPPION_REDACTED_MULTILINE_LITERAL"));
        assert!(!compact_redacted.contains("\"xyz\""));
    }

    #[test]
    fn prefixed_multiline_sensitive_keys_are_redacted() {
        for input in [
            "OPENAI_API_KEY =\n  \"abc\"\nafter = safe",
            "DATABASE_PASSWORD =\n  \"x\"\nafter = safe",
            "AWS_SECRET_ACCESS_KEY =\n  \"short\"\nafter = safe",
            "SESSION_TOKEN =\n  \"xyz\"\nafter = safe",
        ] {
            let redacted = redact_high_confidence_secrets(input);
            assert!(redacted.contains("SIPPION_REDACTED_MULTILINE_LITERAL"));
            assert!(!redacted.contains("\"abc\""));
            assert!(!redacted.contains("\"x\""));
            assert!(!redacted.contains("\"short\""));
            assert!(!redacted.contains("\"xyz\""));
            assert!(redacted.contains("after = safe"));
            assert_eq!(input.lines().count(), redacted.lines().count());
        }

        let block = "OPENAI_API_KEY: |\n  first-secret-line\n  second-secret-line\nafter: safe";
        let block_redacted = redact_high_confidence_secrets(block);
        assert!(block_redacted.contains("SIPPION_REDACTED_MULTILINE_LITERAL"));
        assert!(!block_redacted.contains("first-secret-line"));
        assert!(!block_redacted.contains("second-secret-line"));
        assert!(block_redacted.ends_with("after: safe"));
        assert_eq!(block.lines().count(), block_redacted.lines().count());
    }

    #[test]
    fn multiline_sensitive_value_allows_comments_but_preserves_computed_and_nested_values() {
        let commented = "token:\n  # loaded below\n  very-secret-token\nnext: safe";
        let redacted = redact_high_confidence_secrets(commented);
        assert!(!redacted.contains("very-secret-token"));
        assert!(redacted.contains("# loaded below"));

        for input in [
            "password:\n  ${PASSWORD_FROM_ENV}\nnext: safe",
            "password:\n  type: string\nnext: safe",
        ] {
            assert_eq!(redact_high_confidence_secrets(input), input);
        }
    }

    #[test]
    fn yaml_sensitive_block_scalars_are_suppressed_with_line_count_preserved() {
        let input = "secret: |\n  first-secret-line\n  second-secret-line\nafter: safe";
        let redacted = redact_high_confidence_secrets(input);
        assert!(redacted.contains("SIPPION_REDACTED_MULTILINE_LITERAL"));
        assert!(!redacted.contains("first-secret-line"));
        assert!(!redacted.contains("second-secret-line"));
        assert!(redacted.ends_with("after: safe"));
        assert_eq!(input.lines().count(), redacted.lines().count());
    }

    #[test]
    fn jwt_cookie_session_and_url_credentials_are_redacted() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.c2lnbmF0dXJlMTIzNDU2";
        let jwt_redacted = redact_high_confidence_secrets(jwt);
        assert_eq!(jwt_redacted, "[SIPPION_REDACTED_JWT]");

        let cookie = "curl -H 'Cookie: session=abcdef0123456789; theme=dark' https://example.test";
        let cookie_redacted = redact_high_confidence_secrets(cookie);
        assert!(cookie_redacted.contains("Cookie: [SIPPION_REDACTED_COOKIE]"));
        assert!(cookie_redacted.ends_with("' https://example.test"));
        assert!(!cookie_redacted.contains("abcdef0123456789"));

        let session = r#"session_id = "abcdef0123456789""#;
        let session_redacted = redact_high_confidence_secrets(session);
        assert!(session_redacted.contains(r#"session_id = "[SIPPION_REDACTED_LITERAL]""#));
        assert!(!session_redacted.contains("abcdef0123456789"));

        let url = r#"let endpoint = "postgres://alice:correct-horse-battery@example.test/app";"#;
        let url_redacted = redact_high_confidence_secrets(url);
        assert!(
            url_redacted.contains("postgres://[SIPPION_REDACTED_URL_CREDENTIAL]@example.test/app")
        );
        assert!(!url_redacted.contains("correct-horse-battery"));
    }

    #[test]
    fn auth_placeholders_and_urls_without_passwords_are_preserved() {
        for input in [
            "Authorization: Bearer {token}",
            "Authorization: Bearer ${TOKEN}",
            "Authorization: Bearer <token>",
            "use Bearer token in documentation",
            "https://example.test/path",
            "https://alice@example.test/path",
        ] {
            assert_eq!(redact_high_confidence_secrets(input), input);
        }

        let explicit_bare_word = redact_high_confidence_secrets("Authorization: Bearer token");
        assert!(explicit_bare_word.contains("SIPPION_REDACTED_AUTH_CREDENTIAL"));
        assert!(!explicit_bare_word.ends_with("Bearer token"));
    }

    #[test]
    fn every_sensitive_literal_on_one_line_is_redacted() {
        let input = r#"{"password":"abcdefgh","token":"ijklmnop","api_key":"qrstuvwx"}"#;
        let redacted = redact_high_confidence_secrets(input);
        assert!(!redacted.contains("abcdefgh"));
        assert!(!redacted.contains("ijklmnop"));
        assert!(!redacted.contains("qrstuvwx"));
        assert_eq!(redacted.matches("SIPPION_REDACTED_LITERAL").count(), 3);
    }

    #[test]
    fn computed_sensitive_value_does_not_hide_later_literal() {
        let input = r#"password=config.password.clone(), token="abcdefghijkl""#;
        let redacted = redact_high_confidence_secrets(input);
        assert!(redacted.contains("config.password.clone()"));
        assert!(!redacted.contains("abcdefghijkl"));
        assert_eq!(redacted.matches("SIPPION_REDACTED_LITERAL").count(), 1);
    }

    #[test]
    fn later_equals_cannot_steal_an_earlier_colon_literal() {
        let input = r#"password: "abcdefgh", token="ijklmnop""#;
        let redacted = redact_high_confidence_secrets(input);
        assert!(!redacted.contains("abcdefgh"));
        assert!(!redacted.contains("ijklmnop"));
        assert_eq!(redacted.matches("SIPPION_REDACTED_LITERAL").count(), 2);
    }

    #[test]
    fn multiline_private_key_key_does_not_disable_whole_block_redaction() {
        let input = concat!(
            "private_key:\n",
            "  -----BEGIN PRIVATE KEY-----\n",
            "  SECRET-KEY-MATERIAL\n",
            "  -----END PRIVATE KEY-----\n",
            "after: safe",
        );
        let redacted = redact_high_confidence_secrets(input);
        assert!(redacted.contains("SIPPION_REDACTED_PRIVATE_KEY"));
        assert!(!redacted.contains("SECRET-KEY-MATERIAL"));
        assert!(redacted.ends_with("after: safe"));
        assert_eq!(input.lines().count(), redacted.lines().count());
    }

    #[test]
    fn whole_private_key_block_is_redacted() {
        let input = "-----BEGIN PRIVATE KEY-----\nABCDEF123456\n-----END PRIVATE KEY-----\ncode";
        let redacted = redact_high_confidence_secrets(input);
        assert!(!redacted.contains("ABCDEF123456"));
        assert!(redacted.ends_with("code"));
    }

    #[cfg(unix)]
    #[test]
    fn fifo_replacement_is_rejected_without_blocking() {
        use std::process::Command;
        use std::sync::mpsc;

        let root = temp_root("fifo-replacement");
        std::fs::create_dir_all(&root).expect("temp root");
        let path = root.join("victim.rs");
        std::fs::write(&path, "fn victim() {}\n").expect("write initial regular file");
        let repository = Arc::new(RepositoryAccess::open(&root).expect("open repository"));

        std::fs::remove_file(&path).expect("remove regular file");
        let status = match Command::new("mkfifo").arg(&path).status() {
            Ok(status) => status,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return;
            }
            Err(error) => panic!("run mkfifo: {error}"),
        };
        assert!(
            status.success(),
            "mkfifo must succeed for FIFO regression test"
        );

        let (tx, rx) = mpsc::channel();
        let worker_repository = Arc::clone(&repository);
        let worker = std::thread::spawn(move || {
            let _ = tx.send(worker_repository.read_source("victim.rs"));
        });

        let result = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("FIFO open must not block waiting for a writer");
        assert!(matches!(
            result,
            Err((RepositoryAccessError::NotRegularFile, 0))
        ));
        worker.join().expect("FIFO read worker");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_alias_cannot_bypass_denied_path() {
        use std::os::unix::fs::symlink;
        let root = temp_root("symlink-alias");
        std::fs::create_dir_all(&root).expect("temp root");
        std::fs::write(root.join(".env"), "not-a-real-secret").expect("write denied file");
        symlink(".env", root.join("safe.txt")).expect("create symlink");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        assert!(repository.read_source("safe.txt").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn final_symlink_to_allowed_file_is_still_refused() {
        use std::os::unix::fs::symlink;
        let root = temp_root("final-link");
        std::fs::create_dir_all(&root).expect("temp root");
        std::fs::write(root.join("real.rs"), "fn real() {}").expect("write real file");
        symlink("real.rs", root.join("alias.rs")).expect("create symlink");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        assert!(repository.read_source("alias.rs").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn parent_directory_symlink_is_refused() {
        use std::os::unix::fs::symlink;
        let root = temp_root("parent-link");
        std::fs::create_dir_all(root.join("real")).expect("temp root");
        std::fs::write(root.join("real/file.rs"), "fn real() {}").expect("write real file");
        symlink("real", root.join("alias")).expect("create directory symlink");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        assert!(repository.read_source("alias/file.rs").is_err());
    }

    #[test]
    fn regular_file_still_reads_after_nofollow_hardening() {
        let root = temp_root("regular");
        std::fs::create_dir_all(&root).expect("temp root");
        std::fs::write(root.join("safe.rs"), "fn safe() {}").expect("write regular file");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let source = repository
            .read_source("safe.rs")
            .expect("read regular file");
        assert_eq!(source.text, "fn safe() {}");
    }

    #[cfg(unix)]
    #[test]
    fn hard_linked_source_is_denied_and_policy_excluded() {
        let root = temp_root("hardlink-root");
        let outside = temp_root("hardlink-outside");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::create_dir_all(&outside).expect("outside");
        let outside_file = outside.join("secret.rs");
        std::fs::write(&outside_file, "fn outside_secret() {}\n").expect("write outside");
        std::fs::hard_link(&outside_file, root.join("looks_safe.rs")).expect("create hard link");
        std::fs::write(root.join("normal.rs"), "fn normal() {}\n").expect("write normal");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let (error, _) = repository
            .read_source("looks_safe.rs")
            .expect_err("hard-linked source must be denied");
        assert_eq!(error, RepositoryAccessError::HardLinkedFile);

        let outcome = repository
            .search(&normalized_query("definitely_missing"), 8, None)
            .expect("search succeeds");
        assert_eq!(outcome.coverage.policy_excluded_files, 1);
        assert_eq!(
            outcome.coverage.indexed_files,
            outcome.coverage.eligible_files
        );
        assert_eq!(outcome.coverage.confidence_milli, 350);
        assert!(!outcome.truncated);
    }

    #[cfg(windows)]
    #[test]
    fn windows_hard_linked_source_is_denied_by_open_handle_information() {
        let root = temp_root("windows-hardlink-root");
        let outside = temp_root("windows-hardlink-outside");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::create_dir_all(&outside).expect("outside");
        let outside_file = outside.join("secret.rs");
        std::fs::write(&outside_file, "fn outside_secret() {}\n").expect("write outside");
        std::fs::hard_link(&outside_file, root.join("looks_safe.rs")).expect("create hard link");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let (error, _) = repository
            .read_source("looks_safe.rs")
            .expect_err("hard-linked source must be denied on Windows");
        assert_eq!(error, RepositoryAccessError::HardLinkedFile);
    }

    #[cfg(unix)]
    #[test]
    fn source_stamp_detects_same_length_file_replacement() {
        let root = temp_root("stamp-replacement");
        std::fs::create_dir_all(&root).expect("root");
        let path = root.join("same.rs");
        let replacement = root.join("replacement.rs");
        std::fs::write(&path, "AAAA\n").expect("write original");
        let before = source_stamp(&std::fs::metadata(&path).expect("metadata before"));
        std::fs::write(&replacement, "BBBB\n").expect("write replacement");
        std::fs::rename(&replacement, &path).expect("replace same-length file");
        let after = source_stamp(&std::fs::metadata(&path).expect("metadata after"));
        assert_ne!(before, after);
        assert_eq!(before.len, after.len);
    }

    #[test]
    fn reset_ram_index_discards_cached_documents_and_saturation() {
        let root = temp_root("reset-index");
        std::fs::create_dir_all(&root).expect("root");
        let repository = RepositoryAccess::open(&root).expect("open repository");
        repository
            .insert_index_document(
                "cached.rs".to_string(),
                build_indexed_document("old cached term", None),
            )
            .expect("insert cached document");
        {
            let mut index = repository.ram_index.lock().expect("index lock");
            index.saturated = true;
            assert!(!index.files.is_empty());
        }

        repository.reset_ram_index().expect("reset index");
        let index = repository.ram_index.lock().expect("index lock");
        assert!(index.files.is_empty());
        assert_eq!(index.total_entries, 0);
        assert!(!index.saturated);
    }

    #[cfg(windows)]
    #[test]
    fn windows_search_rebuilds_a_stale_same_stamp_ram_index() {
        let root = temp_root("windows-stale-index");
        std::fs::create_dir_all(&root).expect("root");
        let path = root.join("same.rs");
        std::fs::write(&path, "fresh_unique_term\n").expect("write source");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let stamp = source_stamp(&std::fs::metadata(&path).expect("metadata"));
        repository
            .insert_index_document(
                "same.rs".to_string(),
                build_indexed_document("stale_unique_term\n", Some(stamp)),
            )
            .expect("seed stale same-stamp index");

        let outcome = repository
            .search(&normalized_query("fresh_unique_term"), 8, None)
            .expect("search");
        assert!(
            outcome
                .hits
                .iter()
                .any(|hit| hit.relative_path == "same.rs")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_map_discards_stale_same_stamp_structural_caches() {
        let root = temp_root("windows-stale-structural-cache");
        std::fs::create_dir_all(&root).expect("root");
        let path = root.join("same.rs");
        std::fs::write(&path, "pub fn fresh_symbol() -> bool { true }\n").expect("write source");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let stamp = source_stamp(&std::fs::metadata(&path).expect("metadata"));
        {
            let mut analysis = repository.analysis_cache.lock().expect("analysis cache");
            analysis.entries.insert(
                "same.rs".to_string(),
                CachedAnalysis {
                    stamp: stamp.clone(),
                    symbols: vec![CachedRepoMapSymbol {
                        name: "stale_symbol".to_string(),
                        kind: "function".to_string(),
                        line: 1,
                    }],
                    semantics: SemanticFacts::default(),
                    cacheable: true,
                    last_used: 1,
                },
            );
        }
        let stale_graph_key = GraphCacheKey(vec![GraphCacheNode {
            path: "same.rs".to_string(),
            stamp,
        }]);
        {
            let mut graph = repository.graph_cache.lock().expect("graph cache");
            graph.entries.insert(
                stale_graph_key,
                CachedGraph {
                    edge_maps: vec![HashMap::new()],
                    centrality: vec![999.0],
                    last_used: 1,
                },
            );
        }

        let hits = vec![SearchHit {
            relative_path: "same.rs".to_string(),
            start_line: 1,
            end_line: 1,
            excerpt: "fresh_symbol".to_string(),
            score: 1.0,
            source_stamp: None,
            source_fingerprint: None,
        }];
        let outcome = repository
            .map_from_hits(&normalized_query("fresh_symbol"), &hits, 1, None)
            .expect("map");
        let entry = outcome.entries.first().expect("map entry");
        assert!(
            entry
                .symbols
                .iter()
                .any(|symbol| symbol.name == "fresh_symbol")
        );
        assert!(
            !entry
                .symbols
                .iter()
                .any(|symbol| symbol.name == "stale_symbol")
        );
        assert!(
            entry.score < 100.0,
            "stale graph centrality must not be reused"
        );
    }

    #[test]
    fn search_candidate_excerpt_is_bounded_before_retention() {
        let long = "x".repeat(MAX_SEARCH_EXCERPT_BYTES * 2);
        let lines = [long.as_str()];
        let (bounded, start, end) = bounded_search_excerpt(&lines, 0, MAX_SEARCH_EXCERPT_BYTES);
        assert!(bounded.len() <= MAX_SEARCH_EXCERPT_BYTES);
        assert!(bounded.contains("SIPPION_EXCERPT_TRUNCATED"));
        assert_eq!((start, end), (0, 1));
    }

    #[test]
    fn non_utf8_read_reports_consumed_bytes_for_scan_budget() {
        let root = temp_root("binary");
        std::fs::create_dir_all(&root).expect("temp root");
        let bytes = [0xff, 0xfe, 0xfd, 0x00];
        std::fs::write(root.join("binary.bin"), bytes).expect("write binary file");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let (error, consumed) = repository
            .read_source("binary.bin")
            .expect_err("binary must not become model text");
        assert_eq!(error, RepositoryAccessError::NonUtf8Source);
        assert_eq!(consumed, bytes.len());
    }

    #[test]
    fn file_local_best_hit_prefers_higher_score_then_earlier_line() {
        let earlier = SearchHit {
            relative_path: "a.rs".into(),
            start_line: 2,
            end_line: 2,
            excerpt: "earlier".into(),
            score: 10.0,
            source_stamp: None,
            source_fingerprint: None,
        };
        let later = SearchHit {
            start_line: 20,
            ..earlier.clone()
        };
        let higher = SearchHit {
            score: 11.0,
            ..later.clone()
        };
        assert!(hit_is_better(&earlier, &later));
        assert!(hit_is_better(&higher, &earlier));
    }

    #[test]
    fn bounded_focus_line_is_utf8_safe_and_byte_bounded() {
        let line = format!("{}MATCH{}", "界".repeat(900), "界".repeat(900));
        let match_byte = line.find("MATCH").expect("match");
        let bounded = bounded_focus_line(&line, match_byte);
        assert!(bounded.contains("MATCH"));
        assert!(bounded.len() <= MAX_SEARCH_EXCERPT_BYTES);
    }

    #[test]
    fn bounded_excerpt_keeps_match_when_an_adjacent_line_is_huge() {
        let huge = "x".repeat(MAX_SEARCH_EXCERPT_BYTES * 2);
        let lines = [huge.as_str(), "authentication_token_validation"];
        let (excerpt, start, end) = bounded_search_excerpt(&lines, 1, 0);
        assert!(excerpt.contains("authentication_token_validation"));
        assert!(excerpt.len() <= MAX_SEARCH_EXCERPT_BYTES);
        assert!(start <= 1 && end >= 2);
    }

    #[test]
    fn multi_line_query_terms_score_as_one_evidence_window() {
        let root = temp_root("window-score");
        std::fs::create_dir_all(&root).expect("temp root");
        std::fs::write(
            root.join("a_relevant.rs"),
            "fn check() {\n    // authentication\n    let token = load();\n    // validation\n}\n",
        )
        .expect("write relevant source");
        std::fs::write(root.join("z_noise.rs"), "let token = load();\n")
            .expect("write noise source");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let outcome = repository
            .search(
                &normalized_query("authentication token validation"),
                8,
                None,
            )
            .expect("search succeeds");
        assert_eq!(
            outcome.hits.first().map(|hit| hit.relative_path.as_str()),
            Some("a_relevant.rs")
        );
        assert!(outcome.hits[0].score > outcome.hits[1].score);
    }

    #[test]
    fn obvious_binary_formats_are_pruned_before_source_scan() {
        assert!(is_obvious_binary(Path::new("assets/logo.png")));
        assert!(is_obvious_binary(Path::new("lib/archive.JAR")));
        assert!(!is_obvious_binary(Path::new("src/app.rs")));
        assert!(!is_obvious_binary(Path::new("assets/icon.svg")));
    }

    #[test]
    fn non_utf8_skip_is_not_reported_as_bounded_scan_failure() {
        assert!(!read_failure_makes_scan_incomplete(
            &RepositoryAccessError::NonUtf8Source
        ));
        assert!(read_failure_makes_scan_incomplete(
            &RepositoryAccessError::TooLarge
        ));
        assert!(!read_failure_makes_scan_incomplete(
            &RepositoryAccessError::DeniedPath
        ));
    }

    #[test]
    fn structural_map_links_symbol_references_with_multi_pattern_matcher() {
        let root = temp_root("structural-aho");
        std::fs::create_dir_all(&root).expect("temp root");
        std::fs::write(root.join("caller.rs"), "fn handle() { authenticate(); }\n")
            .expect("write caller");
        std::fs::write(
            root.join("auth.rs"),
            "pub fn authenticate() -> bool { true }\n",
        )
        .expect("write auth");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let hits = vec![
            SearchHit {
                relative_path: "caller.rs".into(),
                start_line: 1,
                end_line: 1,
                excerpt: "authenticate".into(),
                score: 10.0,
                source_stamp: None,
                source_fingerprint: None,
            },
            SearchHit {
                relative_path: "auth.rs".into(),
                start_line: 1,
                end_line: 1,
                excerpt: "authenticate".into(),
                score: 9.0,
                source_stamp: None,
                source_fingerprint: None,
            },
        ];
        let map = repository
            .map_from_hits(&normalized_query("authenticate"), &hits, 2, None)
            .expect("map succeeds");
        let caller = map
            .entries
            .iter()
            .find(|entry| entry.relative_path == "caller.rs")
            .expect("caller entry");
        assert!(caller.links_to.iter().any(|path| path == "auth.rs"));
        assert!(
            caller
                .semantic_links
                .iter()
                .any(|link| { link.relative_path == "auth.rs" && link.weight >= 0.80 })
        );
    }

    #[test]
    fn structural_analysis_and_graph_are_shared_across_repeated_calls() {
        let root = temp_root("shared-analysis-cache");
        std::fs::create_dir_all(&root).expect("temp root");
        std::fs::write(root.join("caller.rs"), "fn handle() { authenticate(); }\n")
            .expect("write caller");
        std::fs::write(
            root.join("auth.rs"),
            "pub fn authenticate() -> bool { true }\n",
        )
        .expect("write auth");
        let repository = RepositoryAccess::open(&root).expect("open repository");
        let hits = vec![
            SearchHit {
                relative_path: "caller.rs".into(),
                start_line: 1,
                end_line: 1,
                excerpt: "authenticate".into(),
                score: 10.0,
                source_stamp: None,
                source_fingerprint: None,
            },
            SearchHit {
                relative_path: "auth.rs".into(),
                start_line: 1,
                end_line: 1,
                excerpt: "authenticate".into(),
                score: 9.0,
                source_stamp: None,
                source_fingerprint: None,
            },
        ];
        let query = normalized_query("authenticate");
        repository
            .map_from_hits(&query, &hits, 2, None)
            .expect("first map");
        let analysis_entries = repository
            .analysis_cache
            .lock()
            .expect("analysis cache")
            .entries
            .len();
        let graph_entries = repository
            .graph_cache
            .lock()
            .expect("graph cache")
            .entries
            .len();
        assert_eq!(analysis_entries, 2);
        assert_eq!(graph_entries, 1);

        repository
            .map_from_hits(&query, &hits, 2, None)
            .expect("second map");
        assert_eq!(
            repository
                .analysis_cache
                .lock()
                .expect("analysis cache")
                .entries
                .len(),
            analysis_entries
        );
        assert_eq!(
            repository
                .graph_cache
                .lock()
                .expect("graph cache")
                .entries
                .len(),
            graph_entries
        );
    }

    #[test]
    fn sibling_agent_memory_prefers_complementary_paths() {
        let root = temp_root("agent-diversity");
        std::fs::create_dir_all(&root).expect("temp root");
        let repository = RepositoryAccess::open(&root).expect("open repository");
        let query = normalized_query("authentication token");
        let hits = vec![SearchHit {
            relative_path: "src/auth.rs".into(),
            start_line: 1,
            end_line: 1,
            excerpt: "authentication token".into(),
            score: 10.0,
            source_stamp: None,
            source_fingerprint: None,
        }];
        let first = CoordinationContext {
            session_id: Some("bugfix-1".into()),
            agent_id: Some("agent-a".into()),
        };
        let sibling = CoordinationContext {
            session_id: Some("bugfix-1".into()),
            agent_id: Some("agent-b".into()),
        };
        repository.remember_search(&query, &hits, Some(&first));
        assert!(repository.memory_adjustment(&query.terms, "src/auth.rs", Some(&first)) > 0.0);
        assert!(repository.memory_adjustment(&query.terms, "src/auth.rs", Some(&sibling)) < 0.0);
    }

    #[test]
    fn pre_cancelled_search_stops_before_discovery() {
        let root = temp_root("cancelled");
        std::fs::create_dir_all(&root).expect("temp root");
        std::fs::write(root.join("source.rs"), "fn target() {}\n").expect("write source");
        let repository = RepositoryAccess::open(&root).expect("open repository");
        let cancelled = AtomicBool::new(true);

        let result = repository.search(&normalized_query("target helper"), 8, Some(&cancelled));
        assert_eq!(result, Err(RepositoryAccessError::Cancelled));
    }

    #[test]
    fn high_confidence_aws_access_key_is_redacted_without_erasing_line() {
        let text = "access_key = AKIAABCDEFGHIJKLMNOP # fixture";
        let redacted = redact_high_confidence_secrets(text);
        assert_eq!(redacted, "access_key = [SIPPION_REDACTED_TOKEN] # fixture");
    }

    #[test]
    fn sensitive_literal_assignments_are_redacted_without_erasing_the_key() {
        let cases = [
            (
                r#"password = "correct-horse-battery""#,
                "password",
                "correct-horse-battery",
            ),
            (
                r#""client_secret": "abcdefghijklmnop""#,
                "client_secret",
                "abcdefghijklmnop",
            ),
            (
                "api_token: abcdefghijklmnop",
                "api_token",
                "abcdefghijklmnop",
            ),
            (
                r#"AWS_SECRET_ACCESS_KEY = "abcdefghijklmnopqrstuvwx""#,
                "AWS_SECRET_ACCESS_KEY",
                "abcdefghijklmnopqrstuvwx",
            ),
            (
                r#"clientSecret = "abcdefghijklmnop""#,
                "clientSecret",
                "abcdefghijklmnop",
            ),
            (
                r#"DATABASE_URL = "postgres://user:password@example.test/db""#,
                "DATABASE_URL",
                "postgres://user:password@example.test/db",
            ),
        ];
        for (input, key, secret) in cases {
            let redacted = redact_high_confidence_secrets(input);
            assert!(redacted.contains(key));
            assert!(redacted.contains("SIPPION_REDACTED_LITERAL"));
            assert!(!redacted.contains(secret));
        }
    }

    #[test]
    fn short_sensitive_literal_assignments_are_redacted() {
        let cases = [
            (r#"password = "x""#, "x"),
            ("token=abc", "abc"),
            (r#"client_secret='12'"#, "12"),
            ("api_key=q", "q"),
        ];

        for (input, secret) in cases {
            let redacted = redact_high_confidence_secrets(input);
            assert!(redacted.contains("SIPPION_REDACTED_LITERAL"));
            assert!(!redacted.contains(secret));
        }
    }

    #[test]
    fn empty_and_structural_sensitive_values_are_preserved() {
        for input in [
            r#"password = """#,
            "token=''",
            "api_key=false",
            "client_secret=null",
        ] {
            assert_eq!(redact_high_confidence_secrets(input), input);
        }
    }

    #[test]
    fn unquoted_secret_literals_with_url_and_password_punctuation_are_redacted() {
        let cases = [
            (
                "password: p@ssw0rd!",
                "password: [SIPPION_REDACTED_LITERAL]",
            ),
            (
                "DATABASE_URL=postgres://user:password@example.test/db",
                "DATABASE_URL=[SIPPION_REDACTED_LITERAL]",
            ),
            ("token=abc#def!ghi", "token=[SIPPION_REDACTED_LITERAL]"),
        ];
        for (input, expected) in cases {
            assert_eq!(redact_high_confidence_secrets(input), expected);
        }
    }

    #[test]
    fn type_annotation_does_not_hide_the_actual_secret_literal() {
        let input = r#"let password: SecretString = "correct-horse-battery";"#;
        let redacted = redact_high_confidence_secrets(input);
        assert!(redacted.contains("password: SecretString = "));
        assert!(!redacted.contains("correct-horse-battery"));
        assert_eq!(redacted.matches("SIPPION_REDACTED_LITERAL").count(), 1);
    }

    #[test]
    fn computed_secret_references_with_calls_or_shell_expansion_are_not_destroyed() {
        let cases = [
            "let password = config.password.clone();",
            "let token = load_token_from_keychain();",
            "TOKEN=${TOKEN_FROM_KEYCHAIN}",
            "PASSWORD=$PASSWORD_FROM_ENV",
        ];
        for input in cases {
            assert_eq!(redact_high_confidence_secrets(input), input);
        }
    }

    #[test]
    fn computed_secret_values_are_not_destroyed() {
        let input = "let token = load_token_from_keychain();";
        assert_eq!(redact_high_confidence_secrets(input), input);
    }

    #[test]
    fn pgp_private_key_blocks_are_redacted() {
        let input = concat!(
            "-----BEGIN PGP PRIVATE KEY BLOCK-----\n",
            "super-secret-material\n",
            "-----END PGP PRIVATE KEY BLOCK-----\n",
            "after"
        );
        let redacted = redact_high_confidence_secrets(input);
        assert!(!redacted.contains("super-secret-material"));
        assert!(redacted.contains("SIPPION_REDACTED_PRIVATE_KEY"));
        assert!(redacted.ends_with("after"));
    }

    #[test]
    fn oversized_source_is_policy_excluded_without_adaptive_retry() {
        let root = temp_root("oversized-policy");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::write(root.join("normal.rs"), "fn normal() {}\n").expect("write normal");
        std::fs::write(root.join("huge.rs"), vec![b'x'; MAX_SOURCE_BYTES + 1]).expect("write huge");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let outcome = repository
            .search(&normalized_query("definitely_missing"), 8, None)
            .expect("search succeeds");
        assert_eq!(outcome.coverage.policy_excluded_files, 1);
        assert_eq!(outcome.coverage.adaptive_rounds, 1);
        assert_eq!(
            outcome.coverage.indexed_files,
            outcome.coverage.eligible_files
        );
        assert_eq!(outcome.coverage.confidence_milli, 350);
        assert!(!outcome.truncated);
    }

    #[test]
    fn stable_non_utf8_source_is_policy_excluded_without_adaptive_retry() {
        let root = temp_root("nonutf8-policy");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::write(root.join("normal.rs"), "fn normal() {}\n").expect("write normal");
        std::fs::write(root.join("bad.rs"), [0xff, 0xfe, 0xfd, 0x00]).expect("write non-utf8");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let outcome = repository
            .search(&normalized_query("definitely_missing"), 8, None)
            .expect("search succeeds");
        assert_eq!(outcome.coverage.policy_excluded_files, 1);
        assert_eq!(outcome.coverage.adaptive_rounds, 1);
        assert_eq!(
            outcome.coverage.indexed_files,
            outcome.coverage.eligible_files
        );
        assert_eq!(outcome.coverage.confidence_milli, 350);
        assert!(!outcome.truncated);
    }

    #[test]
    fn shared_analysis_cache_does_not_retain_source_line_signatures() {
        let root = temp_root("cache-structural-only");
        std::fs::create_dir_all(&root).expect("root");
        let sentinel = "CACHE_SOURCE_SENTINEL_9f4b2b";
        std::fs::write(
            root.join("safe.rs"),
            format!("fn visible() {{}} // {sentinel}\n"),
        )
        .expect("write source");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let source = repository.read_source("safe.rs").expect("read source");
        let analysis = repository
            .analyze_source_cached(
                "safe.rs",
                &source.text,
                &source.stamp,
                None,
                Instant::now() + Duration::from_secs(1),
            )
            .expect("analysis succeeds")
            .expect("analysis result");
        assert!(
            analysis
                .symbols
                .iter()
                .any(|symbol| symbol.name == "visible")
        );

        let cache = repository.analysis_cache.lock().expect("analysis cache");
        let cached_debug = format!("{:?}", cache.entries.get("safe.rs"));
        assert!(!cached_debug.contains(sentinel));
    }

    #[test]
    fn candidate_generation_pruning_can_never_be_complete_no_match() {
        let root = temp_root("candidate-pruning-completeness");
        std::fs::create_dir_all(&root).expect("root");
        for index in 0..129 {
            std::fs::write(root.join(format!("candidate-{index:03}.rs")), "abc___bcd\n")
                .expect("write n-gram false positive");
        }

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let outcome = repository
            .search(&normalized_query("abcd"), 8, None)
            .expect("search succeeds");
        assert!(outcome.hits.is_empty());
        assert!(
            outcome.truncated,
            "candidate pruning must prevent complete NO_MATCH"
        );
        assert_eq!(
            outcome.coverage.adaptive_rounds, 1,
            "candidate-cap truncation alone must not waste scan-budget expansion rounds",
        );
    }

    #[test]
    fn path_match_is_returned_when_body_has_no_query_term() {
        let root = temp_root("path-match");
        std::fs::create_dir_all(root.join("src/auth")).expect("temp root");
        std::fs::write(
            root.join("src/auth/middleware.rs"),
            "pub fn verify_request() -> bool { true }\n",
        )
        .expect("write source");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let outcome = repository
            .search(&normalized_query("middleware gateway"), 8, None)
            .expect("search succeeds");
        assert_eq!(outcome.hits.len(), 1);
        assert_eq!(outcome.hits[0].relative_path, "src/auth/middleware.rs");
        assert!(outcome.hits[0].excerpt.is_empty());
        assert_eq!(
            (outcome.hits[0].start_line, outcome.hits[0].end_line),
            (0, 0)
        );
        assert_eq!(outcome.hits[0].score, 3.0);
    }

    #[test]
    fn search_redacts_model_visible_excerpt_without_redacting_every_source_read() {
        let root = temp_root("excerpt-redaction");
        std::fs::create_dir_all(&root).expect("temp root");
        let secret = "sk-abcdefghijklmnopqrstuvwxyz0123456789";
        std::fs::write(
            root.join("auth.rs"),
            format!("const AUTH_TOKEN: &str = \"{secret}\";\n"),
        )
        .expect("write source");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let source = repository.read_source("auth.rs").expect("read source");
        assert!(source.text.contains(secret));

        let outcome = repository
            .search(&normalized_query("AUTH_TOKEN credential"), 8, None)
            .expect("search succeeds");
        assert_eq!(outcome.hits.len(), 1);
        assert!(!outcome.hits[0].excerpt.contains(secret));
        assert!(outcome.hits[0].excerpt.contains("SIPPION_REDACTED_TOKEN"));
    }

    #[test]
    fn private_key_redaction_preserves_line_count_without_marker_amplification() {
        let input = concat!(
            "before\n",
            "-----BEGIN PRIVATE KEY-----\n",
            "a\n",
            "b\n",
            "-----END PRIVATE KEY-----\n",
            "after\n"
        );
        let redacted = redact_high_confidence_secrets(input);
        assert_eq!(redacted.lines().count(), input.lines().count());
        assert_eq!(redacted.matches("SIPPION_REDACTED_PRIVATE_KEY").count(), 1);
        assert!(!redacted.contains("\na\n"));
        assert!(!redacted.contains("\nb\n"));
        assert!(redacted.len() <= input.len() + 16);
    }

    #[test]
    fn content_matches_always_rank_above_path_only_matches() {
        let content = SearchHit {
            relative_path: "src/implementation.rs".into(),
            start_line: 1,
            end_line: 1,
            excerpt: "token".into(),
            score: (CONTENT_MATCH_BASE_SCORE + 10) as f64,
            source_stamp: None,
            source_fingerprint: None,
        };
        let path_only = SearchHit {
            relative_path: "authentication/token/validation/middleware.rs".into(),
            start_line: 0,
            end_line: 0,
            excerpt: String::new(),
            score: (MAX_QUERY_TERMS * 3) as f64,
            source_stamp: None,
            source_fingerprint: None,
        };
        assert!(content.score > path_only.score);
    }

    #[test]
    fn prefiltered_policy_paths_are_counted_for_completeness() {
        let root = temp_root("prefilter-policy-count");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::write(root.join("normal.rs"), "fn normal() {}\n").expect("normal");
        std::fs::write(root.join("Cargo.lock"), "pruned_only_marker = true\n").expect("lockfile");
        std::fs::write(root.join("image.png"), b"not-really-an-image").expect("binary extension");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let outcome = repository
            .search(&normalized_query("pruned_only_marker"), 8, None)
            .expect("search succeeds");
        assert!(outcome.hits.is_empty());
        assert!(outcome.coverage.policy_excluded_files >= 2);
        assert_eq!(outcome.coverage.confidence_milli, 350);
        assert!(!outcome.truncated);
    }

    #[test]
    fn dependency_lockfiles_and_generated_dirs_are_pruned() {
        assert!(is_pruned(Path::new("package-lock.json")));
        assert!(is_pruned(Path::new("ios/Pods/Library.swift")));
        assert!(is_pruned(Path::new("app/.gradle/cache.bin")));
        assert!(is_pruned(Path::new("cmake-build-debug/CMakeCache.txt")));
        assert!(!is_pruned(Path::new("src/package.rs")));
        assert!(!is_pruned(Path::new("Cargo.toml")));
    }

    #[test]
    fn top_k_candidate_pruning_is_not_an_incomplete_scan() {
        let mut hits = (0..130)
            .map(|index| SearchHit {
                relative_path: format!("src/path-{index}.rs"),
                start_line: 1,
                end_line: 1,
                excerpt: "path only".into(),
                score: 3.0,
                source_stamp: None,
                source_fingerprint: None,
            })
            .collect::<Vec<_>>();
        prune_candidates_if_needed(&mut hits, 64);
        assert!(hits.len() <= 64);
    }

    #[test]
    fn ram_index_reuses_unchanged_files_without_broad_reread() {
        let root = temp_root("ram-index-reuse");
        std::fs::create_dir_all(&root).expect("temp root");
        std::fs::write(root.join("a.rs"), "fn alpha() {}\n").expect("write a");
        std::fs::write(root.join("b.rs"), "fn beta() {}\n").expect("write b");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let first = repository
            .search(&normalized_query("zzzzmissing"), 8, None)
            .expect("first search");
        assert_eq!(first.coverage.indexed_files, first.coverage.eligible_files);
        assert!(first.coverage.scanned_files >= 2);

        let second = repository
            .search(&normalized_query("zzzzmissing"), 8, None)
            .expect("second search");
        assert_eq!(
            second.coverage.indexed_files,
            second.coverage.eligible_files
        );
        assert_eq!(second.coverage.scanned_files, 0);
        assert_eq!(second.coverage.scanned_bytes, 0);
    }

    #[test]
    fn changed_file_is_invalidated_and_reindexed() {
        let root = temp_root("ram-index-change");
        std::fs::create_dir_all(&root).expect("temp root");
        std::fs::write(root.join("service.rs"), "fn old_value() {}\n").expect("write source");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        repository
            .search(&normalized_query("needle"), 8, None)
            .expect("prime index");
        std::fs::write(
            root.join("service.rs"),
            "fn newly_changed_needle_handler() { println!(\"needle\"); }\n",
        )
        .expect("change source");

        let second = repository
            .search(&normalized_query("needle"), 8, None)
            .expect("changed search");
        assert!(second.coverage.scanned_files > 0);
        assert_eq!(
            second.hits.first().map(|hit| hit.relative_path.as_str()),
            Some("service.rs")
        );
    }

    #[test]
    fn ram_index_adds_identifier_subterms_without_storing_source_body() {
        let document = build_indexed_document("struct AuthTokenValidator;", None);
        let frequencies = indexed_query_frequencies(
            &document,
            &[(stable_term_hash("token"), query_substring_grams("token"))],
        );
        assert!(frequencies[0] > 0);
        assert!(document.terms.len() < 16);
    }

    #[test]
    fn ram_index_preserves_ascii_substring_candidate_recall() {
        let document = build_indexed_document("fn authenticate_request() {}", None);
        let query = [(stable_term_hash("auth"), query_substring_grams("auth"))];
        let frequencies = indexed_query_frequencies(&document, &query);
        assert_eq!(frequencies, vec![1]);
    }

    #[test]
    fn ram_index_preserves_unicode_substring_candidate_recall() {
        let document = build_indexed_document("fn ユーザー認証処理() {}", None);
        let query = [(stable_term_hash("認証"), query_substring_grams("認証"))];
        let frequencies = indexed_query_frequencies(&document, &query);
        assert_eq!(frequencies, vec![1]);
    }

    #[test]
    fn unicode_substring_search_reaches_exact_verification() {
        let root = temp_root("unicode-substring-recall");
        std::fs::create_dir_all(&root).expect("temp root");
        std::fs::write(
            root.join("auth.rs"),
            "fn ユーザー認証処理() { println!(\"認証済み\"); }\n",
        )
        .expect("write source");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let outcome = repository
            .search(&normalized_query("認証"), 8, None)
            .expect("unicode search");
        assert!(
            outcome
                .hits
                .iter()
                .any(|hit| hit.relative_path == "auth.rs"),
            "Unicode substring must not be filtered out by the RAM candidate index"
        );
    }

    #[test]
    fn ascii_substring_recall_survives_mixed_unicode_identifier() {
        let document = build_indexed_document("fn auth認証_handler() {}", None);
        let query = [(stable_term_hash("auth"), query_substring_grams("auth"))];
        let frequencies = indexed_query_frequencies(&document, &query);
        assert_eq!(frequencies, vec![1]);
    }

    #[test]
    fn index_flight_guard_releases_registration_during_unwind() {
        let root = temp_root("index-flight-unwind");
        std::fs::create_dir_all(&root).expect("temp root");
        let repository = RepositoryAccess::open(&root).expect("open repository");
        repository
            .index_inflight
            .lock()
            .expect("index inflight")
            .insert("src/a.rs".to_string());

        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = IndexFlightGuard::new(&repository, "src/a.rs".to_string());
            panic!("simulated index worker panic");
        }));

        assert!(unwind.is_err());
        assert!(
            !repository
                .index_inflight
                .lock()
                .expect("index inflight")
                .contains("src/a.rs")
        );
    }

    #[test]
    fn analysis_flight_guard_releases_registration_during_unwind() {
        let root = temp_root("analysis-flight-unwind");
        std::fs::create_dir_all(&root).expect("temp root");
        let repository = RepositoryAccess::open(&root).expect("open repository");
        repository
            .analysis_cache
            .lock()
            .expect("analysis cache")
            .inflight
            .insert("src/a.rs".to_string());

        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = AnalysisFlightGuard::new(&repository, "src/a.rs".to_string());
            panic!("simulated analysis worker panic");
        }));

        assert!(unwind.is_err());
        assert!(
            !repository
                .analysis_cache
                .lock()
                .expect("analysis cache")
                .inflight
                .contains("src/a.rs")
        );
    }

    #[test]
    fn broad_lane_round_robins_top_level_directories() {
        let pending = [
            "frontend/a.rs",
            "frontend/b.rs",
            "backend/a.rs",
            "backend/b.rs",
            "mobile/a.rs",
        ]
        .into_iter()
        .map(|path| PendingFile {
            file: DiscoveredFile {
                path: path.to_string(),
                stamp: None,
            },
            path_bonus: 0,
            changed: false,
        })
        .collect::<Vec<_>>();
        let (_, _, broad) = stratified_pending_lanes(pending);
        let first_three = broad
            .iter()
            .take(3)
            .map(|file| file.file.path.split('/').next().unwrap_or(""))
            .collect::<HashSet<_>>();
        assert_eq!(first_three.len(), 3);
    }
}
