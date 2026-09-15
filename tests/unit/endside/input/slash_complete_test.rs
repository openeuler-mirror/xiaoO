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
