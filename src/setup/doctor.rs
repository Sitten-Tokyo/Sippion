use super::*;

#[derive(Debug)]
enum CheckStatus {
    Ok,
    Missing,
    Mismatch,
    Error(String),
}

impl CheckStatus {
    fn healthy(&self) -> bool {
        matches!(self, Self::Ok)
    }
}

pub(super) fn run_checks(home: &Path, executable: &Path) -> bool {
    let checks = [
        ("Codex MCP config", check_codex(home, executable)),
        ("Claude Code MCP config", check_claude(home, executable)),
        ("Antigravity MCP config", check_antigravity(home, executable)),
        (
            "Codex global rule",
            check_rule(&home.join(".codex").join("AGENTS.md")),
        ),
        (
            "Claude Code global rule",
            check_rule(&home.join(".claude").join("CLAUDE.md")),
        ),
        (
            "Antigravity global rule",
            check_rule(&home.join(".gemini").join("GEMINI.md")),
        ),
    ];

    let mut healthy = true;
    for (label, status) in checks {
        healthy &= status.healthy();
        match status {
            CheckStatus::Ok => println!("{label}: OK"),
            CheckStatus::Missing => println!("{label}: MISSING"),
            CheckStatus::Mismatch => println!("{label}: MISMATCH"),
            CheckStatus::Error(error) => println!("{label}: ERROR ({error})"),
        }
    }
    healthy
}

fn check_codex(home: &Path, executable: &Path) -> CheckStatus {
    let path = home.join(".codex").join("config.toml");
    let contents = match read_optional_text(&path) {
        Ok(Some(contents)) => contents,
        Ok(None) => return CheckStatus::Missing,
        Err(error) => return CheckStatus::Error(error),
    };
    if (contents.contains(MANAGED_BEGIN) || contents.contains(MANAGED_END))
        && managed_block_range(&contents, MANAGED_BEGIN, MANAGED_END).is_err()
    {
        return CheckStatus::Error("malformed Sippion managed markers".to_string());
    }
    if !contents.contains("[mcp_servers.sippion]") {
        return CheckStatus::Missing;
    }
    let expected = match executable_string(executable) {
        Ok(expected) => expected,
        Err(error) => return CheckStatus::Error(error),
    };
    let ok = contents.contains(&toml_string(&expected))
        && contents.contains("args = [\"mcp\", \"--root\", \".\"]")
        && contents.contains("cwd = \".\"")
        && contents.contains("enabled_tools = [\"repo_context\"]");
    if ok {
        CheckStatus::Ok
    } else {
        CheckStatus::Mismatch
    }
}

fn check_claude(home: &Path, executable: &Path) -> CheckStatus {
    check_json_server(&home.join(".claude.json"), executable, true)
}

fn check_antigravity(home: &Path, executable: &Path) -> CheckStatus {
    check_json_server(
        &home.join(".gemini").join("config").join("mcp_config.json"),
        executable,
        false,
    )
}

fn check_json_server(path: &Path, executable: &Path, claude: bool) -> CheckStatus {
    let root = match read_optional_json(path) {
        Ok(Some(root)) => root,
        Ok(None) => return CheckStatus::Missing,
        Err(error) => return CheckStatus::Error(error),
    };
    let Some(entry) = root
        .get("mcpServers")
        .and_then(Value::as_object)
        .and_then(|servers| servers.get(SERVER_NAME))
    else {
        return CheckStatus::Missing;
    };
    let expected = match executable_string(executable) {
        Ok(expected) => expected,
        Err(error) => return CheckStatus::Error(error),
    };
    let expected_args = ["mcp", "--root", "."];
    let args_ok = entry
        .get("args")
        .and_then(Value::as_array)
        .is_some_and(|args| {
            args.len() == expected_args.len()
                && args
                    .iter()
                    .zip(expected_args)
                    .all(|(actual, expected)| actual.as_str() == Some(expected))
        });
    let cwd_ok = entry.get("cwd").and_then(Value::as_str) == Some(".");
    let type_ok = !claude || entry.get("type").and_then(Value::as_str) == Some("stdio");
    let command_ok = entry.get("command").and_then(Value::as_str) == Some(expected.as_str());
    if command_ok && args_ok && cwd_ok && type_ok && is_sippion_json_entry(entry) {
        CheckStatus::Ok
    } else {
        CheckStatus::Mismatch
    }
}

fn check_rule(path: &Path) -> CheckStatus {
    let contents = match read_optional_text(path) {
        Ok(Some(contents)) => contents,
        Ok(None) => return CheckStatus::Missing,
        Err(error) => return CheckStatus::Error(error),
    };
    match managed_block_range(&contents, RULE_BEGIN, RULE_END) {
        Ok(Some((start, end))) if contents[start..end].contains(DISCOVERY_RULE) => CheckStatus::Ok,
        Ok(Some(_)) => CheckStatus::Mismatch,
        Ok(None) => CheckStatus::Missing,
        Err(error) => CheckStatus::Error(error),
    }
}
