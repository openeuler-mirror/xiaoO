use super::{daemon_display, summarize_first_message};

#[test]
fn daemon_display_extracts_host_and_port() {
    assert_eq!(daemon_display("http://127.0.0.1:8070/"), "127.0.0.1:8070");
    assert_eq!(
        daemon_display("https://example.com:443/api"),
        "example.com:443"
    );
}

#[test]
fn first_message_summary_is_one_line() {
    assert_eq!(
        summarize_first_message("hello\nremote   session"),
        "hello remote session"
    );
    assert!(summarize_first_message(&"a".repeat(80)).ends_with("..."));
}
