use super::*;

#[test]
fn split_frontmatter_with_yaml() {
    let content = "---\nname: test\ndescription: hello\n---\nBody here";
    let (fm, body) = split_frontmatter(content);
    assert_eq!(fm, Some("name: test\ndescription: hello"));
    assert_eq!(body, "Body here");
}

#[test]
fn split_frontmatter_without_yaml() {
    let content = "Just a body without frontmatter";
    let (fm, body) = split_frontmatter(content);
    assert!(fm.is_none());
    assert_eq!(body, content);
}

#[test]
fn extract_description_from_body_skips_headings() {
    let body = "\n# My Skill\n\nThis does something useful.\n\nMore details.";
    assert_eq!(
        extract_description_from_body(body),
        "This does something useful."
    );
}

#[test]
fn extract_description_empty_body() {
    assert_eq!(extract_description_from_body(""), "");
    assert_eq!(extract_description_from_body("# Only heading"), "");
}

#[test]
fn yaml_like_to_json_basic() {
    let yaml = "name: test\nversion: \"1.0\"\nuser-invocable: true\ntags: [a, b, c]";
    let json = yaml_like_to_json(yaml);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["name"], "test");
    assert_eq!(v["version"], "1.0");
    assert_eq!(v["user-invocable"], true);
    assert_eq!(v["tags"].as_array().unwrap().len(), 3);
}

#[test]
fn yaml_like_to_json_multiline_folded() {
    let yaml = "name: test\ndescription: >\n  Line one.\n  Line two.\n  Line three.";
    let json = yaml_like_to_json(yaml);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["name"], "test");
    assert_eq!(v["description"], "Line one. Line two. Line three.");
}

#[test]
fn yaml_like_to_json_multiline_literal() {
    let yaml = "name: test\ndescription: |\n  Line one.\n  Line two.";
    let json = yaml_like_to_json(yaml);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["name"], "test");
    assert_eq!(v["description"], "Line one.\nLine two.");
}
