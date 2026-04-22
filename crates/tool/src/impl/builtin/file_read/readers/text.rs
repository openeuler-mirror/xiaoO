use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};

use super::super::output::TextOutput;

pub async fn read_text_file<P: AsRef<Path>>(
    file_path: P,
    offset: Option<u64>,
    limit: Option<u64>,
) -> std::io::Result<TextOutput> {
    let file_path = file_path.as_ref();
    let file_path_str = file_path.to_string_lossy().to_string();

    let file = File::open(file_path).await?;
    let reader = BufReader::new(file);

    let mut all_lines: Vec<String> = Vec::new();
    let mut lines_stream = reader.lines();

    while let Some(line) = lines_stream.next_line().await? {
        all_lines.push(line);
    }

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
        file_path: file_path_str,
        content,
        num_lines,
        start_line,
        total_lines,
    })
}

#[allow(dead_code)]
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
