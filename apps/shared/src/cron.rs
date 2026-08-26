//! 面向应用层的 cron 词汇与校验。
//!
//! 本模块是应用层（endside TUI、serverside 调度器）与底层 `agent_types::cron`
//! 之间的唯一接触面：校验用粗粒度函数交付，配置类型仅在应用必须命名该类型
//! （serde 反序列化、字段持有）时才再导出。

/// 校验 cron 表达式是否合法；`Err` 携带可直接展示给用户的错误信息。
///
/// 应用侧只做"校验字符串"这一件窄事，故下沉为粗粒度函数：调用方不需要
/// `CronExpression` / `CronParseError` 的名字。
pub fn validate_cron_expr(raw: &str) -> Result<(), String> {
    agent_types::cron::CronExpression::parse(raw)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

// 配置词汇：serverside 调度器持有的 cron 任务配置类型。
// `CronJobConfig` 的字段签名可达 `CronExpression`，无法下沉为粗粒度函数；
// 应用必须命名该类型以 serde 反序列化与持有，故按配置词汇再导出。
pub use agent_types::cron::{CronExecutionError, CronExpression, CronJobConfig};

#[cfg(test)]
mod tests {
    use super::validate_cron_expr;

    #[test]
    fn valid_expression_passes() {
        assert!(validate_cron_expr("0 * * * *").is_ok());
        assert!(validate_cron_expr("0 0 * * *").is_ok());
    }

    #[test]
    fn empty_expression_rejected() {
        let err = validate_cron_expr("").expect_err("empty should fail");
        assert!(!err.is_empty());
    }

    #[test]
    fn invalid_expression_rejected() {
        let err = validate_cron_expr("not a cron").expect_err("garbage should fail");
        assert!(!err.is_empty());
    }
}
