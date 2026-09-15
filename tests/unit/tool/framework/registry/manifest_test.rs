use super::*;

struct TestToolSpec {
    id: ToolId,
    name: ToolName,
    input_schema: InputSchemaRef,
    output_contract: OutputContract,
    effect_profile: EffectProfile,
}

impl TestToolSpec {
    fn new(name: &str) -> Self {
        Self {
            id: ToolId(name.to_string()),
            name: ToolName(name.to_string()),
            input_schema: InputSchemaRef {
                schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" }
                    }
                }),
            },
            output_contract: OutputContract {
                description: "test output".to_string(),
            },
            effect_profile: EffectProfile::default(),
        }
    }
}

impl ToolSpecView for TestToolSpec {
    fn id(&self) -> &ToolId {
        &self.id
    }

    fn name(&self) -> &ToolName {
        &self.name
    }

    fn description(&self) -> &str {
        "test tool"
    }

    fn input_schema(&self) -> &InputSchemaRef {
        &self.input_schema
    }

    fn output_contract(&self) -> &OutputContract {
        &self.output_contract
    }

    fn effect_profile(&self) -> &EffectProfile {
        &self.effect_profile
    }
}

#[test]
fn tool_spec_snapshot_serializes_and_restores_tool_view() {
    let spec = TestToolSpec::new("grep");
    let manifest = snapshot_tool_specs([&spec as &dyn ToolSpecView]);
    let serialized = serde_json::to_string(&manifest).unwrap();
    let restored_manifest: Vec<ToolSpecSnapshot> = serde_json::from_str(&serialized).unwrap();

    let restored_specs = tool_specs_from_snapshot(&restored_manifest);

    assert_eq!(restored_specs.len(), 1);
    assert_eq!(restored_specs[0].id().0, "grep");
    assert_eq!(restored_specs[0].name().0, "grep");
    assert_eq!(restored_specs[0].description(), "test tool");
    assert_eq!(
        restored_specs[0].input_schema().schema["properties"]["query"]["type"],
        "string"
    );
    assert_eq!(
        restored_specs[0].output_contract().description,
        "test output"
    );
}
