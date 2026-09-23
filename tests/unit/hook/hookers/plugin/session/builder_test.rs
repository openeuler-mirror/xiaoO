use super::*;

use super::super::super::parsed_hook_point::parse_plugin_hook_point;
use agent_types::common::HookerId;
use agent_types::hook::HookPointId;

fn definition_for(id: &str, hook_point: &str) -> PluginHookerDefinition {
    PluginHookerDefinition {
        id: HookerId(id.to_string()),
        hook_point: HookPointId(hook_point.to_string()),
        command: "cat > /dev/null".to_string(),
        definition: serde_json::json!({
            "id": id,
            "hook_point": hook_point,
            "command": "cat > /dev/null",
        }),
    }
}

fn build_for_stage(hook_point: &str) -> Result<Box<dyn Hooker>, BuildError> {
    let definition = definition_for("plugin_session_test", hook_point);
    let parsed = parse_plugin_hook_point(&definition.hook_point)?;
    build_plugin_session_hooker(definition, parsed)
}

#[test]
fn session_builder_accepts_created_stage() {
    let hooker =
        build_for_stage("*.Session.lifecycle.created").expect("created stage must be registrable");
    assert_eq!(hooker.id().0, "plugin_session_test");
    assert_eq!(hooker.hook_point().0, "*.Session.lifecycle.created");
}

#[test]
fn session_builder_accepts_closed_stage() {
    let hooker =
        build_for_stage("*.Session.lifecycle.closed").expect("closed stage must be registrable");
    assert_eq!(hooker.hook_point().0, "*.Session.lifecycle.closed");
}

#[test]
fn session_builder_accepts_state_stage() {
    let hooker =
        build_for_stage("*.Session.lifecycle.state").expect("state stage must be registrable");
    assert_eq!(hooker.hook_point().0, "*.Session.lifecycle.state");
}

#[test]
fn session_builder_rejects_unknown_stage() {
    let result = build_for_stage("*.Session.lifecycle.started");
    assert!(
        result.is_err(),
        "unknown session lifecycle stage must be rejected at build time"
    );
}
