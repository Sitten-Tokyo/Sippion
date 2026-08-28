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
    MODEL_VISIBLE_CONTEXT_HARD_BYTES, McpToolInput, SUPPORTED_LANGUAGE_NAMES,
    heuristic_v3_estimated_tokens,
};
use crate::service::{
    LocalRepositoryService, MAX_CONFIGURED_SCAN_BYTES, MAX_SCAN_BYTES, MIN_CONFIGURED_SCAN_BYTES,
    RepositoryService,
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
        _ => Err("supported commands: mcp, query, inspect, setup, doctor, uninstall".to_string()),
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
            _ => {
                return Err(format!(
                    "unknown doctor argument: {}",
                    arg.to_string_lossy()
                ));
            }
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
        println!(
            "{}",
            serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?
        );
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
            return Err(format!(
                "unknown argument: {}",
                args[index].to_string_lossy()
            ));
        }
        index += 1;
    }
    let (root, scan_budget_bytes) =
        finalize_root(options).map_err(|error| format!("mcp {error}"))?;
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
    let (root, scan_budget_bytes) =
        finalize_root(options).map_err(|error| format!("query {error}"))?;
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
        println!(
            "{}",
            serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?
        );
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
    println!(
        "  query runs the same bounded retrieval path as repo_context; diagnostics are opt-in."
    );
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
    println!(
        "  doctor checks current user-wide settings; --json is machine-readable and --verbose shows paths."
    );
    println!("  uninstall removes only Sippion-managed settings and rules.");
}
