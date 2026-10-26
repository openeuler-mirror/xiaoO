use super::*;

#[test]
fn dot_free_segments_map_to_themselves() {
    assert_eq!(
        escape_hook_point_segment("mcp__server__tool"),
        "mcp__server__tool"
    );
    assert_eq!(escape_hook_point_segment(""), "");
}

#[test]
fn dots_and_percent_signs_are_escaped_injectively() {
    assert_eq!(escape_hook_point_segment("plugin.tool"), "plugin%2Etool");
    // the escape char itself is escaped first, so a literal '%2E' cannot
    // collide with an escaped dot and the map stays injective
    assert_eq!(escape_hook_point_segment("a%2Eb"), "a%252Eb");
}
