use super::*;

#[test]
fn parses_explicit_mcp_config_path() {
    let args = Args::try_parse_from([
        "xiaoo",
        "--mcp-config",
        "/tmp/mcp.json",
        "run",
        "--prompt",
        "hello",
    ])
    .expect("CLI should accept --mcp-config");

    assert_eq!(
        args.mcp_config.as_deref(),
        Some(std::path::Path::new("/tmp/mcp.json"))
    );
}

#[test]
fn test_generate_title_from_prompt() {
    let prompt = "Fix the bug in authentication module related to JWT token validation";
    let title = generate_title_from_prompt(prompt);
    assert_eq!(
        title,
        Some("Fix the bug in authentication module related to JWT token".to_string())
    );
}

#[test]
fn test_generate_title_from_short_prompt() {
    let prompt = "Hello world";
    let title = generate_title_from_prompt(prompt);
    assert_eq!(title, Some("Hello world".to_string()));
}

#[test]
fn test_generate_title_from_empty_prompt() {
    let prompt = "";
    let title = generate_title_from_prompt(prompt);
    assert_eq!(title, None);
}

#[test]
fn test_generate_title_from_whitespace_prompt() {
    let prompt = "   ";
    let title = generate_title_from_prompt(prompt);
    assert_eq!(title, None);
}
