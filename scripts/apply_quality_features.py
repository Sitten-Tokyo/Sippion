#!/usr/bin/env python3
from pathlib import Path
import textwrap


def read(path):
    return Path(path).read_text(encoding="utf-8")


def write(path, content):
    p = Path(path)
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(content, encoding="utf-8")


def replace_once(path, old, new):
    text = read(path)
    if text.count(old) != 1:
        raise SystemExit(f"expected one anchor in {path}, found {text.count(old)}: {old[:100]!r}")
    write(path, text.replace(old, new, 1))


# Shared capability metadata: one source of truth for diagnostics and registry-facing metadata.
replace_once(
    "src/core.rs",
    'pub const MAX_QUERY_BYTES: usize = 512;\n',
    '''pub const MAX_QUERY_BYTES: usize = 512;\npub const SUPPORTED_LANGUAGE_NAMES: &[&str] = &[\n    "Rust",\n    "Python",\n    "JavaScript",\n    "TypeScript",\n    "Go",\n    "Java",\n    "C#",\n    "C",\n    "C++",\n];\npub const MODEL_VISIBLE_CONTEXT_HARD_BYTES: &[usize] = &[8 * 1024, 16 * 1024, 24 * 1024, 32 * 1024];\n''',
)
replace_once(
    "src/core.rs",
    '        "schemaVersion": 3,\n        "agent": "sippion",\n        "trustLabels": ["local-only", "read-only", "no-network", "project-scoped"],\n',
    '''        "schemaVersion": 4,\n        "agent": "sippion",\n        "trustLabels": ["local-only", "read-only", "no-network", "project-scoped"],\n        "supportedLanguages": SUPPORTED_LANGUAGE_NAMES,\n        "modelVisibleContext": {\n            "adaptiveHardBytes": MODEL_VISIBLE_CONTEXT_HARD_BYTES,\n            "diagnosticsInToolOutput": false,\n            "diagnosticsSurface": "local CLI only"\n        },\n''',
)

# Protocol introspection is local CLI metadata, not an MCP tool field.
replace_once(
    "src/mcp.rs",
    'const LEGACY_MCP_VERSION: &str = "2025-11-25";\n',
    '''const LEGACY_MCP_VERSION: &str = "2025-11-25";\n\n#[must_use]\npub(crate) fn supported_protocol_versions() -> [&'static str; 2] {\n    [MODERN_MCP_VERSION, LEGACY_MCP_VERSION]\n}\n''',
)

