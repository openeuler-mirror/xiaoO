use serde::Serialize;

#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct SkillToolOutput {
    pub success: bool,
    pub skill_name: String,
    pub content: String,
}
