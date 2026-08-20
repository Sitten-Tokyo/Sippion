pub(super) fn signature_from_lines(lines: &[&str], line: u32) -> String {
    let Some(index) = line.checked_sub(1).map(|value| value as usize) else {
        return String::new();
    };
    lines
        .get(index)
        .map(|value| value.trim_start().chars().take(220).collect::<String>())
        .unwrap_or_default()
}
