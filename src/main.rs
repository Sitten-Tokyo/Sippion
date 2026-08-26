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

use std::path::PathBuf;
use std::sync::Arc;

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
    match command.to_str() {
        Some("setup") => {
            if args.next().is_some() {
                return Err("setup does not accept arguments".to_string());
            }
            return managed::run_setup();
        }
        Some("doctor") => {
            if args.next().is_some() {
                return Err("doctor does not accept arguments".to_string());
            }
            return managed::run_doctor();
        }
        Some("uninstall") => {
            if args.next().is_some() {
                return Err("uninstall does not accept arguments".to_string());
            }
            return managed::run_uninstall();
        }
        Some("mcp") => {}
        _ => {
            return Err(
                "supported commands: mcp --root <project>, mcp --root-auto, setup, doctor, uninstall"
                    .to_string(),
            );
        }
    }

    let mut root: Option<PathBuf> = None;
    let mut root_auto = false;
    let mut allow_broad_root = false;
    let mut allow_broad_root_seen = false;
    let mut scan_budget_bytes = MAX_SCAN_BYTES;
    let mut scan_budget_seen = false;
    while let Some(arg) = args.next() {
        if arg.to_str() == Some("--root") {
            if root.is_some() {
                return Err("--root may be specified once".to_string());
            }
            let Some(value) = args.next() else {
                return Err("--root requires a path".to_string());
            };
            root = Some(PathBuf::from(value));
        } else if arg.to_str() == Some("--root-auto") {
            if root_auto {
                return Err("--root-auto may be specified once".to_string());
            }
            root_auto = true;
        } else if arg.to_str() == Some("--allow-broad-root") {
            if allow_broad_root_seen {
                return Err("--allow-broad-root may be specified once".to_string());
            }
            allow_broad_root_seen = true;
            allow_broad_root = true;
        } else if arg.to_str() == Some("--scan-budget-mib") {
            if scan_budget_seen {
                return Err("--scan-budget-mib may be specified once".to_string());
            }
            scan_budget_seen = true;
            let Some(value) = args.next() else {
                return Err("--scan-budget-mib requires an integer".to_string());
            };
            let value = value
                .to_str()
                .ok_or_else(|| "--scan-budget-mib must be valid UTF-8".to_string())?
                .parse::<usize>()
                .map_err(|_| "--scan-budget-mib must be an integer".to_string())?;
            let bytes = value
                .checked_mul(1024 * 1024)
                .ok_or_else(|| "--scan-budget-mib is too large".to_string())?;
            if !(MIN_CONFIGURED_SCAN_BYTES..=MAX_CONFIGURED_SCAN_BYTES).contains(&bytes) {
                return Err("--scan-budget-mib must be between 16 and 512".to_string());
            }
            scan_budget_bytes = bytes;
        } else {
            return Err(format!("unknown argument: {}", arg.to_string_lossy()));
        }
    }

    if root.is_some() && root_auto {
        return Err("use exactly one of --root <project> or --root-auto".to_string());
    }
    if root.is_none() && !root_auto {
        return Err("mcp requires --root <project> or --root-auto".to_string());
    }
    if root_auto && allow_broad_root {
        return Err("--allow-broad-root is valid only with explicit --root".to_string());
    }

    let root = if root_auto {
        root::infer_project_root_from_cwd()?
    } else {
        root::secure_explicit_root(root.expect("validated explicit root"), allow_broad_root)?
    };
    let service: Arc<dyn RepositoryService> = Arc::new(
        LocalRepositoryService::open_with_scan_budget(root, scan_budget_bytes)
            .map_err(|_| "cannot secure project root".to_string())?,
    );
    mcp::serve_stdio(service).map_err(|error| format!("stdio MCP failed: {error}"))
}

fn print_help() {
    println!("Sippion {}", crate::core::VERSION);
    println!("Usage: sippion mcp --root <project> [--scan-budget-mib 16..512]");
    println!("       sippion mcp --root-auto [--scan-budget-mib 16..512]");
    println!("       sippion setup");
    println!("       sippion doctor");
    println!("       sippion uninstall");
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
    println!("  doctor checks the current user-wide MCP settings and rules.");
    println!("  uninstall removes only Sippion-managed settings and rules.");
}
