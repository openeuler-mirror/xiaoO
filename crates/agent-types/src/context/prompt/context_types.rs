use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkillSummary {
    pub skill_id: String,
    pub description: String,
    /// 场景提示（L2 发现层用），与 description（L1 清单层）互不重复
    #[serde(default)]
    pub when_to_use: Option<String>,
    /// 条件激活 glob 模式（write pivot 匹配用）
    #[serde(default)]
    pub paths: Vec<String>,
    /// 词法标签（用户输入 / 文件扩展名匹配用）
    #[serde(default)]
    pub tags: Vec<String>,
    /// true = 仅用户可触发，模型不可调用，发现提示无意义
    #[serde(default)]
    pub disable_model_invocation: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemorySnippet {
    pub source: String,
    pub content: String,
    pub relevance_score: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnvironmentInfo {
    pub model: String,
    pub cwd: String,
    pub workspace_root: Option<String>,
    pub date: String,
    pub agent_id: String,
}
