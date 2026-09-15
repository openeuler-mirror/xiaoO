use super::{build_progress_card_content, parse_chat_info_payload, parse_member_list_payload};
use crate::channels::{ChannelProgressSection, ChannelProgressState, ChannelProgressUpdate};
use reqwest::StatusCode;
use serde_json::Value;

#[test]
fn builds_progress_card_content_with_sections() {
    let progress = ChannelProgressUpdate {
        title: "小欧正在处理".to_string(),
        summary: "解析群聊需求".to_string(),
        state: ChannelProgressState::Running,
        sections: vec![ChannelProgressSection {
            heading: "状态".to_string(),
            lines: vec!["已识别为群聊任务".to_string(), "开始规划".to_string()],
        }],
    };

    let card = build_progress_card_content(&progress).expect("card content should serialize");
    let payload = serde_json::from_str::<Value>(&card).expect("card should be valid json");

    assert_eq!(payload["header"]["template"], "blue");
    assert_eq!(payload["header"]["title"]["content"], "小欧正在处理");
    assert_eq!(payload["elements"][0]["text"]["content"], "解析群聊需求");
    assert_eq!(
        payload["elements"][2]["text"]["content"],
        "**状态**\n- 已识别为群聊任务\n- 开始规划"
    );
}

#[test]
fn parses_member_list_page_with_pagination() {
    let payload = r#"{
          "code": 0,
          "msg": "success",
          "data": {
            "items": [
              { "member_id": "ou_a", "name": "Alice" },
              { "open_id": "ou_b", "member_name": "Bob" }
            ],
            "has_more": true,
            "page_token": "next-page"
          }
        }"#;

    let (members, next_page) =
        parse_member_list_payload(StatusCode::OK, payload).expect("member list should parse");

    assert_eq!(members.len(), 2);
    assert_eq!(members[0].id, "ou_a");
    assert_eq!(members[0].name.as_deref(), Some("Alice"));
    assert_eq!(members[1].id, "ou_b");
    assert_eq!(members[1].name.as_deref(), Some("Bob"));
    assert_eq!(next_page.as_deref(), Some("next-page"));
}

#[test]
fn parses_chat_info_payload() {
    let payload = r#"{
          "code": 0,
          "msg": "success",
          "data": {
            "owner_id": "ou_owner",
            "chat_type": "group",
            "user_manager_id_list": ["ou_mgr"],
            "bot_manager_id_list": ["cli_bot"]
          }
        }"#;

    let info = parse_chat_info_payload(StatusCode::OK, payload).expect("chat info should parse");

    assert_eq!(info.owner_id.as_deref(), Some("ou_owner"));
    assert_eq!(info.chat_type.as_deref(), Some("group"));
    assert_eq!(info.user_manager_ids, vec!["ou_mgr".to_string()]);
    assert_eq!(info.bot_manager_ids, vec!["cli_bot".to_string()]);
}
