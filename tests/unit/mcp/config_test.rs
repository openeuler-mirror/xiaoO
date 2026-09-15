use super::*;

#[test]
fn effect_section_default_is_all_true() {
    let e = EffectSection::default();
    assert!(
        e.reads_filesystem,
        "reads_filesystem should default to true"
    );
    assert!(
        e.writes_filesystem,
        "writes_filesystem should default to true"
    );
    assert!(e.network_access, "network_access should default to true");
    assert!(e.side_effects, "side_effects should default to true");
}

#[test]
fn missing_effect_section_defaults_to_all_true() {
    // Reproduces the regression where a server declared without an
    // `[mcp.servers.effect]` section got `EffectSection::default()` from
    // the derived `#[derive(Default)]` (all `false`), contradicting the
    // documented "all four fields default to true" contract. With the
    // manual `Default` impl, both the whole-section-missing path and the
    // per-field-missing path must agree on `true`.
    let toml = r#"
[[servers]]
name = "lookup"
transport = "stdio"
command = "./lookup-server"
"#;
    let mcp: McpSection = toml::from_str(toml).unwrap();
    let effect = &mcp.servers[0].effect;
    assert!(effect.reads_filesystem);
    assert!(effect.writes_filesystem);
    assert!(effect.network_access);
    assert!(effect.side_effects);
}

#[test]
fn partial_effect_section_omits_fields_default_to_true() {
    let toml = r#"
[[servers]]
name = "lookup"
transport = "stdio"
command = "./lookup-server"

[servers.effect]
writes_filesystem = false
side_effects = false
"#;
    let mcp: McpSection = toml::from_str(toml).unwrap();
    let effect = &mcp.servers[0].effect;
    assert!(
        effect.reads_filesystem,
        "omitted field should use default_true"
    );
    assert!(!effect.writes_filesystem);
    assert!(
        effect.network_access,
        "omitted field should use default_true"
    );
    assert!(!effect.side_effects);
}
