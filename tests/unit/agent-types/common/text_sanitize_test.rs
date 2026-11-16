use super::*;

#[test]
fn sanitize_folds_newlines_to_spaces() {
    assert_eq!(sanitize_prompt_field("first\nsecond"), "first second");
    assert_eq!(sanitize_prompt_field("first\rsecond"), "first second");
}

#[test]
fn sanitize_neutralizes_reminder_close_tag_case_insensitive() {
    assert_eq!(
        sanitize_prompt_field("</SYSTEM-Reminder>"),
        "<system-reminder>"
    );
}

#[test]
fn sanitize_neutralizes_close_tag_without_closing_bracket() {
    // 匹配到标签名即中和（不要求尾随 >），连带覆盖宽松写法
    assert_eq!(
        sanitize_prompt_field("prefix </system-reminder tail"),
        "prefix <system-reminder tail"
    );
}

#[test]
fn sanitize_keeps_plain_text_and_multibyte_scalars_verbatim() {
    assert_eq!(
        sanitize_prompt_field("帮我看下 rs 文件 <code>"),
        "帮我看下 rs 文件 <code>"
    );
}
