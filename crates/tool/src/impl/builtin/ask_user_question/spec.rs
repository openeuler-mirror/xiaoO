use agent_contracts::tool::ToolSpecView;
use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};

#[derive(Debug, Clone)]
pub struct AskUserQuestionToolSpec {
    id: ToolId,
    name: ToolName,
    description: String,
    input_schema: InputSchemaRef,
    output_contract: OutputContract,
    effect_profile: EffectProfile,
}

impl AskUserQuestionToolSpec {
    pub fn new() -> Self {
        // JSON Schema 支持三种 kind 的 oneOf 变体
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "description": "要向用户提出的问题列表（1–4 个）。每个问题通过 kind 字段区分类型。",
                    "minItems": 1,
                    "maxItems": 4,
                    "items": {
                        "oneOf": [
                            {
                                "type": "object",
                                "description": "确认类问题，用户回答是或否。",
                                "properties": {
                                    "kind":   { "type": "string", "enum": ["confirm"] },
                                    "prompt": { "type": "string", "description": "向用户展示的问题文本。" }
                                },
                                "required": ["kind", "prompt"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "description": "文本输入类问题，用户可自由输入文本。",
                                "properties": {
                                    "kind":   { "type": "string", "enum": ["text_input"] },
                                    "prompt": { "type": "string", "description": "向用户展示的问题文本。" }
                                },
                                "required": ["kind", "prompt"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "description": "选项类问题，用户从给定选项中选择。",
                                "properties": {
                                    "kind":   { "type": "string", "enum": ["choice"] },
                                    "prompt": { "type": "string", "description": "向用户展示的问题文本。" },
                                    "options": {
                                        "type": "array",
                                        "description": "供用户选择的选项列表（至少 2 项）。",
                                        "items": { "type": "string" },
                                        "minItems": 2
                                    },
                                    "allow_custom_input": {
                                        "type": "boolean",
                                        "description": "是否允许用户输入选项以外的自定义内容。",
                                        "default": false
                                    }
                                },
                                "required": ["kind", "prompt", "options"],
                                "additionalProperties": false
                            }
                        ]
                    }
                }
            },
            "required": ["questions"]
        });

        Self {
            id: ToolId("builtin_ask_user_question".to_string()),
            name: ToolName("ask_user_question".to_string()),
            description: "向用户提出一个或多个问题并收集回答。\n\
                支持三种问题类型：\n\
                - confirm：是/否确认\n\
                - text_input：自由文本输入\n\
                - choice：从给定选项中选择（可选允许自定义输入）\n\
                每次调用最多可提出 4 个问题，按顺序依次与用户交互。"
                .to_string(),
            input_schema: InputSchemaRef { schema },
            output_contract: OutputContract {
                description: "包含每个问题回答的列表，顺序与输入问题一致。".to_string(),
            },
            effect_profile: EffectProfile {
                reads_filesystem: false,
                writes_filesystem: false,
                network_access: false,
                side_effects: false,
            },
        }
    }
}

impl ToolSpecView for AskUserQuestionToolSpec {
    fn id(&self) -> &ToolId {
        &self.id
    }

    fn name(&self) -> &ToolName {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
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