# Human CLI: query uses the exact same RepositoryService path as MCP. inspect is static/local only.
write(
    "src/main.rs",
    textwrap.dedent(r'''\
        #![forbid(unsafe_code)]

        mod core;
        mod hybrid;
        mod managed;
        mod mcp;
        mod repo;
        mod root;
        mod service;
        mod setup;
        mod syntax;

        use std::ffi::OsString;
        use std::path::PathBuf;
        use std::sync::Arc;

        use crate::core::{
            McpToolInput, MODEL_VISIBLE_CONTEXT_HARD_BYTES, SUPPORTED_LANGUAGE_NAMES,
            heuristic_v3_estimated_tokens,
        };
        use crate::service::{
            LocalRepositoryService, MAX_CONFIGURED_SCAN_BYTES, MAX_SCAN_BYTES,
            MIN_CONFIGURED_SCAN_BYTES, RepositoryService,
        };

        fn main() {
            if let Err(message) = run() {
                eprintln!("Sippion: {message}");
                std::process::exit(2);
            }
        }

        fn run() -> Result<(), String> {
            let mut args = std::env::args_os();
            let _program = args.next();
            let Some(command) = args.next() else {
                print_help();
                return Ok(());
            };

            if command
                .to_str()
                .is_some_and(|value| matches!(value, "--version" | "-V"))
            {
                println!("{} {}", crate::core::PRODUCT_NAME, crate::core::VERSION);
                return Ok(());
            }
            if command
                .to_str()
                .is_some_and(|value| matches!(value, "--help" | "-h"))
            {
                print_help();
                return Ok(());
            }

            let rest = args.collect::<Vec<_>>();
            match command.to_str() {
                Some("setup") => {
                    if !rest.is_empty() {
                        return Err("setup does not accept arguments".to_string());
                    }
                    managed::run_setup()
                }
                Some("doctor") => run_doctor_command(&rest),
                Some("uninstall") => {
                    if !rest.is_empty() {
                        return Err("uninstall does not accept arguments".to_string());
                    }
                    managed::run_uninstall()
                }
                Some("inspect") => run_inspect_command(&rest),
                Some("query") => run_query_command(&rest),
                Some("mcp") => run_mcp_command(&rest),
                _ => Err(
                    "supported commands: mcp, query, inspect, setup, doctor, uninstall".to_string(),
                ),
            }
        }

        fn run_doctor_command(args: &[OsString]) -> Result<(), String> {
            let mut json_output = false;
            let mut verbose = false;
            for arg in args {
                match arg.to_str() {
                    Some("--json") if !json_output => json_output = true,
                    Some("--verbose") if !verbose => verbose = true,
                    Some("--json") => return Err("--json may be specified once".to_string()),
                    Some("--verbose") => return Err("--verbose may be specified once".to_string()),
                    _ => return Err(format!("unknown doctor argument: {}", arg.to_string_lossy())),
                }
            }
            if json_output && verbose {
                return Err("doctor --json and --verbose are mutually exclusive".to_string());
            }
            managed::run_doctor(json_output, verbose)
        }

        fn run_inspect_command(args: &[OsString]) -> Result<(), String> {
            let json_output = match args {
                [] => false,
                [arg] if arg.to_str() == Some("--json") => true,
                _ => return Err("inspect accepts only optional --json".to_string()),
            };
            let protocols = mcp::supported_protocol_versions();
            if json_output {
                let value = serde_json::json!({
                    "product": crate::core::PRODUCT_NAME,
                    "version": crate::core::VERSION,
                    "trustLabels": ["local-only", "read-only", "no-network", "project-scoped"],
                    "supportedLanguages": SUPPORTED_LANGUAGE_NAMES,
                    "mcpProtocolVersions": protocols,
                    "modelVisibleContextHardBytes": MODEL_VISIBLE_CONTEXT_HARD_BYTES,
                    "diagnosticsInRepoContext": false,
                    "capabilityRegistry": crate::core::capability_registry(),
                });
                println!("{}", serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?);
            } else {
                println!("Sippion {}", crate::core::VERSION);
                println!("trust: local-only, read-only, no-network, project-scoped");
                println!("languages: {}", SUPPORTED_LANGUAGE_NAMES.join(", "));
                println!("MCP protocols: {}", protocols.join(", "));
                println!(
                    "model-visible hard budgets: {}",
                    MODEL_VISIBLE_CONTEXT_HARD_BYTES
                        .iter()
                        .map(|value| value.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                println!("diagnostics in repo_context: no (local CLI only)");
            }
            Ok(())
        }

        #[derive(Debug)]
        struct RepositoryOptions {
            root: Option<PathBuf>,
            root_auto: bool,
            allow_broad_root: bool,
            scan_budget_bytes: usize,
        }

        impl Default for RepositoryOptions {
            fn default() -> Self {
                Self {
                    root: None,
                    root_auto: false,
                    allow_broad_root: false,
                    scan_budget_bytes: MAX_SCAN_BYTES,
                }
            }
        }

        fn parse_scan_budget(value: &OsString) -> Result<usize, String> {
            let mib = value
                .to_str()
                .ok_or_else(|| "--scan-budget-mib must be valid UTF-8".to_string())?
                .parse::<usize>()
                .map_err(|_| "--scan-budget-mib must be an integer".to_string())?;
            let bytes = mib
                .checked_mul(1024 * 1024)
                .ok_or_else(|| "--scan-budget-mib is too large".to_string())?;
            if !(MIN_CONFIGURED_SCAN_BYTES..=MAX_CONFIGURED_SCAN_BYTES).contains(&bytes) {
                return Err("--scan-budget-mib must be between 16 and 512".to_string());
            }
            Ok(bytes)
        }

        fn finalize_root(options: RepositoryOptions) -> Result<(PathBuf, usize), String> {
            if options.root.is_some() && options.root_auto {
                return Err("use exactly one of --root <project> or --root-auto".to_string());
            }
            if options.root.is_none() && !options.root_auto {
                return Err("requires --root <project> or --root-auto".to_string());
            }
            if options.root_auto && options.allow_broad_root {
                return Err("--allow-broad-root is valid only with explicit --root".to_string());
            }
            let root = if options.root_auto {
                root::infer_project_root_from_cwd()?
            } else {
                root::secure_explicit_root(
                    options.root.expect("validated explicit root"),
                    options.allow_broad_root,
                )?
            };
            Ok((root, options.scan_budget_bytes))
        }

        fn parse_repository_option(
            args: &[OsString],
            index: &mut usize,
            options: &mut RepositoryOptions,
        ) -> Result<bool, String> {
            let arg = &args[*index];
            match arg.to_str() {
                Some("--root") => {
                    if options.root.is_some() {
                        return Err("--root may be specified once".to_string());
                    }
                    *index += 1;
                    let value = args
                        .get(*index)
                        .ok_or_else(|| "--root requires a path".to_string())?;
                    options.root = Some(PathBuf::from(value));
                    Ok(true)
                }
                Some("--root-auto") => {
                    if options.root_auto {
                        return Err("--root-auto may be specified once".to_string());
                    }
                    options.root_auto = true;
                    Ok(true)
                }
                Some("--allow-broad-root") => {
                    if options.allow_broad_root {
                        return Err("--allow-broad-root may be specified once".to_string());
                    }
                    options.allow_broad_root = true;
                    Ok(true)
                }
                Some("--scan-budget-mib") => {
                    *index += 1;
                    let value = args
                        .get(*index)
                        .ok_or_else(|| "--scan-budget-mib requires an integer".to_string())?;
                    options.scan_budget_bytes = parse_scan_budget(value)?;
                    Ok(true)
                }
                _ => Ok(false),
            }
        }

        fn run_mcp_command(args: &[OsString]) -> Result<(), String> {
            let mut options = RepositoryOptions::default();
            let mut index = 0usize;
            while index < args.len() {
                if !parse_repository_option(args, &mut index, &mut options)? {
                    return Err(format!("unknown argument: {}", args[index].to_string_lossy()));
                }
                index += 1;
            }
            let (root, scan_budget_bytes) = finalize_root(options)
                .map_err(|error| format!("mcp {error}"))?;
            let service: Arc<dyn RepositoryService> = Arc::new(
                LocalRepositoryService::open_with_scan_budget(root, scan_budget_bytes)
                    .map_err(|_| "cannot secure project root".to_string())?,
            );
            mcp::serve_stdio(service).map_err(|error| format!("stdio MCP failed: {error}"))
        }

        #[derive(Debug, serde::Serialize)]
        struct RankedFileDiagnostic {
            path: String,
            rank: f64,
        }

        #[derive(Debug, serde::Serialize)]
        struct QueryDiagnostic {
            returned_bytes: usize,
            estimated_tokens: usize,
            hard_budget_bytes: Option<usize>,
            target_estimated_tokens: Option<usize>,
            scanned_bytes: Option<usize>,
            ranked_files: Vec<RankedFileDiagnostic>,
        }

        fn parse_numeric_field(line: &str, key: &str) -> Option<usize> {
            line.split_whitespace().find_map(|part| {
                part.strip_prefix(key)
                    .and_then(|value| value.parse::<usize>().ok())
            })
        }

        fn query_diagnostic(context: &str) -> QueryDiagnostic {
            let mut ranked_files = Vec::new();
            let mut hard_budget_bytes = None;
            let mut target_estimated_tokens = None;
            let mut scanned_bytes = None;
            for line in context.lines() {
                if line.starts_with("PACK ") {
                    hard_budget_bytes = parse_numeric_field(line, "hard_bytes=");
                    target_estimated_tokens = parse_numeric_field(line, "target_estimated_tokens=");
                } else if line.starts_with("COVERAGE ") {
                    scanned_bytes = parse_numeric_field(line, "scanned_bytes=");
                } else if let Some(rest) = line.strip_prefix("FILE path=") {
                    if let Some((path_json, rest)) = rest.split_once(" rank=") {
                        if let (Ok(path), Some(rank_text)) = (
                            serde_json::from_str::<String>(path_json),
                            rest.split_whitespace().next(),
                        ) {
                            if let Ok(rank) = rank_text.parse::<f64>() {
                                ranked_files.push(RankedFileDiagnostic { path, rank });
                            }
                        }
                    }
                }
            }
            QueryDiagnostic {
                returned_bytes: context.len(),
                estimated_tokens: heuristic_v3_estimated_tokens(context),
                hard_budget_bytes,
                target_estimated_tokens,
                scanned_bytes,
                ranked_files,
            }
        }

        fn run_query_command(args: &[OsString]) -> Result<(), String> {
            let mut options = RepositoryOptions::default();
            let mut json_output = false;
            let mut explain = false;
            let mut session_id = None;
            let mut agent_id = None;
            let mut query_parts = Vec::<String>::new();
            let mut index = 0usize;
            let mut after_separator = false;
            while index < args.len() {
                let arg = &args[index];
                if after_separator {
                    query_parts.push(
                        arg.to_str()
                            .ok_or_else(|| "query must be valid UTF-8".to_string())?
                            .to_string(),
                    );
                    index += 1;
                    continue;
                }
                if arg.to_str() == Some("--") {
                    after_separator = true;
                } else if parse_repository_option(args, &mut index, &mut options)? {
                } else {
                    match arg.to_str() {
                        Some("--json") if !json_output => json_output = true,
                        Some("--explain") if !explain => explain = true,
                        Some("--session-id") => {
                            index += 1;
                            session_id = Some(
                                args.get(index)
                                    .and_then(|value| value.to_str())
                                    .ok_or_else(|| "--session-id requires UTF-8 value".to_string())?
                                    .to_string(),
                            );
                        }
                        Some("--agent-id") => {
                            index += 1;
                            agent_id = Some(
                                args.get(index)
                                    .and_then(|value| value.to_str())
                                    .ok_or_else(|| "--agent-id requires UTF-8 value".to_string())?
                                    .to_string(),
                            );
                        }
                        _ => {
                            return Err(format!(
                                "unknown query argument before --: {}",
                                arg.to_string_lossy()
                            ));
                        }
                    }
                }
                index += 1;
            }
            if query_parts.is_empty() {
                return Err("query requires `-- <1-8 technical search terms>`".to_string());
            }
            let input = McpToolInput {
                q: query_parts.join(" "),
                session_id,
                agent_id,
            };
            let normalized = input
                .normalize()
                .map_err(|error| format!("invalid query: {error:?}"))?;
            let coordination = input
                .coordination()
                .map_err(|error| format!("invalid coordination id: {error:?}"))?;
            let (root, scan_budget_bytes) = finalize_root(options)
                .map_err(|error| format!("query {error}"))?;
            let service = LocalRepositoryService::open_with_scan_budget(root, scan_budget_bytes)
                .map_err(|_| "cannot secure project root".to_string())?;
            let context = service
                .context(&normalized, Some(&coordination), None)
                .map_err(|error| error.user_message().to_string())?;
            let diagnostic = query_diagnostic(&context);

            if json_output {
                let value = serde_json::json!({
                    "context": context,
                    "diagnostics": diagnostic,
                });
                println!("{}", serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?);
            } else {
                print!("{context}");
            }
            if explain {
                eprintln!(
                    "Sippion query diagnostics: bytes={} estimated_tokens={} hard_budget_bytes={} target_estimated_tokens={} scanned_bytes={} ranked_files={}",
                    diagnostic.returned_bytes,
                    diagnostic.estimated_tokens,
                    diagnostic
                        .hard_budget_bytes
                        .map_or_else(|| "unknown".to_string(), |value| value.to_string()),
                    diagnostic
                        .target_estimated_tokens
                        .map_or_else(|| "unknown".to_string(), |value| value.to_string()),
                    diagnostic
                        .scanned_bytes
                        .map_or_else(|| "unknown".to_string(), |value| value.to_string()),
                    diagnostic
                        .ranked_files
                        .iter()
                        .map(|entry| format!("{}@{:.3}", entry.path, entry.rank))
                        .collect::<Vec<_>>()
                        .join(","),
                );
            }
            Ok(())
        }

        fn print_help() {
            println!("Sippion {}", crate::core::VERSION);
            println!("Usage: sippion mcp --root <project> [--scan-budget-mib 16..512]");
            println!("       sippion mcp --root-auto [--scan-budget-mib 16..512]");
            println!("       sippion query --root <project>|--root-auto [--json] [--explain] -- <q>");
            println!("       sippion inspect [--json]");
            println!("       sippion setup");
            println!("       sippion doctor [--json|--verbose]");
            println!("       sippion uninstall");
            println!("  query runs the same bounded retrieval path as repo_context; diagnostics are opt-in.");
            println!("  inspect reports static local capabilities without reading repository source.");
            println!(
                "  --root-auto discovers a Git/project root from the current directory and refuses home/filesystem roots."
            );
            println!(
                "  --allow-broad-root permits an explicit home, home-ancestor, or filesystem root and is intentionally never used by setup."
            );
            println!(
                "  --scan-budget-mib sets the adaptive scan ceiling; retrieval normally starts at 32 MiB."
            );
            println!("  setup configures user-wide MCP settings and read-only discovery rules.");
            println!("  doctor checks current user-wide settings; --json is machine-readable and --verbose shows paths.");
            println!("  uninstall removes only Sippion-managed settings and rules.");
        }
    '''),
)

