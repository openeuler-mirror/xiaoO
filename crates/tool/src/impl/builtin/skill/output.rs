use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct SkillToolOutput {
    pub success: bool,
    pub skill_name: String,
    pub content: String,
}
