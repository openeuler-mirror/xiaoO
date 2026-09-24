use super::*;
use serde_json::json;

fn extra_with_wrapper(wrapper: serde_json::Value) -> serde_json::Value {
    serde_json::json!([
        { "hooker_id": "plugin-a", "spawn_wrapper": wrapper }
    ])
}

#[test]
fn none_when_extra_absent_or_empty() {
    assert!(spawn_wrapper_from_extra(None).is_none());
    assert!(spawn_wrapper_from_extra(Some(&serde_json::json!([]))).is_none());
}

#[test]
fn parses_prefix_and_defaults_externalize_flag() {
    let extra = extra_with_wrapper(json!({
        "prefix": ["/opt/wrap", "--session=s1", "--"]
    }));
    let wrapper = spawn_wrapper_from_extra(Some(&extra)).expect("wrapper");
    assert_eq!(wrapper.prefix, vec!["/opt/wrap", "--session=s1", "--"]);
    assert!(
        !wrapper.externalize_fs_writes,
        "externalize_fs_writes defaults to false"
    );
}

#[test]
fn parses_externalize_fs_writes_true() {
    let extra = extra_with_wrapper(json!({
        "prefix": ["/opt/wrap"],
        "externalize_fs_writes": true
    }));
    let wrapper = spawn_wrapper_from_extra(Some(&extra)).expect("wrapper");
    assert!(wrapper.externalize_fs_writes);
}

#[test]
fn first_valid_entry_wins() {
    let extra = json!([
        { "hooker_id": "plugin-a", "spawn_wrapper": { "prefix": ["/first"] } },
        { "hooker_id": "plugin-b", "spawn_wrapper": { "prefix": ["/second"] } }
    ]);
    let wrapper = spawn_wrapper_from_extra(Some(&extra)).expect("wrapper");
    assert_eq!(wrapper.prefix, vec!["/first"]);
}

#[test]
fn entries_without_spawn_wrapper_are_skipped() {
    let extra = json!([
        { "hooker_id": "plugin-a", "unrelated": { "note": true } },
        { "hooker_id": "plugin-b", "spawn_wrapper": { "prefix": ["/wrap"] } }
    ]);
    let wrapper = spawn_wrapper_from_extra(Some(&extra)).expect("wrapper");
    assert_eq!(wrapper.prefix, vec!["/wrap"]);
}

#[test]
fn malformed_shapes_are_ignored() {
    // prefix missing / not an array / empty / non-string element
    for malformed in [
        json!({}),
        json!({ "prefix": "not-an-array" }),
        json!({ "prefix": [] }),
        json!({ "prefix": ["/ok", 42] }),
    ] {
        let extra = extra_with_wrapper(malformed);
        assert!(
            spawn_wrapper_from_extra(Some(&extra)).is_none(),
            "malformed wrapper must be ignored"
        );
    }
}

#[test]
fn non_array_extra_channel_is_ignored() {
    assert!(
        spawn_wrapper_from_extra(Some(&json!({ "spawn_wrapper": { "prefix": ["/x"] } }))).is_none()
    );
    assert!(spawn_wrapper_from_extra(Some(&json!("string"))).is_none());
}

#[test]
fn unknown_keys_are_ignored_for_forward_compatibility() {
    let extra = extra_with_wrapper(json!({
        "prefix": ["/opt/wrap"],
        "externalize_fs_writes": true,
        "future_field": { "anything": true }
    }));
    let wrapper = spawn_wrapper_from_extra(Some(&extra)).expect("wrapper");
    assert_eq!(wrapper.prefix, vec!["/opt/wrap"]);
    assert!(wrapper.externalize_fs_writes);
}