# Doctor: keep one source of truth for checks; only rendering changes by mode.
replace_once(
    "src/managed.rs",
    '''pub(crate) fn run_doctor() -> Result<(), String> {\n    let home = home_dir()?;\n    validate_managed_parent_boundaries(&home)?;\n    crate::setup::run_doctor()\n}\n''',
    '''pub(crate) fn run_doctor(json_output: bool, verbose: bool) -> Result<(), String> {\n    let home = home_dir()?;\n    validate_managed_parent_boundaries(&home)?;\n    crate::setup::run_doctor(json_output, verbose)\n}\n''',
)
replace_once(
    "src/setup.rs",
    '''impl CheckStatus {\n    fn is_ok(self) -> bool {\n        self == Self::Ok\n    }\n}\n''',
    '''impl CheckStatus {\n    fn is_ok(self) -> bool {\n        self == Self::Ok\n    }\n\n    fn as_str(self) -> &'static str {\n        match self {\n            Self::Ok => "ok",\n            Self::Missing => "missing",\n            Self::Mismatch => "mismatch",\n            Self::Error => "error",\n        }\n    }\n}\n\n#[derive(Debug)]\nstruct DoctorCheck {\n    label: &'static str,\n    path: PathBuf,\n    status: CheckStatus,\n}\n''',
)
old_doctor = '''pub fn run_doctor() -> Result<(), String> {\n    let executable = installed_executable()?;\n    let home = home_dir()?;\n    println!("Sippion {}", crate::core::VERSION);\n    println!("binary: {}", executable.display());\n    println!();\n    let statuses = [\n        check_codex(&home, &executable),\n        check_claude(&home, &executable),\n        check_antigravity(&home, &executable),\n        check_rule(&home.join(".codex").join("AGENTS.md"), "Codex"),\n        check_rule(&home.join(".claude").join("CLAUDE.md"), "Claude Code"),\n        check_rule(&home.join(".gemini").join("GEMINI.md"), "Antigravity"),\n    ];\n    let failures = statuses.iter().filter(|status| !status.is_ok()).count();\n    if failures == 0 {\n        Ok(())\n    } else {\n        Err(format!("doctor found {failures} configuration problem(s)"))\n    }\n}\n'''
new_doctor = '''pub fn run_doctor(json_output: bool, verbose: bool) -> Result<(), String> {\n    let executable = installed_executable()?;\n    let home = home_dir()?;\n    let checks = vec![\n        DoctorCheck {\n            label: "Codex MCP config",\n            path: home.join(".codex").join("config.toml"),\n            status: check_codex(&home, &executable),\n        },\n        DoctorCheck {\n            label: "Claude Code MCP config",\n            path: home.join(".claude.json"),\n            status: check_claude(&home, &executable),\n        },\n        DoctorCheck {\n            label: "Antigravity MCP config",\n            path: home.join(".gemini").join("config").join("mcp_config.json"),\n            status: check_antigravity(&home, &executable),\n        },\n        DoctorCheck {\n            label: "Codex global rule",\n            path: home.join(".codex").join("AGENTS.md"),\n            status: check_rule(&home.join(".codex").join("AGENTS.md")),\n        },\n        DoctorCheck {\n            label: "Claude Code global rule",\n            path: home.join(".claude").join("CLAUDE.md"),\n            status: check_rule(&home.join(".claude").join("CLAUDE.md")),\n        },\n        DoctorCheck {\n            label: "Antigravity global rule",\n            path: home.join(".gemini").join("GEMINI.md"),\n            status: check_rule(&home.join(".gemini").join("GEMINI.md")),\n        },\n    ];\n    let failures = checks.iter().filter(|check| !check.status.is_ok()).count();\n\n    if json_output {\n        let checks_json = checks\n            .iter()\n            .map(|check| {\n                json!({\n                    "name": check.label,\n                    "status": check.status.as_str(),\n                    "path": check.path.to_string_lossy(),\n                })\n            })\n            .collect::<Vec<_>>();\n        println!(\n            "{}",\n            serde_json::to_string_pretty(&json!({\n                "version": crate::core::VERSION,\n                "binary": executable.to_string_lossy(),\n                "ok": failures == 0,\n                "checks": checks_json,\n            }))\n            .map_err(|error| error.to_string())?\n        );\n    } else {\n        println!("Sippion {}", crate::core::VERSION);\n        println!("binary: {}", executable.display());\n        println!();\n        for check in &checks {\n            println!("{}: {}", check.label, check.status.as_str().to_ascii_uppercase());\n            if verbose {\n                println!("  path: {}", check.path.display());\n            }\n        }\n    }\n\n    if failures == 0 {\n        Ok(())\n    } else {\n        Err(format!("doctor found {failures} configuration problem(s)"))\n    }\n}\n'''
replace_once("src/setup.rs", old_doctor, new_doctor)

