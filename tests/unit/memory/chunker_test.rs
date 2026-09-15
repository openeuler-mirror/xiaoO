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
