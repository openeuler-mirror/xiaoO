//! 配置文件身份（规范路径 + 内容 SHA-256）。
//!
//! `/remote` 要求 TUI 与 daemon 使用同一份配置，否则 open/turn 的合并
//! 校验会报出与配置无关、难以排查的错误。两侧各取一份"启动时实际加载
//! 的配置内容"的身份（daemon 经 `/api/v1/health` 报告，TUI 在加载点
//! 算出快照），连接时比对。路径规范化与哈希收敛在本模块，两侧不许各
//! 算各的。

use sha2::{Digest, Sha256};

/// 生效配置文件的身份标识。
#[derive(Debug, Clone)]
pub struct ConfigFileIdentity {
    /// 规范化后的配置文件路径，仅用于展示，不参与相等判定。
    pub path: String,
    /// 配置文件内容的 SHA-256 十六进制摘要，是比对的唯一依据。
    pub sha256: String,
}

// 相等只看摘要：path 两侧机器几乎必然不同，纳入比较会让同一内容、
// 不同路径判不等。
impl PartialEq for ConfigFileIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.sha256 == other.sha256
    }
}

impl Eq for ConfigFileIdentity {}

impl ConfigFileIdentity {
    /// 按磁盘当前内容计算身份，仅供测试夹具等场景；启动路径必须用
    /// [`Self::from_loaded_content`]，加载后另行重读会拿到进程从未
    /// 加载的内容。
    pub fn from_path(path: &std::path::Path) -> std::io::Result<Self> {
        let bytes = std::fs::read(path)?;
        Ok(Self::from_loaded_content(path, &bytes))
    }

    /// 用已加载的内容计算身份（不读盘）：身份必须与进入内存的配置
    /// 字节同源，连接时重读磁盘会拿到"文件改过、内存没变"的内容。
    pub fn from_loaded_content(path: &std::path::Path, content: &[u8]) -> Self {
        // canonicalize 失败时退回原始路径展示：哈希才是比对依据。
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        Self {
            path: canonical.display().to_string(),
            sha256: hex_sha256(content),
        }
    }
}

/// 统一的 SHA-256 十六进制编码入口。
fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
#[path = "../../../tests/unit/shared/config_identity_test.rs"]
mod tests;
