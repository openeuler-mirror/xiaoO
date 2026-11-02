//! OpenRouter 公共模型库查询：在运行时动态解析模型上下文窗口数据
//! （链路位置见 `models::resolve_model_context_length`：provider
//! catalog > OpenRouter > Ollama `/api/show`）。
//!
//! 请求为匿名 GET 公开端点，不携带用户数据与 API key；全链 fail-soft：
//! 内存缓存（TTL 24h）→ 新鲜磁盘缓存 → HTTP → 过期磁盘缓存 → 过期内存
//! 缓存逐级回落（新鲜磁盘缓存优先于 HTTP：进程冷启动后首查零网络）。
//!
//! 环境变量：`XIAOO_OPENROUTER_MODELS_URL` 自定义镜像；
//! `XIAOO_DISABLE_OPENROUTER_LOOKUP` 置为非 `0`/`false`/空 时禁用。

use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::normalize_model_id;
use crate::error::{map_reqwest_error_raw as map_reqwest_error, LlmError};
use serde::{Deserialize, Serialize};

const DEFAULT_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
const URL_ENV: &str = "XIAOO_OPENROUTER_MODELS_URL";
const DISABLE_ENV: &str = "XIAOO_DISABLE_OPENROUTER_LOOKUP";
const CACHE_FILE_NAME: &str = "openrouter_models.json";
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CatalogEntry {
    id: String,
    context_length: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedCatalog {
    fetched_at_unix: u64,
    entries: Vec<CatalogEntry>,
}

impl CachedCatalog {
    fn fresh(&self) -> bool {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|now| now.checked_sub(Duration::from_secs(self.fetched_at_unix)))
            .is_some_and(|age| age < CACHE_TTL)
    }
}

/// 单次 OpenRouter 查询的三态结论。
///
/// 只有 [`Self::Miss`] 断言"公共目录确无该模型",且仅在目录数据新鲜
/// （内存/磁盘在 24h TTL 内,或刚成功拉取）时返回:过期数据上未命中只
/// 证明数据旧,上游解析链不得据此下长期负结论。命中则不受新鲜度约束——
/// 过期目录给出的窗口仍是一个可用的答案。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenRouterLookup {
    /// 命中:目录中有该模型的上下文窗口。
    Found(u64),
    /// 新鲜数据中确无该模型。
    Miss,
    /// 目录不可用(查询被禁用/拉取失败/只剩过期数据可对照)。
    Unavailable,
}

/// 目录数据的新鲜度,决定"未命中"是否可信。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatalogFreshness {
    Fresh,
    Stale,
}

pub(crate) struct OpenRouterCatalog {
    url: String,
    cache_path: Option<PathBuf>,
    http: reqwest::Client,
    memory: RwLock<Option<CachedCatalog>>,
}

impl OpenRouterCatalog {
    /// 按环境变量构造（URL 覆盖 + 默认磁盘缓存路径）。
    pub(crate) fn from_env() -> Self {
        let url = std::env::var(URL_ENV)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MODELS_URL.to_string());
        Self::with_url_and_cache(url, default_cache_path())
    }

    /// 显式指定 URL 与缓存路径（测试注入用；`cache_path = None` 关闭磁盘缓存）。
    pub(crate) fn with_url_and_cache(url: String, cache_path: Option<PathBuf>) -> Self {
        Self {
            url,
            cache_path,
            http: super::model_catalog_http_client(FETCH_TIMEOUT),
            memory: RwLock::new(None),
        }
    }

    /// 查询模型上下文窗口；过期数据上的未命中报告为
    /// [`OpenRouterLookup::Unavailable`]，不当作结论。
    ///
    /// 注意：禁用开关（`XIAOO_DISABLE_OPENROUTER_LOOKUP`）由单例门面
    /// [`lookup_context_length`] 检查，显式构造的实例不受其影响（测试
    /// 注入用）。
    pub(crate) async fn lookup_context_length(&self, model: &str) -> OpenRouterLookup {
        let Some((entries, freshness)) = self.load_entries().await else {
            return OpenRouterLookup::Unavailable;
        };
        match match_model(&entries, model) {
            Some(context) => OpenRouterLookup::Found(context),
            // 过期数据上的未命中不是证据：目录可能只是旧了。
            None if freshness == CatalogFreshness::Stale => OpenRouterLookup::Unavailable,
            None => OpenRouterLookup::Miss,
        }
    }

    /// 加载条目——回落链见模块 `//!`。过期缓存保留的原因：它仍比裸协议族
    /// 静态兜底值更接近真实窗口，可作离线兜底；但以 [`CatalogFreshness::Stale`]
    /// 标注，未命中不得视为"目录没有该模型"。
    async fn load_entries(&self) -> Option<(Vec<CatalogEntry>, CatalogFreshness)> {
        if let Some(cached) = self.memory.read().ok().and_then(|guard| guard.clone()) {
            if cached.fresh() {
                return Some((cached.entries, CatalogFreshness::Fresh));
            }
        }

        // 磁盘缓存只读一次：新鲜则直接采用（并回填内存，后续查询免磁盘
        // IO），过期则留作 HTTP 失败后的离线兜底。
        let stale_disk = match self.read_disk_cache() {
            Some(cached) if cached.fresh() => {
                if let Ok(mut guard) = self.memory.write() {
                    *guard = Some(cached.clone());
                }
                return Some((cached.entries, CatalogFreshness::Fresh));
            }
            stale => stale,
        };

        match self.fetch().await {
            Ok(cached) => {
                self.persist_to_disk(&cached);
                if let Ok(mut guard) = self.memory.write() {
                    *guard = Some(cached.clone());
                }
                return Some((cached.entries, CatalogFreshness::Fresh));
            }
            Err(error) => {
                tracing::warn!(
                    url = %self.url,
                    error = %error,
                    "openrouter model catalog fetch failed; falling back to cache"
                );
            }
        }

        if let Some(cached) = stale_disk {
            return Some((cached.entries, CatalogFreshness::Stale));
        }
        self.memory.read().ok().and_then(|guard| {
            guard
                .clone()
                .map(|cached| (cached.entries, CatalogFreshness::Stale))
        })
    }

    async fn fetch(&self) -> Result<CachedCatalog, LlmError> {
        let mut redactor = crate::error::ErrorRedactor::new(&self.url, "");
        let result = async {
            let request = self
                .http
                .get(&self.url)
                .build()
                .map_err(map_reqwest_error)?;
            redactor.headers(request.headers());
            let response = self
                .http
                .execute(request)
                .await
                .map_err(map_reqwest_error)?;
            redactor.headers(response.headers());
            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(LlmError::ApiError(format!(
                    "OpenRouter models request failed: HTTP {status}: {body}"
                )));
            }
            let payload: serde_json::Value = response.json().await.map_err(map_reqwest_error)?;
            parse_catalog_payload(&payload)
        }
        .await;
        redactor.error_result(result)
    }

    fn persist_to_disk(&self, cached: &CachedCatalog) {
        let Some(path) = &self.cache_path else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string(cached) {
            Ok(serialized) => {
                if let Err(error) = std::fs::write(path, serialized) {
                    tracing::warn!(
                        path = %path.display(),
                        %error,
                        "failed to persist openrouter model cache"
                    );
                }
            }
            Err(error) => {
                tracing::warn!(%error, "failed to serialize openrouter model cache");
            }
        }
    }

    fn read_disk_cache(&self) -> Option<CachedCatalog> {
        let path = self.cache_path.as_ref()?;
        let raw = std::fs::read_to_string(path).ok()?;
        match serde_json::from_str(&raw) {
            Ok(cached) => Some(cached),
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "ignoring corrupt openrouter model cache"
                );
                None
            }
        }
    }
}

