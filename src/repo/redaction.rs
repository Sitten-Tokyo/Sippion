use super::*;

/// Defense in depth only. Path denial is the primary policy. Inline redaction is deliberately
/// limited to high-confidence credential forms so ordinary auth code is not destroyed.
#[must_use]
pub(super) fn redact_high_confidence_secrets(text: &str) -> String {
    redact_high_confidence_secrets_with_limit(text, None).text
}

#[must_use]
pub(super) fn redact_high_confidence_secrets_bounded(
    text: &str,
    max_output_bytes: usize,
) -> RedactionOutcome {
    redact_high_confidence_secrets_with_limit(text, Some(max_output_bytes))
}

pub(super) fn redact_high_confidence_secrets_with_limit(
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

pub(super) fn push_redacted_line(
    output: &mut String,
    line: &str,
    max_output_bytes: Option<usize>,
) -> bool {
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

pub(super) fn dangling_sensitive_key(line: &str) -> Option<PendingSensitiveValue> {
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

pub(super) fn is_yaml_block_scalar_indicator(trimmed: &str) -> bool {
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

pub(super) fn redact_sensitive_block_scalar_declaration(line: &str) -> Option<(usize, String)> {
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

pub(super) fn redact_indented_sensitive_scalar(
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

pub(super) fn redact_explicit_authorization_header_credentials(line: &str) -> String {
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

pub(super) fn redact_auth_scheme_credentials(line: &str) -> String {
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

pub(super) fn redact_jwt_substrings(line: &str) -> String {
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

pub(super) fn redact_url_userinfo_credentials(line: &str) -> String {
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

pub(super) fn redact_cookie_header_values(line: &str) -> String {
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

pub(super) fn redact_token_substrings(line: &str) -> String {
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

pub(super) fn redact_sensitive_literal_assignments(line: &str) -> String {
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
