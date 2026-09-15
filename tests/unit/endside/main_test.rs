use super::{classify_args, parse_config_path_from, EntryInvocation};
use std::ffi::OsString;

#[test]
fn tui_accepts_mcp_config_alongside_toml_config() {
    let parsed = parse_config_path_from([
        "xiaoo",
        "--config",
        "/tmp/config.toml",
        "--mcp-config",
        "/tmp/mcp.json",
    ])
    .expect("TUI should accept both config paths");

    assert_eq!(parsed.path, std::path::PathBuf::from("/tmp/config.toml"));
    assert_eq!(
        parsed.mcp_config,
        Some(std::path::PathBuf::from("/tmp/mcp.json"))
    );
}

#[test]
fn no_args_dispatches_to_tui() {
    assert_eq!(
        classify_args(vec![OsString::from("xiaoo")]),
        EntryInvocation::Tui(vec![OsString::from("xiaoo")])
    );
}

#[test]
fn cli_switch_dispatches_to_cli_without_switch() {
    assert_eq!(
        classify_args(vec![
            OsString::from("xiaoo"),
            OsString::from("--cli"),
            OsString::from("run"),
            OsString::from("-p"),
            OsString::from("hello"),
        ]),
        EntryInvocation::Cli(vec![
            OsString::from("xiaoo"),
            OsString::from("run"),
            OsString::from("-p"),
            OsString::from("hello"),
        ])
    );
}

#[test]
fn help_dispatches_to_end_side_help() {
    assert_eq!(
        classify_args(vec![OsString::from("xiaoo"), OsString::from("--help")]),
        EntryInvocation::Help {
            program: OsString::from("xiaoo"),
        }
    );
}