# Silence individual checks; run_doctor is the sole renderer.
for old, new in [
    ('                println!("Codex MCP config: ERROR (malformed Sippion managed markers)");\n                return CheckStatus::Error;\n', '                return CheckStatus::Error;\n'),
    ('                println!("Codex MCP config: MISSING");\n                return CheckStatus::Missing;\n', '                return CheckStatus::Missing;\n'),
    ('            println!("Codex MCP config: {}", if ok { "OK" } else { "MISMATCH" });\n            if ok {\n', '            if ok {\n'),
    ('        Ok(None) => {\n            println!("Codex MCP config: MISSING");\n            CheckStatus::Missing\n        }\n        Err(error) => {\n            println!("Codex MCP config: ERROR ({error})");\n            CheckStatus::Error\n        }\n', '        Ok(None) => CheckStatus::Missing,\n        Err(_) => CheckStatus::Error,\n'),
    ('                println!("{label}: MISSING");\n                return CheckStatus::Missing;\n', '                return CheckStatus::Missing;\n'),
    ('            println!("{label}: {}", if ok { "OK" } else { "MISMATCH" });\n            if ok {\n', '            if ok {\n'),
    ('        Ok(None) => {\n            println!("{label}: MISSING");\n            CheckStatus::Missing\n        }\n        Err(error) => {\n            println!("{label}: ERROR ({error})");\n            CheckStatus::Error\n        }\n', '        Ok(None) => CheckStatus::Missing,\n        Err(_) => CheckStatus::Error,\n'),
]:
    replace_once("src/setup.rs", old, new)
replace_once(
    "src/setup.rs",
    'fn check_json_server(path: &Path, executable: &Path, label: &str, claude: bool) -> CheckStatus {\n',
    'fn check_json_server(path: &Path, executable: &Path, _label: &str, claude: bool) -> CheckStatus {\n',
)
old_rule = '''fn check_rule(path: &Path, label: &str) -> CheckStatus {\n    let status = match read_optional_text(path) {\n        Ok(Some(contents)) => match managed_block_range(&contents, RULE_BEGIN, RULE_END) {\n            Ok(Some(_)) => CheckStatus::Ok,\n            Ok(None) => CheckStatus::Missing,\n            Err(_) => CheckStatus::Error,\n        },\n        Ok(None) => CheckStatus::Missing,\n        Err(_) => CheckStatus::Error,\n    };\n    let label_status = match status {\n        CheckStatus::Ok => "OK",\n        CheckStatus::Missing => "MISSING",\n        CheckStatus::Mismatch => "MISMATCH",\n        CheckStatus::Error => "ERROR",\n    };\n    println!("{label} global rule: {label_status}");\n    status\n}\n'''
new_rule = '''fn check_rule(path: &Path) -> CheckStatus {\n    match read_optional_text(path) {\n        Ok(Some(contents)) => match managed_block_range(&contents, RULE_BEGIN, RULE_END) {\n            Ok(Some(_)) => CheckStatus::Ok,\n            Ok(None) => CheckStatus::Missing,\n            Err(_) => CheckStatus::Error,\n        },\n        Ok(None) => CheckStatus::Missing,\n        Err(_) => CheckStatus::Error,\n    }\n}\n'''
replace_once("src/setup.rs", old_rule, new_rule)

# Retrieval evaluation: deterministic corpus with explicit context/token gates.
write(
    "eval/cases.json",
    '''{\n  "minRecallAt5": 1.0,\n  "minMrr": 1.0,\n  "maxAverageEstimatedTokens": 2100,\n  "cases": [\n    {"name":"rust-auth","query":"validate_session_token","expectedPaths":["src/auth.rs"],"maxReturnedBytes":8192,"maxEstimatedTokens":2100},\n    {"name":"python-router","query":"RouteDispatcher","expectedPaths":["python/router.py"],"maxReturnedBytes":8192,"maxEstimatedTokens":2100},\n    {"name":"java-account","query":"JavaAccountService","expectedPaths":["java/AccountService.java"],"maxReturnedBytes":8192,"maxEstimatedTokens":2100},\n    {"name":"csharp-token","query":"CSharpTokenValidator","expectedPaths":["csharp/TokenValidator.cs"],"maxReturnedBytes":8192,"maxEstimatedTokens":2100},\n    {"name":"c-packet","query":"normalize_packet_header","expectedPaths":["c/packet.c"],"maxReturnedBytes":8192,"maxEstimatedTokens":2100},\n    {"name":"cpp-cache","query":"native_cache_probe","expectedPaths":["cpp/cache.cpp"],"maxReturnedBytes":8192,"maxEstimatedTokens":2100}\n  ]\n}\n''',
)
fixtures = {
    "eval/fixture/src/auth.rs": "pub fn validate_session_token(token: &str) -> bool { !token.is_empty() }\npub fn unrelated_auth_helper() {}\n",
    "eval/fixture/src/noise.rs": "pub fn session_archive_rotation() {}\npub fn token_bucket_metrics() {}\n",
    "eval/fixture/python/router.py": "class RouteDispatcher:\n    def dispatch(self, route):\n        return route\n\ndef unrelated_router_metric():\n    return 1\n",
    "eval/fixture/java/AccountService.java": "public final class JavaAccountService {\n    public String loadAccount(String id) { return id; }\n}\n",
    "eval/fixture/csharp/TokenValidator.cs": "public sealed class CSharpTokenValidator {\n    public bool Validate(string token) => token.Length > 0;\n}\n",
    "eval/fixture/c/packet.c": "int normalize_packet_header(int value) { return value & 255; }\n",
    "eval/fixture/cpp/cache.cpp": "int native_cache_probe(int key) { return key * 2; }\n",
    "eval/fixture/README.md": "Frozen deterministic retrieval evaluation fixture for Sippion.\n",
}
for path, content in fixtures.items():
    write(path, content)

