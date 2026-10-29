//! 本机主机标识(`/remote` 同机约束用)。
//!
//! 远程模式下 open/turn 携带的本地覆盖项、工作区与工具语义都只在
//! "TUI 与 daemon 同机"时成立:连到其他服务器的 daemon 时连接照样成功,
//! 后续运行才冒出各种环境不一致的错误,难以归因。因此 daemon 经
//! `/api/v1/health` 上报所在主机标识,TUI 连接时用同一规则取本机标识
//! 比对,不一致直接拒绝进入远程模式。取值规则收敛在本模块,两侧不许
//! 各算各的(与 `config_identity` 同一约束)。
//!
//! 标识取值优先 `/etc/machine-id`(主流 Linux 发行版标配,改 hostname、
//! 重置网络均不变,是比 hostname 更稳的主机指纹),读不到回退 hostname。
//! 首次调用后进程内缓存:主机标识运行期不会变化,重复读盘只浪费 IO,
//! 也避免异步路径上出现阻塞读。

use std::sync::OnceLock;

/// 取本机主机标识:优先 `/etc/machine-id`,回退 hostname;两者都拿不到
/// 返回 `None`,调用方按校验失败处理,不许当作匹配放行。
pub fn local_host_identity() -> Option<String> {
    static CACHED: OnceLock<Option<String>> = OnceLock::new();
    CACHED.get_or_init(compute_local_host_identity).clone()
}

/// 本机 hostname(主机标识的回退源,也被 lease 展示等纯信息场景复用)。
/// unix 下先读 `/etc/hostname`,再退 `HOSTNAME` 环境变量;非 unix 退
/// `COMPUTERNAME`。
pub fn local_hostname() -> Option<String> {
    #[cfg(unix)]
    {
        read_trimmed("/etc/hostname").or_else(|| env_trimmed("HOSTNAME"))
    }
    #[cfg(not(unix))]
    {
        env_trimmed("COMPUTERNAME")
    }
}

fn compute_local_host_identity() -> Option<String> {
    read_trimmed("/etc/machine-id").or_else(local_hostname)
}

/// 读文件并去首尾空白;空内容视同读不到,让回退链继续。
fn read_trimmed(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(trimmed_non_empty)
}

fn env_trimmed(name: &str) -> Option<String> {
    std::env::var(name).ok().and_then(trimmed_non_empty)
}

fn trimmed_non_empty(value: String) -> Option<String> {
    let trimmed = value.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/shared/host_identity_test.rs"]
mod tests;
