use std::collections::HashMap;
use std::path::PathBuf;

use agent_contracts::SkillContext;
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct Skill {
    // --- 基础元数据 ---
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    pub author: Option<String>,
    pub tags: Vec<String>,
    pub location: Option<PathBuf>,

    // --- Prompt 内容 ---
    pub prompt: String,

    // --- 调用控制 ---
    pub user_invocable: bool,
    pub disable_model_invocation: bool,
    pub context: SkillContext,
    pub argument_hint: Option<String>,
    pub arguments: Vec<String>,

    // --- 条件激活 ---
    pub paths: Vec<String>,

    // --- 工具定义（可选扩展）---
    pub tools: Vec<SkillToolDef>,
}

impl Default for Skill {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            version: None,
            author: None,
            tags: Vec::new(),
            location: None,
            prompt: String::new(),
            user_invocable: true,
            disable_model_invocation: false,
            context: SkillContext::Inline,
            argument_hint: None,
            arguments: Vec::new(),
            paths: Vec::new(),
            tools: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SkillToolDef {
    pub name: String,
    pub description: String,
    pub kind: SkillToolKind,
    pub command: String,
    #[serde(default)]
    pub args: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SkillToolKind {
    Shell,
    Http,
    Script,
}
