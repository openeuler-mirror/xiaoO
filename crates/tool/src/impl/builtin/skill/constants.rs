/// Stable builtin tool id exposed through discovery.
pub const SKILL_TOOL_ID: &str = "builtin_skill";

/// Stable builtin tool name exposed through discovery.
pub const SKILL_TOOL_NAME: &str = "skill";

/// Input key carrying the skill name to invoke. The agent loop's L2 skill
/// discovery ledgering reads this key from completed skill tool results.
pub const SKILL_NAME_INPUT_KEY: &str = "skill";
