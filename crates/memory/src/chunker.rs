use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct Chunk {
    pub index: usize,
    pub content: String,
    pub heading: Option<Arc<str>>,
}

/// Split markdown text into token-budget-aware chunks preserving heading context.
///
/// Strategy: (1) split on `# `, `## `, `### ` headings, (2) split on blank lines
/// if a section exceeds max_tokens, (3) split on line boundaries if still too long.
pub fn chunk_markdown(text: &str, max_tokens: usize) -> Vec<Chunk> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let max_chars = max_tokens.saturating_mul(4).max(1);
    let sections = split_on_headings(trimmed);
    let mut chunks = Vec::new();

    for (heading, body) in &sections {
        let heading_arc: Option<Arc<str>> = heading.as_deref().map(Arc::from);
        let body = body.trim();
        if body.is_empty() {
            if let Some(h) = &heading_arc {
                chunks.push(Chunk {
                    index: 0,
                    content: h.to_string(),
                    heading: heading_arc.clone(),
                });
            }
            continue;
        }

        if body.len() <= max_chars {
            let content = match &heading_arc {
                Some(h) => format!("{h}\n{body}"),
                None => body.to_string(),
            };
            chunks.push(Chunk {
                index: 0,
                content,
                heading: heading_arc.clone(),
            });
            continue;
        }

        let paragraphs = split_on_blank_lines(body);
        for para in &paragraphs {
            if para.len() <= max_chars {
                let content = match &heading_arc {
                    Some(h) => format!("{h}\n{para}"),
                    None => para.to_string(),
                };
                chunks.push(Chunk {
                    index: 0,
                    content,
                    heading: heading_arc.clone(),
                });
            } else {
                let sub_chunks = split_on_lines(para, max_chars);
                for sub in sub_chunks {
                    let content = match &heading_arc {
                        Some(h) => format!("{h}\n{sub}"),
                        None => sub,
                    };
                    chunks.push(Chunk {
                        index: 0,
                        content,
                        heading: heading_arc.clone(),
                    });
                }
            }
        }
    }

    for (i, chunk) in chunks.iter_mut().enumerate() {
        chunk.index = i;
    }
    chunks
}

fn split_on_headings(text: &str) -> Vec<(Option<String>, String)> {
    let mut sections = Vec::new();
    let mut current_heading: Option<String> = None;
    let mut current_body = String::new();

    for line in text.lines() {
        let is_heading =
            line.starts_with("# ") || line.starts_with("## ") || line.starts_with("### ");
        if is_heading {
            if !current_body.is_empty() || current_heading.is_some() {
                sections.push((current_heading.take(), std::mem::take(&mut current_body)));
            }
            current_heading = Some(line.to_string());
        } else {
            if !current_body.is_empty() {
                current_body.push('\n');
            }
            current_body.push_str(line);
        }
    }
    if !current_body.is_empty() || current_heading.is_some() {
        sections.push((current_heading, current_body));
    }
    sections
}

fn split_on_blank_lines(text: &str) -> Vec<String> {
    let mut paragraphs = Vec::new();
    let mut current = String::new();

    for line in text.lines() {
        if line.trim().is_empty() {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                paragraphs.push(trimmed);
            }
            current.clear();
        } else {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        paragraphs.push(trimmed);
    }
    paragraphs
}

fn split_on_lines(text: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for line in text.lines() {
        // If a single line exceeds max_chars, split it by words
        if line.len() > max_chars {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            let mut word_buf = String::new();
            for word in line.split_whitespace() {
                let new_len = if word_buf.is_empty() {
                    word.len()
                } else {
                    word_buf.len() + 1 + word.len()
                };
                if new_len > max_chars && !word_buf.is_empty() {
                    chunks.push(std::mem::take(&mut word_buf));
                }
                if !word_buf.is_empty() {
                    word_buf.push(' ');
                }
                word_buf.push_str(word);
            }
            if !word_buf.is_empty() {
                chunks.push(word_buf);
            }
            continue;
        }

        let new_len = if current.is_empty() {
            line.len()
        } else {
            current.len() + 1 + line.len()
        };

        if new_len > max_chars && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
        }

        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text() {
        assert!(chunk_markdown("", 100).is_empty());
        assert!(chunk_markdown("   \n\n  ", 100).is_empty());
    }

    #[test]
    fn single_short_paragraph() {
        let chunks = chunk_markdown("Hello world", 100);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, "Hello world");
        assert!(chunks[0].heading.is_none());
    }

    #[test]
    fn heading_sections() {
        let text = "# Title\nBody of title\n## Section\nBody of section";
        let chunks = chunk_markdown(text, 1000);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].content.contains("Title"));
        assert!(chunks[1].content.contains("Section"));
    }

    #[test]
    fn preserves_heading_in_split() {
        let long_body = "word ".repeat(500);
        let text = format!("# My Heading\n{long_body}");
        let chunks = chunk_markdown(&text, 50);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.content.starts_with("# My Heading"));
            assert!(chunk.heading.is_some());
        }
    }

    #[test]
    fn sequential_indexing() {
        let text = "# A\nBody A\n## B\nBody B\n## C\nBody C";
        let chunks = chunk_markdown(text, 1000);
        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.index, i);
        }
    }

    #[test]
    fn no_content_loss() {
        let text = "# Title\nLine 1\nLine 2\n\n## Section\nLine 3\nLine 4";
        let chunks = chunk_markdown(text, 1000);
        let reassembled: String = chunks
            .iter()
            .map(|c| c.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(reassembled.contains("Line 1"));
        assert!(reassembled.contains("Line 2"));
        assert!(reassembled.contains("Line 3"));
        assert!(reassembled.contains("Line 4"));
    }

    #[test]
    fn unicode_content() {
        let text = "# 标题\n这是中文内容\n## 小节\n更多中文";
        let chunks = chunk_markdown(text, 1000);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].content.contains("标题"));
        assert!(chunks[1].content.contains("小节"));
    }

    #[test]
    fn oversized_single_word_preserved() {
        // A single 100-char "word" with max_tokens=5 (20 chars)
        let long_word = "a".repeat(100);
        let text = format!("short {long_word} end");
        let chunks = chunk_markdown(&text, 5);
        // The oversized word becomes its own chunk, not silently dropped
        let all_text: String = chunks
            .iter()
            .map(|c| c.content.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            all_text.contains(&long_word),
            "oversized word must not be dropped"
        );
    }

    #[test]
    fn max_tokens_zero_does_not_panic() {
        let chunks = chunk_markdown("some text", 0);
        // max_chars = max(0*4, 1) = 1, so everything splits aggressively but no panic
        assert!(!chunks.is_empty());
    }
}
