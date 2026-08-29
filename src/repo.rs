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
// Discovery remains hard-bounded, but large monorepos can exceed 50k eligible source files before
// a relevant package is reached. Keep the file/entry/path caps proportional so retrieval can cover
// those repositories without making source scanning or indexing unbounded.
pub const MAX_DISCOVERED_FILES: usize = 100_000;
pub const MAX_DISCOVERED_ENTRIES: usize = 200_000;
pub const MAX_DISCOVERED_PATH_BYTES: usize = 32 * 1024 * 1024;
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
/// Request-local source reuse is deliberately limited to small verified candidates. These snapshots
/// never enter a RepositoryAccess field or cross-request cache and disappear with one repo_context
/// call. Larger files keep the existing re-read path instead of increasing transient memory sharply.
const MAX_REQUEST_SNAPSHOT_SOURCE_BYTES: usize = 512 * 1024;
// Structural-map redaction must not amplify a bounded source file into a much larger retained
// buffer. Bounded redaction also refuses to run the allocating per-line redactors over an
// attacker-controlled giant/minified line; that line is suppressed instead.
const MAX_REPOSITORY_MAP_SOURCE_BYTES: usize = 32 * 1024 * 1024;
const MAX_BOUNDED_REDACTION_LINE_BYTES: usize = 64 * 1024;
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

include!("repo/types.rs");

mod access;
mod adaptive;
mod coordination;
mod map;
mod policy;
mod ranking;
mod redaction;
mod search;

use policy::*;
use ranking::*;
use redaction::*;

#[cfg(test)]
use self::redaction::REDACTED_OVERSIZE_LINE;

const SENSITIVE_LITERAL_KEYS: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "secret_key",
    "secretkey",
    "secret_key_base",
    "secretkeybase",
    "signing_key",
    "signingkey",
    "encryption_key",
    "encryptionkey",
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct RedactionOutcome {
    text: String,
    truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingSensitiveValue {
    indent: usize,
    allow_same_indent: bool,
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

#[cfg(feature = "fuzzing")]
pub(crate) fn fuzz_redact_bounded(text: &str, max_output_bytes: usize) -> (String, bool) {
    let outcome = redact_high_confidence_secrets_bounded(text, max_output_bytes);
    (outcome.text, outcome.truncated)
}

#[cfg(feature = "fuzzing")]
pub(crate) fn fuzz_path_disposition(path: &str) -> u8 {
    let Ok(normalized) = normalize_relative(Path::new(path)) else {
        return 0;
    };
    let normalized = Path::new(&normalized);
    if is_denied(normalized) {
        1
    } else if is_pruned(normalized) {
        2
    } else {
        3
    }
}

#[cfg(test)]
mod tests;
