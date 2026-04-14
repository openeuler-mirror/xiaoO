use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct SkillToolInput {
    pub skill: String,
    pub args: Option<String>,
}
