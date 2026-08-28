//! Skills 配置词汇再导出。
//!
//! 应用层（endside / serverside）的 serde 配置字段需要命名 [`SkillsConfig`]
//! 才能从配置文件反序列化与持有，无法下沉为粗粒度函数（应用必须命名该类型），
//! 故作为配置词汇再导出。该类型属于 skill crate，因 xiaoo-api 冻结不能再导出，
//! 由 shared 这一层承接。其余 skill 内部实现类型不向应用导出。

pub use skill::SkillsConfig;
