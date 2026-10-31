//! 上下文窗口解析结论的跨进程磁盘持久化。
//!
//! 内存缓存进程一退即失,冷启动要重跑探测链;本模块把结论写入
//! `dirs::cache_dir()/xiaoo/context_windows.json`,使解析顺序成为
//! 内存 → 磁盘(TTL 内直接采用)→ 网络。
//!
//! 调用方约定:`load` 在内存缓存锁内调用(adoption 决策需在锁内原子落定,
//! 同步 IO 不跨 await);`store` 在锁外调用(`run_probe`)——进程内并发写与
//! 跨进程并发一样,最坏丢一条条目,只是下次多探一次。与
//! OpenRouter 模型缓存(`crates/llm-client/src/models/openrouter.rs`)在
//! "损坏文件按无缓存落穿、回写整文件覆盖顺带修复"两点上一致;本模块额外
//! 用 temp+rename 原子替换,读方不会撞见写了一半的文件。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const CACHE_FILE_NAME: &str = "context_windows.json";

const fn days(count: u64) -> u64 {
    count * 24 * 60 * 60
}

/// 正结果 TTL:模型目录数据随新版本发布变化,量级是"天",一周足够新鲜。
pub(crate) const POSITIVE_TTL: Duration = Duration::from_secs(days(7));

/// 负结果 TTL。比正结果长:一天的 TTL 在晨到晨的使用节奏下天天过期、天天
/// 重探,等于没治;过期代价只是退回静态估算窗口,到期自动重探自愈。
pub(crate) const NEGATIVE_TTL: Duration = Duration::from_secs(days(24));

/// 单条结论,键与内存层同构,为 `provider|model|api_base|credential_fingerprint`
/// 四段(构造见 `runtime_request::resolve_window_with_shared_cache`)。指纹段把
/// 内存层的凭据隔离语义延伸到磁盘:负结果 TTL 长达 [`NEGATIVE_TTL`],若不按
/// 凭据分片,一个凭据源(如无 key 的 TUI 探测)的失败结论会冻结另一个源
/// (如带 key 的 CLI)数周的探测。正结果的窗口本与凭据无关,分片只是让同一
/// 组合多存几条等值条目,代价可忽略。
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PersistedWindowEntry {
    /// `Some` 为探测到的窗口,`None` 为确证负结果(探测链各源被正常咨询后
    /// 仍未命中);瞬态失败不落盘——磁盘 TTL 太长,冻结它会阻断网络恢复后
    /// 的自愈,落盘闸门见 `runtime_request::run_probe`。
    pub(crate) value: Option<u64>,
    /// UNIX 秒级时间戳,TTL 依此判定。
    pub(crate) resolved_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct PersistedWindowCache {
    pub(crate) entries: HashMap<String, PersistedWindowEntry>,
}

/// 磁盘层句柄。`path = None` 表示整层关闭(解析不出缓存目录,或测试隔离)。
#[derive(Clone)]
pub(crate) struct WindowDiskCache {
    path: Option<PathBuf>,
}

impl Default for WindowDiskCache {
    fn default() -> Self {
        Self {
            path: default_cache_path(),
        }
    }
}

impl WindowDiskCache {
    /// 读取 TTL 内的结论条目。外层 `None` 表示无可用条目(文件缺失/损坏/
    /// 条目非法/过期/整层关闭,一律落穿到网络);内层 `None` 表示负结果条目。
    pub(crate) fn load(&self, key: &str) -> Option<Option<usize>> {
        let path = self.path.as_ref()?;
        let raw = std::fs::read_to_string(path).ok()?;
        let cache: PersistedWindowCache = match serde_json::from_str(&raw) {
            Ok(cache) => cache,
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "ignoring corrupt context window cache"
                );
                return None;
            }
        };
        let entry = cache.entries.get(key)?;
        let ttl = match entry.value {
            Some(_) => POSITIVE_TTL,
            None => NEGATIVE_TTL,
        };
        // |now − resolved_at| ≥ TTL 判过期:过去方向是正常 TTL;未来方向把
        // 大幅超前的时间戳(时钟被回写或文件被篡改)按损坏处理——否则这类
        // 条目在真实时间里永不过期。小幅超前(进程间时钟抖动)仍视为新鲜。
        if unix_now().abs_diff(entry.resolved_at) >= ttl.as_secs() {
            return None;
        }
        match entry.value {
            None => Some(None),
            // 探测链只会产出 > 0 的窗口,0 或装不进 usize 的值只可能来自被
            // 篡改的文件;按无条目落穿,不能当负结果(TTL 语义不同)。
            Some(value) => usize::try_from(value)
                .ok()
                .filter(|value| *value > 0)
                .map(Some),
        }
    }

    /// 写入一条结论(读改写 + temp+rename)。损坏旧文件按空表处理,写入顺带
    /// 修复;失败只记 warn,磁盘层尽力而为,不阻塞解析。
    pub(crate) fn store(&self, key: &str, window: Option<usize>) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let mut cache: PersistedWindowCache = std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        cache.entries.insert(
            key.to_string(),
            PersistedWindowEntry {
                value: window.map(|value| value as u64),
                resolved_at: unix_now(),
            },
        );
        let serialized = match serde_json::to_string(&cache) {
            Ok(serialized) => serialized,
            Err(error) => {
                tracing::warn!(%error, "failed to serialize context window cache");
                return;
            }
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // 临时文件名带 PID,避免跨进程并发写到同一临时路径互相截断;rename
        // 失败时清掉临时文件。
        let temp = path.with_extension(format!("{}.tmp", std::process::id()));
        if let Err(error) = std::fs::write(&temp, serialized) {
            tracing::warn!(
                path = %temp.display(),
                %error,
                "failed to write context window cache"
            );
            return;
        }
        if let Err(error) = std::fs::rename(&temp, path) {
            tracing::warn!(
                path = %path.display(),
                %error,
                "failed to replace context window cache"
            );
            let _ = std::fs::remove_file(&temp);
        }
    }
}

/// OnceLock 固化:turn 路径反复构造默认句柄,且环境变化不会分裂读写。
fn default_cache_path() -> Option<PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| dirs::cache_dir().map(|dir| dir.join("xiaoo").join(CACHE_FILE_NAME)))
        .clone()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

// 声明取 pub(crate) 而非私有:runtime_request 的测试要复用本测试文件内的
// test_support 夹具与单测构造器(句柄构造须访问私有字段,只能留在本模块子树内)。
#[cfg(test)]
#[path = "../../../../tests/unit/endside/gateway_api/window_disk_cache_test.rs"]
pub(crate) mod tests;
