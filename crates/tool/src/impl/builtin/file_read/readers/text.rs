use super::super::output::TextOutput;

pub fn read_text_from_bytes(
    file_path: &str,
    bytes: &[u8],
    offset: Option<u64>,
    limit: Option<u64>,
) -> std::io::Result<TextOutput> {
    let text = String::from_utf8_lossy(bytes);
    let all_lines: Vec<&str> = text.lines().collect();
    let total_lines = all_lines.len() as u64;

    let start_pos = offset
        .map(|o| if o == 0 { 0 } else { (o - 1) as usize })
        .unwrap_or(0);

    let start_pos = start_pos.min(all_lines.len());

    let end_pos = match limit {
        Some(l) if l > 0 => (start_pos + l as usize).min(all_lines.len()),
        _ => all_lines.len(),
    };

    let selected_lines = &all_lines[start_pos..end_pos];
    let content = selected_lines.join("\n");

    let num_lines = selected_lines.len() as u64;
    let start_line = if total_lines == 0 {
        1
    } else {
        (start_pos + 1) as u64
    };

    Ok(TextOutput {
        file_path: file_path.to_string(),
        content,
        num_lines,
        start_line,
        total_lines,
    })
}
