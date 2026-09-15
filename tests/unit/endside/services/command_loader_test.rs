use super::*;

#[test]
fn split_frontmatter_basic() {
    let content = "---\ndescription: hello world\n---\nBody here";
    let (fm, body) = split_frontmatter(content);
    assert_eq!(fm, Some("description: hello world"));
    assert_eq!(body, "Body here");
}

#[test]
fn split_frontmatter_no_yaml() {
    let content = "Just plain text";
    let (fm, body) = split_frontmatter(content);
    assert!(fm.is_none());
    assert_eq!(body, content);
}

#[test]
fn extract_field_basic() {
    let fm = "description: Agent project development\ndisable-model-invocation: true";
    assert_eq!(
        extract_field(fm, "description"),
        Some("Agent project development".to_string())
    );
    assert_eq!(
        extract_field(fm, "disable-model-invocation"),
        Some("true".to_string())
    );
    assert_eq!(extract_field(fm, "missing"), None);
}

#[test]
fn extract_field_quoted() {
    let fm = "description: \"quoted value\"";
    assert_eq!(
        extract_field(fm, "description"),
        Some("quoted value".to_string())
    );
}