write(
    "scripts/retrieval-eval.py",
    textwrap.dedent(r'''\
        #!/usr/bin/env python3
        import argparse
        import json
        import statistics
        import subprocess
        import time
        from pathlib import Path

        def percentile(values, p):
            if not values:
                return 0.0
            ordered = sorted(values)
            index = (len(ordered) - 1) * p
            low = int(index)
            high = min(low + 1, len(ordered) - 1)
            fraction = index - low
            return ordered[low] * (1 - fraction) + ordered[high] * fraction

        def main():
            parser = argparse.ArgumentParser()
            parser.add_argument("--binary", default="target/release/sippion")
            parser.add_argument("--cases", default="eval/cases.json")
            parser.add_argument("--fixture", default="eval/fixture")
            args = parser.parse_args()
            config = json.loads(Path(args.cases).read_text(encoding="utf-8"))
            results = []
            failures = []
            reciprocal_ranks = []
            recall_hits = 0

            for case in config["cases"]:
                started = time.perf_counter()
                proc = subprocess.run(
                    [args.binary, "query", "--root", args.fixture, "--json", "--", case["query"]],
                    text=True,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    check=False,
                )
                elapsed_ms = (time.perf_counter() - started) * 1000.0
                if proc.returncode != 0:
                    failures.append(f"{case['name']}: query failed: {proc.stderr.strip()}")
                    continue
                payload = json.loads(proc.stdout)
                diagnostic = payload["diagnostics"]
                ranked = [entry["path"] for entry in diagnostic["ranked_files"]]
                rank = next((i + 1 for i, path in enumerate(ranked) if path in case["expectedPaths"]), None)
                if rank is not None and rank <= 5:
                    recall_hits += 1
                reciprocal_ranks.append(0.0 if rank is None else 1.0 / rank)
                if rank is None:
                    failures.append(f"{case['name']}: expected path absent from ranked files: {ranked}")
                if diagnostic["returned_bytes"] > case["maxReturnedBytes"]:
                    failures.append(
                        f"{case['name']}: returned bytes {diagnostic['returned_bytes']} > {case['maxReturnedBytes']}"
                    )
                if diagnostic["estimated_tokens"] > case["maxEstimatedTokens"]:
                    failures.append(
                        f"{case['name']}: estimated tokens {diagnostic['estimated_tokens']} > {case['maxEstimatedTokens']}"
                    )
                results.append({
                    "name": case["name"],
                    "rank": rank,
                    "returnedBytes": diagnostic["returned_bytes"],
                    "estimatedTokens": diagnostic["estimated_tokens"],
                    "elapsedMs": round(elapsed_ms, 2),
                })

            count = len(config["cases"])
            recall = recall_hits / count if count else 0.0
            mrr = sum(reciprocal_ranks) / count if count else 0.0
            avg_tokens = statistics.fmean(r["estimatedTokens"] for r in results) if results else 0.0
            latencies = [r["elapsedMs"] for r in results]
            if recall < config["minRecallAt5"]:
                failures.append(f"Recall@5 {recall:.3f} < {config['minRecallAt5']:.3f}")
            if mrr < config["minMrr"]:
                failures.append(f"MRR {mrr:.3f} < {config['minMrr']:.3f}")
            if avg_tokens > config["maxAverageEstimatedTokens"]:
                failures.append(
                    f"average estimated tokens {avg_tokens:.1f} > {config['maxAverageEstimatedTokens']}"
                )
            summary = {
                "recallAt5": round(recall, 4),
                "mrr": round(mrr, 4),
                "averageEstimatedTokens": round(avg_tokens, 2),
                "p50LatencyMs": round(percentile(latencies, 0.50), 2),
                "p95LatencyMs": round(percentile(latencies, 0.95), 2),
                "cases": results,
                "failures": failures,
            }
            print(json.dumps(summary, indent=2))
            raise SystemExit(1 if failures else 0)

        if __name__ == "__main__":
            main()
    '''),
)

# External official MCP client conformance test (black-box stdio).
write(
    "scripts/mcp-conformance.mjs",
    textwrap.dedent(r'''\
        import path from 'node:path';
        import process from 'node:process';
        import { Client } from '@modelcontextprotocol/client';
        import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';

        const binary = path.resolve(process.argv[2] ?? 'target/release/sippion');
        const root = path.resolve(process.argv[3] ?? 'eval/fixture');

        function assert(condition, message) {
          if (!condition) throw new Error(message);
        }

        async function runClient(options, expectedEra) {
          const client = new Client(
            { name: `sippion-conformance-${expectedEra}`, version: '1.0.0' },
            options,
          );
          const transport = new StdioClientTransport({
            command: binary,
            args: ['mcp', '--root', root],
            stderr: 'pipe',
          });
          try {
            await client.connect(transport);
            assert(client.getProtocolEra() === expectedEra, `expected ${expectedEra} era`);
            const listed = await client.listTools();
            assert(listed.tools.length === 1, 'Sippion must expose exactly one MCP tool');
            const tool = listed.tools[0];
            assert(tool.name === 'repo_context', 'repo_context tool missing');
            assert(tool.annotations?.readOnlyHint === true, 'repo_context must advertise readOnlyHint');
            const result = await client.callTool({
              name: 'repo_context',
              arguments: { q: 'validate_session_token' },
            });
            const text = result.content?.find((item) => item.type === 'text')?.text ?? '';
            assert(text.includes('src/auth.rs'), 'repo_context did not return expected fixture path');
            assert(text.includes('PACK adaptive=true'), 'bounded adaptive pack metadata missing');
          } finally {
            await client.close();
          }
        }

        await runClient(
          { versionNegotiation: { mode: { pin: '2026-07-28' } } },
          'modern',
        );
        await runClient({}, 'legacy');
        console.log('official MCP client conformance: modern + legacy stdio OK');
    '''),
)

# MCPB assembly helpers. Manifest keeps project root an explicit user-selected directory.
write(
    "scripts/prepare-mcpb.py",
    textwrap.dedent(r'''\
        #!/usr/bin/env python3
        import argparse
        import json
        import os
        import shutil
        from pathlib import Path

        parser = argparse.ArgumentParser()
        parser.add_argument('--binary', required=True)
        parser.add_argument('--platform', choices=['linux', 'darwin', 'win32'], required=True)
        parser.add_argument('--version', required=True)
        parser.add_argument('--output-dir', required=True)
        args = parser.parse_args()

        output = Path(args.output_dir)
        server = output / 'server'
        server.mkdir(parents=True, exist_ok=True)
        binary_name = 'sippion.exe' if args.platform == 'win32' else 'sippion'
        destination = server / binary_name
        shutil.copy2(args.binary, destination)
        if args.platform != 'win32':
            os.chmod(destination, 0o755)

        manifest = {
            'manifest_version': '0.3',
            'name': 'sippion',
            'display_name': 'Sippion',
            'version': args.version,
            'description': 'Local read-only MCP repository context retrieval for AI coding agents.',
            'long_description': 'Sippion narrows repository-wide discovery to bounded ranked structural context before agents broadly open source files. It is local-only, read-only, no-network while serving, and project-scoped.',
            'author': {'name': 'Sitten-Tokyo'},
            'repository': {'type': 'git', 'url': 'https://github.com/Sitten-Tokyo/Sippion.git'},
            'documentation': 'https://github.com/Sitten-Tokyo/Sippion#readme',
            'support': 'https://github.com/Sitten-Tokyo/Sippion/issues',
            'server': {
                'type': 'binary',
                'entry_point': f'server/{binary_name}',
                'mcp_config': {
                    'command': f'${{__dirname}}/server/{binary_name}',
                    'args': ['mcp', '--root', '${user_config.project_root}'],
                    'env': {},
                },
            },
            'tools': [{
                'name': 'repo_context',
                'description': 'Return bounded ranked structural repository context and source evidence.',
            }],
            'keywords': ['repository', 'coding-agent', 'context', 'search', 'local', 'read-only'],
            'license': 'MIT OR Apache-2.0',
            'compatibility': {'platforms': [args.platform]},
            'user_config': {
                'project_root': {
                    'type': 'directory',
                    'title': 'Project root',
                    'description': 'Repository or project directory that Sippion may read.',
                    'required': True,
                },
            },
        }
        (output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
    '''),
)
write(
    "scripts/generate-server-json.py",
    textwrap.dedent(r'''\
        #!/usr/bin/env python3
        import argparse
        import hashlib
        import json
        from pathlib import Path

        parser = argparse.ArgumentParser()
        parser.add_argument('--assets-dir', required=True)
        parser.add_argument('--tag', required=True)
        parser.add_argument('--version', required=True)
        parser.add_argument('--output', required=True)
        args = parser.parse_args()
        assets = Path(args.assets_dir)
        names = [
            'sippion-linux-x86_64.mcpb',
            'sippion-windows-x86_64.mcpb',
            'sippion-macos-aarch64.mcpb',
            'sippion-macos-x86_64.mcpb',
        ]
        packages = []
        for name in names:
            content = (assets / name).read_bytes()
            digest = hashlib.sha256(content).hexdigest()
            packages.append({
                'registryType': 'mcpb',
                'identifier': f'https://github.com/Sitten-Tokyo/Sippion/releases/download/{args.tag}/{name}',
                'fileSha256': digest,
                'transport': {'type': 'stdio'},
            })
        value = {
            '$schema': 'https://static.modelcontextprotocol.io/schemas/2025-12-11/server.schema.json',
            'name': 'io.github.Sitten-Tokyo/sippion',
            'title': 'Sippion',
            'description': 'Local read-only repository context retrieval for AI coding agents.',
            'repository': {'url': 'https://github.com/Sitten-Tokyo/Sippion.git', 'source': 'github'},
            'version': args.version,
            'packages': packages,
        }
        Path(args.output).write_text(json.dumps(value, indent=2) + '\n', encoding='utf-8')
    '''),
)

