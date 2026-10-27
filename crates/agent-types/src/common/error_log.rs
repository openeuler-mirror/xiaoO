use std::path::PathBuf;

/// 解析共享 error.log 的落盘位置,是所有写入方的 single source of truth。
///
/// llm-client 的流式错误块(`source=llm_stream_error`)与 endside 的 TUI
/// 错误条目(`source=remote_input` 等)写同一份文件;该约定一旦分叉,
/// 一次事故的线索会被拆进两个文件,所以不要在调用方复制这段逻辑。
///
/// XIAOO_ERROR_LOG 是运维重定向钩子(例如把日志收集到容器卷),未设置
/// 或为空时回落到按用户布局的 ~/.xiaoo/log/error.log;home 目录解析
/// 失败(无 HOME 的服务进程)时落到同构的相对路径布局,保证"提示消息
/// 指向的文件"与"实际写入的文件"始终一致。
pub fn error_log_path() -> PathBuf {
    if let Ok(path) = std::env::var("XIAOO_ERROR_LOG") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    dirs::home_dir()
        .map(|h| h.join(".xiaoo").join("log").join("error.log"))
        .unwrap_or_else(|| PathBuf::from(".xiaoo").join("log").join("error.log"))
}
