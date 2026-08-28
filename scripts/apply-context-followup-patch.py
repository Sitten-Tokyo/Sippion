#!/usr/bin/env python3
from pathlib import Path


def replace_once(text, old, new, label):
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected 1 match, got {count}")
    return text.replace(old, new, 1)


path = Path("src/repo/map.rs")
text = path.read_text(encoding="utf-8")
old = r'''fn normalized_module_name(value: &str) -> String {
    let mut normalized = crate::core::unicode_search_fold(value)
        .replace("::", "/")
        .replace(['.', '\\'], "/");
    while normalized.starts_with("./") || normalized.starts_with("../") {
        normalized = normalized
            .strip_prefix("./")
            .or_else(|| normalized.strip_prefix("../"))
            .unwrap_or(&normalized)
            .to_string();
    }
    for prefix in ["crate/", "self/", "super/"] {
        normalized = normalized
            .strip_prefix(prefix)
            .unwrap_or(&normalized)
            .to_string();
    }
    normalized.trim_matches('/').to_string()
}

fn normalized_path_module(path: &str) -> String {
    let folded = crate::core::unicode_search_fold(path).replace('\\', "/");
    let no_ext = Path::new(&folded)
        .with_extension("")
        .to_string_lossy()
        .replace('\\', "/");
    no_ext
        .trim_end_matches("/mod")
        .trim_end_matches("/index")
        .trim_matches('/')
        .to_string()
}
'''
new = r'''fn strip_relative_module_prefixes(mut value: String) -> String {
    while value.starts_with("./") || value.starts_with("../") {
        value = value
            .strip_prefix("./")
            .or_else(|| value.strip_prefix("../"))
            .unwrap_or(&value)
            .to_string();
    }
    value
}

fn normalized_slash_module(value: &str, strip_source_extension: bool) -> String {
    let mut normalized = strip_relative_module_prefixes(
        crate::core::unicode_search_fold(value).replace('\\', "/"),
    );
    if strip_source_extension {
        let known_extension = Path::new(&normalized)
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| {
                matches!(
                    extension,
                    "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts"
                        | "c" | "cc" | "cpp" | "cxx" | "h" | "hh" | "hpp" | "hxx"
                )
            });
        if known_extension {
            normalized = Path::new(&normalized)
                .with_extension("")
                .to_string_lossy()
                .replace('\\', "/");
        }
    }
    normalized.trim_matches('/').to_string()
}

fn normalized_module_name(value: &str) -> String {
    let mut normalized = crate::core::unicode_search_fold(value)
        .replace("::", "/")
        .replace(['.', '\\'], "/");
    normalized = normalized.trim_matches('/').to_string();
    for prefix in ["crate/", "self/", "super/"] {
        normalized = normalized
            .strip_prefix(prefix)
            .unwrap_or(&normalized)
            .to_string();
    }
    normalized.trim_matches('/').to_string()
}

fn normalized_import_module(seed_path: &str, value: &str) -> String {
    let extension = Path::new(seed_path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" | "c" | "cc"
        | "cpp" | "cxx" | "h" | "hh" | "hpp" | "hxx" => {
            normalized_slash_module(value, true)
        }
        "go" => normalized_slash_module(value, false),
        _ => normalized_module_name(value),
    }
}

fn normalized_path_module(path: &str) -> String {
    let folded = crate::core::unicode_search_fold(path).replace('\\', "/");
    let no_ext = Path::new(&folded)
        .with_extension("")
        .to_string_lossy()
        .replace('\\', "/");
    no_ext
        .trim_end_matches("/mod")
        .trim_end_matches("/index")
        .trim_end_matches("/__init__")
        .trim_matches('/')
        .to_string()
}

fn module_aliases_for_path(path: &str) -> Vec<String> {
    let module = normalized_path_module(path);
    if module.is_empty() {
        return Vec::new();
    }
    let mut bases = vec![module.clone()];
    if let Some((parent, _)) = module.rsplit_once('/') {
        if !parent.is_empty() {
            bases.push(parent.to_string());
        }
    }
    let mut aliases = Vec::new();
    for base in bases {
        let segments = base
            .split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        for start in segments.len().saturating_sub(4)..segments.len() {
            let alias = segments[start..].join("/");
            if alias.len() >= 2 {
                aliases.push(alias);
            }
        }
    }
    aliases.sort();
    aliases.dedup();
    aliases
}

fn content_keyed_analysis_path(path: &str, safe_source: &str) -> String {
    let fingerprint = source_content_fingerprint(safe_source);
    let source_path = Path::new(path);
    let extension = source_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let stem = source_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("source");
    let parent = source_path
        .parent()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let keyed_name = if extension.is_empty() {
        format!("{stem}.__sippion_{:016x}{:016x}", fingerprint.0, fingerprint.1)
    } else {
        format!(
            "{stem}.__sippion_{:016x}{:016x}.{extension}",
            fingerprint.0, fingerprint.1
        )
    };
    if parent.is_empty() {
        keyed_name
    } else {
        format!("{parent}/{keyed_name}")
    }
}
'''
text = replace_once(text, old, new, "module helper block")
text = replace_once(
    text,
    "        #[cfg(windows)]\n        self.reset_structural_caches()?;\n",
    "",
    "windows structural reset",
)
old = '''        let graph_key = GraphCacheKey(
            candidates
                .iter()
                .map(|candidate| GraphCacheNode {
                    path: candidate.relative_path.clone(),
                    stamp: candidate.stamp.clone(),
                })
                .collect(),
        );
'''
new = '''        let graph_key = GraphCacheKey(
            candidates
                .iter()
                .map(|candidate| GraphCacheNode {
                    // Graph reuse is content-keyed as well as stamp-keyed. This is required on
                    // Windows, where an in-place same-size/same-mtime rewrite can preserve the
                    // metadata identity visible to the stable API.
                    path: content_keyed_analysis_path(
                        &candidate.relative_path,
                        &candidate.source_lower,
                    ),
                    stamp: candidate.stamp.clone(),
                })
                .collect(),
        );
'''
text = replace_once(text, old, new, "graph key")
old = '''                for import_path in &candidate.semantics.import_paths {
                    let import_lower = crate::core::unicode_search_fold(import_path);
                    for (to, target) in candidates.iter().enumerate() {
                        if to == from {
                            continue;
                        }
                        let path = crate::core::unicode_search_fold(&target.relative_path);
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
'''
new = '''                for import_path in &candidate.semantics.import_paths {
                    let import_module =
                        normalized_import_module(&candidate.relative_path, import_path);
                    if import_module.is_empty() {
                        continue;
                    }
                    for (to, target) in candidates.iter().enumerate() {
                        if to == from {
                            continue;
                        }
                        let matched = module_aliases_for_path(&target.relative_path)
                            .iter()
                            .any(|alias| {
                                import_module == *alias
                                    || import_module.ends_with(&format!("/{alias}"))
                                    || alias.ends_with(&format!("/{import_module}"))
                            });
                        if matched {
                            upsert_repo_edge(&mut edge_maps, from, to, 0.40, "import");
                        }
                    }
                }
'''
text = replace_once(text, old, new, "graph import matching")
old = '''        let Some(analysis) = self.analyze_source_cached(
            path,
            &safe,
            &source.stamp,
            cancellation,
            *started + MAX_SEARCH_WALL_TIME,
        )?
'''
new = '''        // The source was verified and read before this point. Key structural analysis by a
        // content fingerprint while preserving the source extension used for language selection.
        // This permits safe cross-request analysis reuse even on Windows without trusting mtime.
        let analysis_path = content_keyed_analysis_path(path, &safe);
        let Some(analysis) = self.analyze_source_cached(
            &analysis_path,
            &safe,
            &source.stamp,
            cancellation,
            *started + MAX_SEARCH_WALL_TIME,
        )?
'''
text = replace_once(text, old, new, "analysis key")
old = '''            let module = normalized_path_module(path);
            if !module.is_empty() {
                by_module.entry(module).or_default().push(path.clone());
            }
'''
new = '''            for module in module_aliases_for_path(path) {
                by_module.entry(module).or_default().push(path.clone());
            }
'''
text = replace_once(text, old, new, "module aliases")
text = replace_once(
    text,
    "                let module = normalized_module_name(import);\n",
    "                let module = normalized_import_module(&seed.relative_path, import);\n",
    "seed import normalization",
)
marker = '''    #[test]
    fn import_neighbor_can_enter_structure_without_lexical_query_match() {
'''
tests = '''    #[test]
    fn language_aware_import_normalization_preserves_package_semantics() {
        assert_eq!(
            normalized_import_module("src/app.ts", "./dependency.ts"),
            "dependency"
        );
        assert_eq!(
            normalized_import_module("cmd/main.go", "github.com/acme/pkg"),
            "github.com/acme/pkg"
        );
        assert_eq!(
            normalized_import_module("src/main.rs", "crate::service::engine"),
            "service/engine"
        );
        assert_eq!(
            normalized_import_module("src/app.py", "package.module"),
            "package/module"
        );
    }

    #[test]
    fn module_aliases_cover_source_roots_and_package_directories() {
        let aliases = module_aliases_for_path("src/main/java/com/example/Foo.java");
        assert!(aliases.iter().any(|alias| alias == "com/example/foo"));
        assert!(aliases.iter().any(|alias| alias == "com/example"));
        assert!(module_aliases_for_path("src/dependency.ts")
            .iter()
            .any(|alias| alias == "dependency"));
    }

    #[test]
    fn structural_cache_key_changes_with_content_and_preserves_extension() {
        let first = content_keyed_analysis_path("src/main.rs", "fn first() {}");
        let second = content_keyed_analysis_path("src/main.rs", "fn second() {}");
        assert_ne!(first, second);
        assert!(first.ends_with(".rs"));
        assert!(second.ends_with(".rs"));
    }

'''
text = replace_once(text, marker, tests + marker, "new map tests")
path.write_text(text, encoding="utf-8")

access = Path("src/repo/access.rs")
text = access.read_text(encoding="utf-8")
old = '''
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
'''
text = replace_once(text, old, "\n", "remove structural reset")
access.write_text(text, encoding="utf-8")

repo = Path("src/repo.rs")
text = repo.read_text(encoding="utf-8")
old = '''    // The same metadata limitation applies to the structural analysis and graph caches. Serialize
    // top-level map construction before discarding those cross-request caches on Windows so a
    // same-size/same-mtime replacement can never reuse stale structural facts.
'''
new = '''    // Structural source is re-read on Windows and analysis/graph cache keys include a content
    // fingerprint. Serialize top-level map construction so those verified caches can be reused
    // across requests without stale same-size/same-mtime replacements racing one another.
'''
text = replace_once(text, old, new, "windows cache comment")
repo.write_text(text, encoding="utf-8")
