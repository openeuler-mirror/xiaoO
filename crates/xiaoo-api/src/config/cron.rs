//! Cron 表达式校验。
//!
//! 【L1 支撑类型层】[`CronExpression`] 用于表达式校验：xiaoo 的 `/cron`
//! 对话框用它做表达式校验（`apps/endside/src/services/cron_service.rs`），
//! daemon 的 cron 执行体系也用它。
//!
//! > 仅校验表达式；cron 的**执行**体系是 daemon 独有（`refactor.md` §3.5），
//! > 不进入 xiaoo-api。

#[doc(inline)]
pub use agent_types::cron::{CronExpression, CronParseError};