fn default_cache_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|dir| dir.join("xiaoo").join(CACHE_FILE_NAME))
}

fn lookup_disabled() -> bool {
    match std::env::var(DISABLE_ENV) {
        Ok(value) => !matches!(value.trim(), "" | "0" | "false"),
        Err(_) => false,
    }
}

/// 解析 OpenRouter `GET /api/v1/models` 响应。
fn parse_catalog_payload(payload: &serde_json::Value) -> Result<CachedCatalog, LlmError> {
    let data = payload
        .get("data")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            LlmError::ParseError("OpenRouter models payload missing `data` array".to_string())
        })?;
    let entries = data
        .iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?.to_string();
            let context_length = item.get("context_length").and_then(|v| v.as_u64());
            Some(CatalogEntry { id, context_length })
        })
        .collect();
    let fetched_at_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    Ok(CachedCatalog {
        fetched_at_unix,
        entries,
    })
}

/// 在 OpenRouter 条目中查找模型的上下文窗口。
///
/// OpenRouter 的 id 带 vendor 前缀（`z-ai/glm-5.3`），而用户配置通常只写
/// 裸模型名（`glm-5.3`）。匹配顺序：
/// 1. 全 id 精确相等（用户写全 `z-ai/glm-5.3` 时，含 `:batch` 变体）；
/// 2. 去前缀后的末段精确相等（`glm-5.3` ↔ `z-ai/glm-5.3`），跳过
///    `:batch` / `:free` 等变体，避免它们抢先命中。
///
/// 末段必须整段相等：`glm-5.3-prime` 不会匹配 `glm-5.3`。
fn match_model(entries: &[CatalogEntry], model: &str) -> Option<u64> {
    let target = normalize_model_id(model);
    if let Some(exact) = entries
        .iter()
        .find(|entry| normalize_model_id(&entry.id) == target)
    {
        return exact.context_length;
    }
    let target_last = last_segment(&target);
    if target_last.is_empty() {
        return None;
    }
    entries
        .iter()
        .filter(|entry| !is_variant_id(&entry.id))
        .find(|entry| last_segment(&normalize_model_id(&entry.id)) == target_last)
        .and_then(|entry| entry.context_length)
}

fn last_segment(model: &str) -> &str {
    model.rsplit('/').next().unwrap_or_default()
}

fn is_variant_id(id: &str) -> bool {
    id.contains(':')
}

/// 进程级单例门面：所有 provider 装配共享同一份缓存。
/// `XIAOO_DISABLE_OPENROUTER_LOOKUP` 在此生效——显式构造的实例不受影响。
/// 禁用时报告 [`OpenRouterLookup::Unavailable`]：没做过的查询不能当"确证未命中"。
pub(crate) async fn lookup_context_length(model: &str) -> OpenRouterLookup {
    if lookup_disabled() {
        return OpenRouterLookup::Unavailable;
    }
    static CATALOG: OnceLock<OpenRouterCatalog> = OnceLock::new();
    CATALOG
        .get_or_init(OpenRouterCatalog::from_env)
        .lookup_context_length(model)
        .await
}

#[cfg(test)]
#[path = "../../../../tests/unit/llm-client/models/openrouter_test.rs"]
mod tests;