# README/CHANGELOG discoverability.
replace_once(
    "README.md",
    'Sippion exposes one tool, `repo_context`,',
    'Sippion exposes one MCP tool, `repo_context`,',
)
# Add CLI diagnostics section before architecture/detail material if anchor exists.
readme = read("README.md")
anchor = "## Security"
if anchor in readme and "## Local diagnostics" not in readme:
    diagnostic_docs = '''## Local diagnostics\n\nDiagnostics are deliberately outside the model-visible MCP tool output. `sippion query --root <project> --json -- <terms>` runs the same bounded retrieval path as `repo_context`; add `--explain` for local ranking/budget diagnostics. `sippion inspect --json` reports static capabilities, supported languages, MCP protocol versions, and context budgets. `sippion doctor --json` provides machine-readable client setup checks, while `--verbose` adds managed paths to the human report.\n\nRetrieval quality is regression-tested against a frozen corpus with Recall@5, MRR, returned-byte, estimated-token, and latency reporting.\n\n'''
    readme = readme.replace(anchor, diagnostic_docs + anchor, 1)
    write("README.md", readme)

changelog = read("CHANGELOG.md")
unreleased = "## [Unreleased]\n"
if unreleased in changelog and "Retrieval evaluation" not in changelog:
    addition = '''## [Unreleased]\n\n### Added\n\n- Retrieval evaluation with Recall@5/MRR and model-visible byte/token regression gates.\n- Opt-in `query`, `inspect`, and machine-readable/verbose Doctor diagnostics without expanding `repo_context`.\n- Black-box MCP conformance checks using the official MCP client implementation.\n- Per-platform MCPB release packaging, generated `server.json`, and post-release Official MCP Registry publication via GitHub OIDC.\n'''
    changelog = changelog.replace(unreleased, addition, 1)
    write("CHANGELOG.md", changelog)

# CI: external client and retrieval eval become part of the already-required Linux x86_64 check.
replace_once(
    ".github/workflows/ci.yml",
    '''      - name: Run Clippy\n        run: cargo clippy --all-targets --all-features --locked -- -D warnings\n''',
    '''      - name: Run Clippy\n        run: cargo clippy --all-targets --all-features --locked -- -D warnings\n\n      - name: Run retrieval quality and context budget evaluation\n        if: runner.os == 'Linux'\n        run: python3 scripts/retrieval-eval.py --binary target/release/sippion\n\n      - name: Set up pinned Node.js for official MCP client\n        if: runner.os == 'Linux'\n        uses: actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020 # v4.4.0\n        with:\n          node-version: '22.19.0'\n\n      - name: Install exact official MCP client\n        if: runner.os == 'Linux'\n        run: npm install --no-save --ignore-scripts --no-audit --no-fund @modelcontextprotocol/client@2.0.0\n\n      - name: Run external MCP protocol conformance\n        if: runner.os == 'Linux'\n        run: node scripts/mcp-conformance.mjs target/release/sippion eval/fixture\n''',
)

# Release packaging: exact MCPB CLI, generated per-platform bundles and server.json.
replace_once(
    ".github/workflows/release-draft.yml",
    '''      - name: Install pinned SBOM generator\n        run: cargo install cargo-cyclonedx --version '=0.5.9' --locked\n''',
    '''      - name: Set up pinned Node.js for MCPB tooling\n        uses: actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020 # v4.4.0\n        with:\n          node-version: '22.19.0'\n\n      - name: Install pinned SBOM generator\n        run: cargo install cargo-cyclonedx --version '=0.5.9' --locked\n''',
)
replace_once(
    ".github/workflows/release-draft.yml",
    '''      - name: Assemble and verify release assets\n        shell: bash\n        run: |\n          set -euo pipefail\n          mkdir -p release-assets\n''',
    '''      - name: Build and validate per-platform MCPB packages\n        env:\n          RELEASE_TAG: ${{ needs.prepare-release.outputs.tag }}\n        shell: bash\n        run: |\n          set -euo pipefail\n          version=${RELEASE_TAG#v}\n          mkdir -p release-assets mcpb-work\n          python3 scripts/prepare-mcpb.py --binary downloaded/sippion-linux-x86_64/sippion-linux-x86_64 --platform linux --version "$version" --output-dir mcpb-work/linux\n          python3 scripts/prepare-mcpb.py --binary downloaded/sippion-windows-x86_64.exe/sippion-windows-x86_64.exe --platform win32 --version "$version" --output-dir mcpb-work/windows\n          python3 scripts/prepare-mcpb.py --binary downloaded/sippion-macos-aarch64/sippion-macos-aarch64 --platform darwin --version "$version" --output-dir mcpb-work/macos-aarch64\n          python3 scripts/prepare-mcpb.py --binary downloaded/sippion-macos-x86_64/sippion-macos-x86_64 --platform darwin --version "$version" --output-dir mcpb-work/macos-x86_64\n          for spec in \\n            'linux:sippion-linux-x86_64.mcpb' \\n            'windows:sippion-windows-x86_64.mcpb' \\n            'macos-aarch64:sippion-macos-aarch64.mcpb' \\n            'macos-x86_64:sippion-macos-x86_64.mcpb'; do\n            dir=${spec%%:*}\n            file=${spec#*:}\n            npx --yes @anthropic-ai/mcpb@2.1.2 validate "mcpb-work/$dir"\n            npx --yes @anthropic-ai/mcpb@2.1.2 pack "mcpb-work/$dir" "release-assets/$file"\n            test -s "release-assets/$file"\n          done\n          python3 scripts/generate-server-json.py --assets-dir release-assets --tag "$RELEASE_TAG" --version "$version" --output release-assets/server.json\n          test -s release-assets/server.json\n\n      - name: Validate generated Official MCP Registry metadata\n        shell: bash\n        run: |\n          set -euo pipefail\n          curl --fail --location --silent --show-error \\n            --output /tmp/mcp-publisher.tar.gz \\n            https://github.com/modelcontextprotocol/registry/releases/download/v1.8.1/mcp-publisher_linux_amd64.tar.gz\n          echo 'a06c9096dcb9727c13555b6be26c7effa707b01f06a4c561ba7a3635443cf2cc  /tmp/mcp-publisher.tar.gz' | sha256sum -c -\n          tar -xzf /tmp/mcp-publisher.tar.gz -C /tmp mcp-publisher\n          /tmp/mcp-publisher validate release-assets/server.json\n\n      - name: Assemble and verify release assets\n        shell: bash\n        run: |\n          set -euo pipefail\n          mkdir -p release-assets\n''',
)
replace_once(
    ".github/workflows/release-draft.yml",
    '''            sha256sum sippion.cdx.json > sippion.cdx.json.sha256\n            cat install.sh.sha256 install.ps1.sha256 sippion.cdx.json.sha256 >> SHA256SUMS\n            sha256sum -c SHA256SUMS\n''',
    '''            sha256sum sippion.cdx.json > sippion.cdx.json.sha256\n            sha256sum server.json > server.json.sha256\n            for bundle in *.mcpb; do\n              sha256sum "$bundle" > "$bundle.sha256"\n            done\n            cat install.sh.sha256 install.ps1.sha256 sippion.cdx.json.sha256 server.json.sha256 *.mcpb.sha256 >> SHA256SUMS\n            sha256sum -c SHA256SUMS\n''',
)
replace_once(
    ".github/workflows/release-draft.yml",
    '''          test -s release-assets/sippion.cdx.json\n          test -s release-assets/sippion.cdx.json.sha256\n\n      - name: Attest installer and SBOM provenance\n''',
    '''          test -s release-assets/sippion.cdx.json\n          test -s release-assets/sippion.cdx.json.sha256\n          test -s release-assets/server.json\n          test -s release-assets/server.json.sha256\n          for bundle in release-assets/*.mcpb; do\n            test -s "$bundle"\n            test -s "$bundle.sha256"\n          done\n\n      - name: Attest installer, SBOM, MCPB, and registry metadata provenance\n''',
)
replace_once(
    ".github/workflows/release-draft.yml",
    '''            release-assets/install.ps1\n            release-assets/sippion.cdx.json\n''',
    '''            release-assets/install.ps1\n            release-assets/sippion.cdx.json\n            release-assets/server.json\n            release-assets/sippion-linux-x86_64.mcpb\n            release-assets/sippion-windows-x86_64.mcpb\n            release-assets/sippion-macos-aarch64.mcpb\n            release-assets/sippion-macos-x86_64.mcpb\n''',
)

