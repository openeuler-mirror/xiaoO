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
fn long_prompt_title_is_truncated_to_ten_words() {
    let prompt = "Fix the bug in authentication module related to JWT token validation";
    let title = generate_title_from_prompt(prompt);
    assert_eq!(
        title,
        Some("Fix the bug in authentication module related to JWT token".to_string())
    );
}

#[test]
fn short_prompt_is_used_as_title_verbatim() {
    let prompt = "Hello world";
    let title = generate_title_from_prompt(prompt);
    assert_eq!(title, Some("Hello world".to_string()));
}

#[test]
fn empty_prompt_yields_no_title() {
    let prompt = "";
    let title = generate_title_from_prompt(prompt);
    assert_eq!(title, None);
}

#[test]
fn whitespace_only_prompt_yields_no_title() {
    let prompt = "   ";
    let title = generate_title_from_prompt(prompt);
    assert_eq!(title, None);
}

#[test]
fn task_timeout_defaults_to_ten_minutes_and_accepts_override() {
    for (extra, expected) in [(vec![], 600), (vec!["--timeout", "120"], 120)] {
        let mut arguments = vec!["xiaoo", "run", "--prompt", "hello"];
        arguments.extend(extra);
        let args = Args::try_parse_from(arguments).unwrap();
        assert!(
            matches!(args.command, Some(Command::Run { timeout_secs, .. }) if timeout_secs == expected)
        );
    }
    assert!(Args::try_parse_from(["xiaoo", "run", "--prompt", "hello", "--timeout", "0"]).is_err());
}

#[test]
fn run_parses_positive_token_limits() {
    let defaults = Args::try_parse_from(["xiaoo", "run", "--prompt", "hello"]).unwrap();
    assert!(matches!(
        defaults.command,
        Some(Command::Run {
            context_window: None,
            max_tokens: None,
            ..
        })
    ));
    for (context_flag, output_flag) in [
        ("--context-window", "--max-tokens"),
        ("--context-limit-tokens", "--max-output-tokens"),
    ] {
        let explicit = Args::try_parse_from([
            "xiaoo",
            "run",
            "--prompt",
            "hello",
            context_flag,
            "32768",
            output_flag,
            "4096",
        ])
        .unwrap();
        assert!(matches!(
            explicit.command,
            Some(Command::Run {
                context_window: Some(32768),
                max_tokens: Some(4096),
                ..
            })
        ));
    }

    let help = Args::try_parse_from(["xiaoo", "run", "--help"])
        .err()
        .unwrap()
        .to_string();
    assert!(help.contains("--context-window"));
    assert!(help.contains("--max-tokens"));
    assert!(!help.contains("--context-limit-tokens"));
    assert!(!help.contains("--max-output-tokens"));

    for flag in ["--context-window", "--max-tokens"] {
        for value in ["0", "-1", "1.5", "4294967296"] {
            assert!(
                Args::try_parse_from(["xiaoo", "run", "--prompt", "hello", flag, value]).is_err()
            );
        }
        assert!(Args::try_parse_from([
            "xiaoo",
            "run",
            "--prompt",
            "hello",
            flag,
            "16",
            "--attach",
            "http://localhost",
        ])
        .is_err());
    }
}
