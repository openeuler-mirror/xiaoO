use super::*;

const NO_EXT: &[ExternalCommand] = &[];

fn sample_external() -> Vec<ExternalCommand> {
    vec![ExternalCommand {
        name: "agent-start".to_string(),
        description: "Start an agent project".to_string(),
        body: "invoke the agent-start skill".to_string(),
    }]
}

#[test]
fn con_to_connect() {
    let mut i: Input = "/con".into();
    assert!(apply_slash_tab(&mut i, NO_EXT));
    assert_eq!(i.value(), "/connect");
}

#[test]
fn c_has_multiple_matches() {
    let mut i: Input = "/c".into();
    // /c now matches both /connect and /cron, so tab cannot expand uniquely
    assert!(!apply_slash_tab(&mut i, NO_EXT));
    assert_eq!(i.value(), "/c");
}

#[test]
fn leading_spaces() {
    let mut i = Input::default().with_value("  /con".to_string());
    assert!(apply_slash_tab(&mut i, NO_EXT));
    assert_eq!(i.value(), "  /connect");
}

#[test]
fn candidates_prefix_builtin() {
    assert_eq!(
        candidates_for_prefix("/", NO_EXT),
        vec![
            "/connect",
            "/cron",
            "/dir",
            "/delete",
            "/load",
            "/mcp",
            "/new",
            "/save",
            "/remote",
            "/sandbox",
            "/sessions",
            "/skills"
        ]
    );
    assert_eq!(
        candidates_for_prefix("/c", NO_EXT),
        vec!["/connect", "/cron"]
    );
    assert_eq!(candidates_for_prefix("/con", NO_EXT), vec!["/connect"]);
    assert_eq!(candidates_for_prefix("/d", NO_EXT), vec!["/dir", "/delete"]);
    assert_eq!(candidates_for_prefix("/l", NO_EXT), vec!["/load"]);
    assert_eq!(candidates_for_prefix("/m", NO_EXT), vec!["/mcp"]);
    assert_eq!(candidates_for_prefix("/n", NO_EXT), vec!["/new"]);
    assert_eq!(candidates_for_prefix("/r", NO_EXT), vec!["/remote"]);
    assert_eq!(
        candidates_for_prefix("/sa", NO_EXT),
        vec!["/save", "/sandbox"]
    );
    assert_eq!(
        candidates_for_prefix("/s", NO_EXT),
        vec!["/save", "/sandbox", "/sessions", "/skills"]
    );
}

#[test]
fn candidates_include_external() {
    let ext = sample_external();
    let all = candidates_for_prefix("/", &ext);
    assert!(all.contains(&"/agent-start".to_string()));
    assert!(all.contains(&"/connect".to_string()));

    let a = candidates_for_prefix("/a", &ext);
    assert_eq!(a, vec!["/agent-start"]);
}

#[test]
fn tab_completes_external() {
    let ext = sample_external();
    let mut i: Input = "/ag".into();
    assert!(apply_slash_tab(&mut i, &ext));
    assert_eq!(i.value(), "/agent-start");
}

#[test]
fn apply_pick() {
    let mut i: Input = "/co".into();
    apply_slash_pick(&mut i, "/connect");
    assert_eq!(i.value(), "/connect");
}

#[test]
fn summaries_builtin() {
    assert_eq!(
        summary_for_command("/connect", NO_EXT),
        Some("打开 provider / model 选择窗口并连接当前后端。".to_string())
    );
    assert_eq!(summary_for_command("/missing", NO_EXT), None);
}

#[test]
fn summaries_external() {
    let ext = sample_external();
    assert_eq!(
        summary_for_command("/agent-start", &ext),
        Some("Start an agent project".to_string())
    );
}

fn chinese_external(name: &str) -> ExternalCommand {
    ExternalCommand {
        name: name.to_string(),
        description: String::new(),
        body: String::new(),
    }
}

/// Regression: 一 (E4 B8 80) 与 二 (E4 BA 8C) 共享首字节，逐字节求公共前缀会停在
/// 字符中间（`end byte index 5 is not a char boundary` -> panic）。
#[test]
fn longest_common_prefix_stops_on_a_char_boundary() {
    assert_eq!(longest_common_prefix(&["/测一分享", "/测二分享"]), "/测");
    assert_eq!(
        longest_common_prefix(&["/测试甲分享", "/测试乙分享"]),
        "/测试"
    );
    assert_eq!(longest_common_prefix(&["😀a", "😀b"]), "😀");
    assert_eq!(longest_common_prefix(&["中文", "中英"]), "中");
}

#[test]
fn longest_common_prefix_keeps_ascii_semantics() {
    assert_eq!(longest_common_prefix(&["abc", "abd"]), "ab");
    assert_eq!(longest_common_prefix(&["abc", "abcd"]), "abc");
    assert_eq!(longest_common_prefix(&["abc", "xyz"]), "");
    assert_eq!(longest_common_prefix(&["solo"]), "solo");
    assert_eq!(longest_common_prefix(&[]), "");
}

/// Regression: 输入 `/测` 后按 Tab，两个非 ASCII 命令名的补全曾直接 panic。
#[test]
fn tab_completion_with_chinese_command_names_does_not_panic() {
    let external = vec![chinese_external("测一分享"), chinese_external("测二分享")];
    let mut input = Input::default()
        .with_value("/测".to_string())
        .with_cursor(2);

    // 公共前缀与已输入内容相同，因此不产生改动——关键是绝不能 panic。
    let changed = apply_slash_tab(&mut input, &external);
    assert!(!changed);
    assert_eq!(input.value(), "/测");
}

#[test]
fn tab_completion_still_expands_a_chinese_prefix() {
    let external = vec![
        chinese_external("测试甲分享"),
        chinese_external("测试乙分享"),
    ];
    let mut input = Input::default()
        .with_value("/测".to_string())
        .with_cursor(2);

    assert!(apply_slash_tab(&mut input, &external));
    assert_eq!(input.value(), "/测试");
}
