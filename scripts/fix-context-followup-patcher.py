#!/usr/bin/env python3
from pathlib import Path

path = Path("scripts/apply-context-followup-patch.py")
text = path.read_text(encoding="utf-8")
start_marker = "old = '''                for import_path in &candidate.semantics.import_paths {"
end_marker = 'text = replace_once(text, old, new, "graph import matching")\n'
start = text.index(start_marker)
end = text.index(end_marker, start) + len(end_marker)
replacement = '''start = text.index("                for import_path in &candidate.semantics.import_paths {")
end = text.index("            let mut patterns = Vec::<String>::new();", start)
new = \'''                for import_path in &candidate.semantics.import_paths {
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
            }

\'''
text = text[:start] + new + text[end:]
'''
path.write_text(text[:start] + replacement + text[end:], encoding="utf-8")