# Supply-chain smoke exercises the same MCPB/server.json generation without touching a release.
replace_once(
    ".github/workflows/release-supply-chain-smoke.yml",
    '''      - scripts/install.ps1\n      - .github/workflows/release-build.yml\n''',
    '''      - scripts/install.ps1\n      - scripts/prepare-mcpb.py\n      - scripts/generate-server-json.py\n      - .github/workflows/mcp-registry-publish.yml\n      - .github/workflows/release-build.yml\n''',
)
replace_once(
    ".github/workflows/release-supply-chain-smoke.yml",
    '''      - name: Generate and validate CycloneDX SBOM\n        shell: bash\n''',
    '''      - name: Set up pinned Node.js for MCPB tooling\n        uses: actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020 # v4.4.0\n        with:\n          node-version: '22.19.0'\n\n      - name: Generate and validate CycloneDX SBOM\n        shell: bash\n''',
)
replace_once(
    ".github/workflows/release-supply-chain-smoke.yml",
    '''      - name: Stage installer assets and checksums\n        shell: bash\n        run: |\n          set -euo pipefail\n          mkdir -p installer-smoke\n''',
    '''      - name: Build and validate MCPB plus registry metadata\n        shell: bash\n        run: |\n          set -euo pipefail\n          version=$(awk -F'"' '/^version = "/ { print $2; exit }' Cargo.toml)\n          tag="v$version"\n          mkdir -p mcpb-work registry-smoke\n          python3 scripts/prepare-mcpb.py --binary downloaded/sippion-linux-x86_64/sippion-linux-x86_64 --platform linux --version "$version" --output-dir mcpb-work/linux\n          python3 scripts/prepare-mcpb.py --binary downloaded/sippion-windows-x86_64.exe/sippion-windows-x86_64.exe --platform win32 --version "$version" --output-dir mcpb-work/windows\n          python3 scripts/prepare-mcpb.py --binary downloaded/sippion-macos-aarch64/sippion-macos-aarch64 --platform darwin --version "$version" --output-dir mcpb-work/macos-aarch64\n          python3 scripts/prepare-mcpb.py --binary downloaded/sippion-macos-x86_64/sippion-macos-x86_64 --platform darwin --version "$version" --output-dir mcpb-work/macos-x86_64\n          for spec in \\n            'linux:sippion-linux-x86_64.mcpb' \\n            'windows:sippion-windows-x86_64.mcpb' \\n            'macos-aarch64:sippion-macos-aarch64.mcpb' \\n            'macos-x86_64:sippion-macos-x86_64.mcpb'; do\n            dir=${spec%%:*}\n            file=${spec#*:}\n            npx --yes @anthropic-ai/mcpb@2.1.2 validate "mcpb-work/$dir"\n            npx --yes @anthropic-ai/mcpb@2.1.2 pack "mcpb-work/$dir" "registry-smoke/$file"\n          done\n          python3 scripts/generate-server-json.py --assets-dir registry-smoke --tag "$tag" --version "$version" --output registry-smoke/server.json\n          curl --fail --location --silent --show-error \\n            --output /tmp/mcp-publisher.tar.gz \\n            https://github.com/modelcontextprotocol/registry/releases/download/v1.8.1/mcp-publisher_linux_amd64.tar.gz\n          echo 'a06c9096dcb9727c13555b6be26c7effa707b01f06a4c561ba7a3635443cf2cc  /tmp/mcp-publisher.tar.gz' | sha256sum -c -\n          tar -xzf /tmp/mcp-publisher.tar.gz -C /tmp mcp-publisher\n          /tmp/mcp-publisher validate registry-smoke/server.json\n          for bundle in registry-smoke/*.mcpb; do\n            sha256sum "$bundle" > "$bundle.sha256"\n            sha256sum -c "$bundle.sha256"\n          done\n          sha256sum registry-smoke/server.json > registry-smoke/server.json.sha256\n          sha256sum -c registry-smoke/server.json.sha256\n\n      - name: Stage installer assets and checksums\n        shell: bash\n        run: |\n          set -euo pipefail\n          mkdir -p installer-smoke\n''',
)
# Future release/workflow_run verification requires new assets; PR verification of immutable rc.35 remains compatible.
replace_once(
    ".github/workflows/release-supply-chain-smoke.yml",
    '''          if [ "$EVENT_NAME" = "release" ] || [ "$EVENT_NAME" = "workflow_run" ]; then\n            expected+=(sippion.cdx.json sippion.cdx.json.sha256)\n          fi\n''',
    '''          if [ "$EVENT_NAME" = "release" ] || [ "$EVENT_NAME" = "workflow_run" ]; then\n            expected+=(\n              sippion.cdx.json\n              sippion.cdx.json.sha256\n              server.json\n              server.json.sha256\n              sippion-linux-x86_64.mcpb\n              sippion-linux-x86_64.mcpb.sha256\n              sippion-windows-x86_64.mcpb\n              sippion-windows-x86_64.mcpb.sha256\n              sippion-macos-aarch64.mcpb\n              sippion-macos-aarch64.mcpb.sha256\n              sippion-macos-x86_64.mcpb\n              sippion-macos-x86_64.mcpb.sha256\n            )\n          fi\n''',
)
replace_once(
    ".github/workflows/release-supply-chain-smoke.yml",
    '''          if [ -f sippion.cdx.json.sha256 ]; then\n            sha256sum -c sippion.cdx.json.sha256\n          fi\n''',
    '''          if [ -f sippion.cdx.json.sha256 ]; then\n            sha256sum -c sippion.cdx.json.sha256\n          fi\n          if [ -f server.json.sha256 ]; then\n            sha256sum -c server.json.sha256\n          fi\n          for sidecar in *.mcpb.sha256; do\n            [ -e "$sidecar" ] || continue\n            sha256sum -c "$sidecar"\n          done\n''',
)
replace_once(
    ".github/workflows/release-supply-chain-smoke.yml",
    '''          if [ -f published/sippion.cdx.json ]; then\n            gh attestation verify "published/sippion.cdx.json" \\\n              --repo "$REPOSITORY" \\\n              --signer-workflow "$REPOSITORY/.github/workflows/release-draft.yml" \\\n              --source-digest "$RELEASE_SHA" >/dev/null\n          fi\n''',
    '''          if [ -f published/sippion.cdx.json ]; then\n            for asset in \\\n              sippion.cdx.json \\\n              server.json \\\n              sippion-linux-x86_64.mcpb \\\n              sippion-windows-x86_64.mcpb \\\n              sippion-macos-aarch64.mcpb \\\n              sippion-macos-x86_64.mcpb; do\n              gh attestation verify "published/$asset" \\\n                --repo "$REPOSITORY" \\\n                --signer-workflow "$REPOSITORY/.github/workflows/release-draft.yml" \\\n                --source-digest "$RELEASE_SHA" >/dev/null\n            done\n          fi\n''',
)

