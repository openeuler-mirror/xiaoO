//! Skills 配置词汇再导出与 `[skills]` 配置节 schema。
//!
//! 应用层（endside / serverside）的 serde 配置字段需要命名 [`SkillsConfig`]
//! 才能从配置文件反序列化与持有，无法下沉为粗粒度函数（应用必须命名该类型），
//! 故作为配置词汇再导出。该类型属于 skill crate，因 xiaoo-api 冻结不能再导出，
//! 由 shared 这一层承接。其余 skill 内部实现类型不向应用导出。
//!
//! [`SkillsSection`]（`[skills]` 配置节的文件形态）就地定义：daemon / TUI /
//! CLI 三端解析同一份 config.toml，节 schema（字段、键名、别名）与截断上限
//! 的缺省回落在此单一声明，防三份本地拷贝漂移；各端的目录装配策略（默认
//! 目录优先级等属应用侧决策）仍留在各 app 的 resolve 函数中。

pub use skill::{SkillsConfig, DEFAULT_MAX_LISTING_DESCRIPTION_CHARS};

use serde::{Deserialize, Serialize};

/// `[skills]` 配置节的文件形态（daemon / TUI / CLI 三端共用同一 schema）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillsSection {
    /// Additional skill directories to scan (besides the default locations).
    #[serde(default)]
    pub dirs: Option<Vec<String>>,
    /// Allow skills to include script files (.sh, .bash, etc.).
    #[serde(default)]
    pub allow_scripts: Option<bool>,
    /// Skill IDs that should remain installed but not be loaded.
    #[serde(default)]
    pub disabled: Vec<String>,
    /// L1 skill 清单 description 截断上限（Unicode 标量数，默认 250）。
    /// 只影响 prompt 清单视图；管理 API / `skill show` 走 spec 访问器取全文
    #[serde(
        default,
        rename = "max-listing-description-chars",
        alias = "max_listing_description_chars"
    )]
    pub max_listing_description_chars: Option<usize>,
}

impl SkillsSection {
    /// 解析 L1 清单 description 截断上限：节缺省或字段缺省时回落
    /// [`DEFAULT_MAX_LISTING_DESCRIPTION_CHARS`]。三端 resolve 共用同一
    /// 回落来源，防默认值漂移
    pub fn resolve_listing_limit(section: Option<&SkillsSection>) -> usize {
        section
            .and_then(|skills| skills.max_listing_description_chars)
            .unwrap_or(DEFAULT_MAX_LISTING_DESCRIPTION_CHARS)
    }
}