# Trusted auto-merge must wait for release smoke when packaging/publish infrastructure changes.
replace_once(
    ".github/workflows/author-auto-merge.yml",
    'Cargo.toml|Cargo.lock|scripts/bootstrap.sh|scripts/bootstrap.ps1|scripts/install.sh|scripts/install.ps1|.github/workflows/release-build.yml|.github/workflows/release-draft.yml|.github/workflows/release-supply-chain-smoke.yml)',
    'Cargo.toml|Cargo.lock|scripts/bootstrap.sh|scripts/bootstrap.ps1|scripts/install.sh|scripts/install.ps1|scripts/prepare-mcpb.py|scripts/generate-server-json.py|.github/workflows/release-build.yml|.github/workflows/release-draft.yml|.github/workflows/release-supply-chain-smoke.yml|.github/workflows/mcp-registry-publish.yml)',
)

# OIDC Registry publication is a post-release backstop, avoiding GITHUB_TOKEN event recursion.
write(
    ".github/workflows/mcp-registry-publish.yml",
    textwrap.dedent(r'''\
        name: Publish to Official MCP Registry

        on:
          workflow_run:
            workflows:
              - Build draft release
            types:
              - completed
          workflow_dispatch:
            inputs:
              tag:
                description: Published Sippion release tag to publish to the MCP Registry
                required: true
                type: string

        permissions:
          contents: read
          id-token: write

        concurrency:
          group: mcp-registry-publish-${{ github.event.workflow_run.id || inputs.tag }}
          cancel-in-progress: false

        jobs:
          publish:
            if: >-
              github.event_name == 'workflow_dispatch' ||
              (
                github.event.workflow_run.event == 'push' &&
                github.event.workflow_run.conclusion == 'success' &&
                startsWith(github.event.workflow_run.head_branch, 'release/v')
              )
            runs-on: ubuntu-24.04
            env:
              GH_TOKEN: ${{ github.token }}
              REPOSITORY: ${{ github.repository }}
            steps:
              - name: Resolve exact published release
                id: release
                env:
                  EVENT_NAME: ${{ github.event_name }}
                  INPUT_TAG: ${{ inputs.tag }}
                  RUN_HEAD_BRANCH: ${{ github.event.workflow_run.head_branch }}
                  RUN_HEAD_SHA: ${{ github.event.workflow_run.head_sha }}
                shell: bash
                run: |
                  set -euo pipefail
                  if [ "$EVENT_NAME" = "workflow_run" ]; then
                    tag=${RUN_HEAD_BRANCH#release/}
                    expected_sha=$RUN_HEAD_SHA
                  else
                    tag=$INPUT_TAG
                    expected_sha=''
                  fi
                  [[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$ ]]
                  release_json=$(gh api "repos/$REPOSITORY/releases/tags/$tag")
                  jq -e '.draft == false' <<<"$release_json" >/dev/null
                  sha=$(gh api "repos/$REPOSITORY/commits/$tag" --jq '.sha')
                  [[ "$sha" =~ ^[0-9a-f]{40}$ ]]
                  if [ -n "$expected_sha" ] && [ "$sha" != "$expected_sha" ]; then
                    echo "Published tag $tag resolves to $sha, expected $expected_sha" >&2
                    exit 2
                  fi
                  echo "tag=$tag" >> "$GITHUB_OUTPUT"
                  echo "sha=$sha" >> "$GITHUB_OUTPUT"

              - name: Download and verify registry release assets
                env:
                  RELEASE_TAG: ${{ steps.release.outputs.tag }}
                  RELEASE_SHA: ${{ steps.release.outputs.sha }}
                shell: bash
                run: |
                  set -euo pipefail
                  mkdir published
                  gh release download "$RELEASE_TAG" --repo "$REPOSITORY" --dir published --pattern 'server.json*' --pattern '*.mcpb*'
                  cd published
                  test -s server.json
                  test -s server.json.sha256
                  sha256sum -c server.json.sha256
                  for bundle in \
                    sippion-linux-x86_64.mcpb \
                    sippion-windows-x86_64.mcpb \
                    sippion-macos-aarch64.mcpb \
                    sippion-macos-x86_64.mcpb; do
                    test -s "$bundle"
                    test -s "$bundle.sha256"
                    sha256sum -c "$bundle.sha256"
                    gh attestation verify "$bundle" \
                      --repo "$REPOSITORY" \
                      --signer-workflow "$REPOSITORY/.github/workflows/release-draft.yml" \
                      --source-digest "$RELEASE_SHA" >/dev/null
                  done
                  gh attestation verify server.json \
                    --repo "$REPOSITORY" \
                    --signer-workflow "$REPOSITORY/.github/workflows/release-draft.yml" \
                    --source-digest "$RELEASE_SHA" >/dev/null

              - name: Install pinned MCP Registry publisher
                shell: bash
                run: |
                  set -euo pipefail
                  curl --fail --location --silent --show-error \
                    --output /tmp/mcp-publisher.tar.gz \
                    https://github.com/modelcontextprotocol/registry/releases/download/v1.8.1/mcp-publisher_linux_amd64.tar.gz
                  echo 'a06c9096dcb9727c13555b6be26c7effa707b01f06a4c561ba7a3635443cf2cc  /tmp/mcp-publisher.tar.gz' | sha256sum -c -
                  tar -xzf /tmp/mcp-publisher.tar.gz -C /tmp mcp-publisher
                  /tmp/mcp-publisher validate published/server.json

              - name: Authenticate with GitHub OIDC
                run: /tmp/mcp-publisher login github-oidc

              - name: Publish exact release metadata
                run: /tmp/mcp-publisher publish published/server.json
    '''),
)

print("quality feature patch applied")
