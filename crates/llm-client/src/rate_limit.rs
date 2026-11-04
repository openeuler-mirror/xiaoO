//! LLM 请求准入闸门：进程内并发信号量 + 按端点共享的令牌桶速率限制。
//!
//! 动机：编排器扇出（多 lane + 毫秒级工具背靠背调用）可以轻易越过网关的
//! 并发/速率配额；429 退避（`request_policy`）只能事后补救，且风暴形态下
//! 重试本身继续消耗配额。本模块在请求发出之前做源头准入，把速率压在
//! 配额线以下，而不是打爆后再退避。
//!
//! 结构与契约：
//! - [`gate_provider`] 在工厂装配处把裸 provider 包成 `GatedProvider`，
//!   位于 `LlmProviderWrapper`（trace/hook/重试循环）之内：每次重试尝试
//!   都经过闸门计数，因为重试同样消耗网关配额。
//! - 限流器按 `(api_base, api_key)` 经 [`REGISTRY`] 全局共享：daemon 单
//!   进程承载所有 lane，但每次 session resolve 都会新建 wrapper/provider
//!   实例，per-instance 限流无法覆盖全局流量。
//! - 流式调用在整个流期间持有并发 permit（流本身占用网关并发额度），
//!   future 被 drop（用户取消）时 permit 随之释放。
//! - 设置为进程内全局态（[`apply_global_settings`]），daemon 在配置装载
//!   时调用一次；tool-cli 等独立进程各自持有独立闸门，调用量极低可接受。
//!   多进程共享同一 key 时进程间无协同，残余越限由退避层兜底。
//! - 闸门位于重试循环之内（见上），排队等待（permit + 令牌）会计入
//!   `request_policy` 的单次尝试与总预算：饱和时排队过久的非流式请求会
//!   以 `Timeout` 失败并可重试，而网关从未收到它。这是"准入延迟算作调用
//!   耗时"的有意取舍，排障时用 `llm rate-limit gate throttled` 日志区分
//!   闸门排队与网关超时。流式调用无总预算，排队只受 permit/令牌约束。

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use agent_contracts::{LlmProvider, ProviderCapabilities};
use agent_types::llm::{LlmError, LlmRequest, LlmResponse, StreamChunk};
use async_trait::async_trait;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// 默认并发上限 8：事故网关实测并发上限 10，留 2 个余量吸收网关侧统计
/// 口径差与在途判定误差——贴满 10 只会把拒绝转嫁给退避层。
const DEFAULT_MAX_CONCURRENT: usize = 8;

/// 默认速率 40 req/min：事故网关配额 50 req/min，留 20% 余量吸收时钟
/// 偏移与滑窗边界效应。单 lane 交互式使用峰值 ~5 req/min，无感知。
const DEFAULT_REQUESTS_PER_MINUTE: u32 = 40;

/// 排队 ≥1s 才升到 info：高频短等待刷屏会淹没有效信号，值得人看到的
/// 只有"闸门已成为瓶颈"的时刻。
const QUEUE_LOG_THRESHOLD: Duration = Duration::from_secs(1);

/// 进程内准入闸门设置：`max_concurrent` 为同时在途请求上限，
/// `requests_per_minute` 为持续补充的速率上限。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitSettings {
    pub max_concurrent: usize,
    pub requests_per_minute: u32,
}

impl Default for RateLimitSettings {
    fn default() -> Self {
        Self {
            max_concurrent: DEFAULT_MAX_CONCURRENT,
            requests_per_minute: DEFAULT_REQUESTS_PER_MINUTE,
        }
    }
}

impl RateLimitSettings {
    /// 任一维为 0 即整体关闭。不提供"只关一维"：令牌桶容量与并发上限
    /// 耦合（见 [`TokenBucket`] 的容量算术），单维关闭会让最坏窗口
    /// 上界失去意义。
    fn is_enabled(&self) -> bool {
        self.max_concurrent > 0 && self.requests_per_minute > 0
    }
}

struct GlobalState {
    settings: RateLimitSettings,
    /// 是否经 [`apply_global_settings`] 显式配置。本地端点（Ollama）没有
    /// 外部配额，仅在显式配置后才受闸门约束，避免默认值无谓压制本地扇出。
    explicit: bool,
}

static GLOBAL: LazyLock<Mutex<GlobalState>> = LazyLock::new(|| {
    Mutex::new(GlobalState {
        settings: RateLimitSettings::default(),
        explicit: false,
    })
});

/// 设置进程内闸门参数。必须在创建任何 provider 之前调用（daemon 在配置
/// 装载时调用）：已创建的 provider 持有限流器快照，之后的变更只影响
/// 此后新端点的注册表项。
pub fn apply_global_settings(settings: RateLimitSettings) {
    let mut global = GLOBAL
        .lock()
        .expect("rate-limit global lock should not be poisoned");
    global.settings = settings;
    global.explicit = true;
}

/// 读取当前进程内闸门参数（诊断与配置装载测试用）。
pub fn global_settings() -> RateLimitSettings {
    GLOBAL
        .lock()
        .expect("rate-limit global lock should not be poisoned")
        .settings
}

/// 连续补充令牌桶。时间源用 `tokio::time::Instant` 而非 std：闸门时序是
/// 可测行为，`start_paused` 假时钟才能驱动确定性的速率断言。
///
/// 桶容量（突发额度）取 `max_concurrent` 而非 rpm：突发额度超过并发上限
/// 并无意义（多余令牌只能排队等 permit），且 [`EndpointLimiter::acquire`]
/// 在 permit 到手后才扣减令牌、扣减后立即发出请求，令牌扣减时刻即发送
/// 时刻，故任意 60s 滑窗内实际发出的请求数 ≤ rpm + 桶容量 =
/// rpm + max_concurrent。默认 40 + 8 = 48，仍在事故网关 50/min 配额之下；
/// 调参时按同一算术自行留余量。
struct TokenBucket {
    tokens: f64,
    capacity: f64,
    refill_per_sec: f64,
    last_refill: tokio::time::Instant,
}

impl TokenBucket {
    fn new(capacity: usize, refill_per_sec: f64, now: tokio::time::Instant) -> Self {
        Self {
            tokens: capacity as f64,
            capacity: capacity as f64,
            refill_per_sec,
            last_refill: now,
        }
    }

    /// 尝试取一个令牌：成功返回 `None`；失败返回距下一个令牌可用的等待
    /// 时长，调用方 sleep 后重试。并发等待者同时醒来时只有一个能取到，
    /// 其余按新缺口重新计算——桶容量小（= 并发上限），惊群规模有限，
    /// 不值得引入 FIFO 公平队列。
    fn try_acquire(&mut self, now: tokio::time::Instant) -> Option<Duration> {
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        if elapsed > 0.0 {
            self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
            self.last_refill = now;
        }
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            None
        } else {
            let deficit = 1.0 - self.tokens;
            Some(Duration::from_secs_f64(deficit / self.refill_per_sec))
        }
    }
}

/// 单个端点（同一 api_base + api_key，即同一配额主体）的准入控制。
pub(crate) struct EndpointLimiter {
    semaphore: Arc<Semaphore>,
    bucket: Mutex<TokenBucket>,
    /// 仅含 api_base，用于日志定位端点。api_key 绝不落入此字段——
    /// 日志不泄密。
    endpoint_label: String,
}

impl EndpointLimiter {
    /// 仅在 [`RateLimitSettings::is_enabled`] 为真时构造；禁用态由
    /// [`gate_provider`] 在更外层直接透传，不进入本类型。
    pub(crate) fn new(settings: RateLimitSettings, endpoint_label: String) -> Self {
        debug_assert!(
            settings.is_enabled(),
            "rate-limit limiter must not be constructed for disabled settings"
        );
        Self {
            semaphore: Arc::new(Semaphore::new(settings.max_concurrent)),
            bucket: Mutex::new(TokenBucket::new(
                settings.max_concurrent,
                settings.requests_per_minute as f64 / 60.0,
                tokio::time::Instant::now(),
            )),
            endpoint_label,
        }
    }

    /// 先取并发 permit、再取速率令牌，次序不可反：令牌扣减时刻必须贴近
    /// 请求真正发出的时刻。若先扣令牌再等 permit，长请求（尤其流式）长期
    /// 占满 permit 时，排队者会攒下大量"已扣未发"的令牌，permit 一释放便
    /// 集中发出，使任意 60s 窗口的实际发出量突破 `rpm + max_concurrent`
    /// （见 [`TokenBucket`] 的窗口算术）。
    /// 持 permit 等令牌不损失吞吐：令牌稀缺即速率是瓶颈，不占 permit 的
    /// 请求同样无法发出；反之令牌充足时 permit 到手即扣令牌、无等待。
    /// 取消安全性：等 permit 期间被 drop 不持有任何资源；持 permit 等令牌
    /// 期间被 drop 只释放 permit、不浪费令牌。
    pub(crate) async fn acquire(&self) -> GateGuard {
        let queued_at = tokio::time::Instant::now();
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("rate-limit semaphore is never closed");
        loop {
            let wait = {
                let mut bucket = self
                    .bucket
                    .lock()
                    .expect("rate-limit bucket lock should not be poisoned");
                bucket.try_acquire(tokio::time::Instant::now())
            };
            match wait {
                None => break,
                Some(wait) => tokio::time::sleep(wait).await,
            }
        }
        let queued = queued_at.elapsed();
        if queued >= QUEUE_LOG_THRESHOLD {
            tracing::info!(
                endpoint = %self.endpoint_label,
                queue_ms = queued.as_millis() as u64,
                "llm rate-limit gate throttled a request"
            );
        } else {
            tracing::debug!(
                endpoint = %self.endpoint_label,
                queue_ms = queued.as_millis() as u64,
                "llm rate-limit gate admitted a request"
            );
        }
        GateGuard { _permit: permit }
    }
}

/// 闸门通行证：持有期间占用一个并发名额，drop 时释放。速率令牌不随
/// guard 持有——它在放行瞬间即已计入窗口。
pub(crate) struct GateGuard {
    _permit: OwnedSemaphorePermit,
}

/// 准入闸门装饰器：包在裸 provider 外、`LlmProviderWrapper` 与重试循环
/// 之内。每次 `complete`/`complete_stream` 调用（含每次重试尝试）都各自
/// 过一次闸门；流式调用在整个流期间持有 permit，future 被 drop（用户
/// 取消）时随 RAII 自动释放。
struct GatedProvider {
    inner: Arc<dyn LlmProvider>,
    limiter: Arc<EndpointLimiter>,
}

#[async_trait]
impl LlmProvider for GatedProvider {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let _gate = self.limiter.acquire().await;
        self.inner.complete(request).await
    }

    async fn complete_stream(
        &self,
        request: &LlmRequest,
        on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
    ) -> Result<LlmResponse, LlmError> {
        let _gate = self.limiter.acquire().await;
        self.inner.complete_stream(request, on_chunk).await
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        self.inner.capabilities()
    }
}

/// 注册表键：`(规范化 api_base, api_key)`。配额按 key 计，与 model 无关，
/// 故 model 不进键。
type EndpointKey = (String, Option<String>);

static REGISTRY: LazyLock<Mutex<HashMap<EndpointKey, Arc<EndpointLimiter>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 工厂装配入口：按当前全局设置决定是否给裸 provider 套闸门。
/// `local_endpoint` 标记本地运行时（Ollama）——无外部配额，仅在设置被
/// 显式配置后才约束。
pub(crate) fn gate_provider(
    raw: Arc<dyn LlmProvider>,
    api_base: &str,
    api_key: Option<String>,
    local_endpoint: bool,
) -> Arc<dyn LlmProvider> {
    let (settings, explicit) = {
        let global = GLOBAL
            .lock()
            .expect("rate-limit global lock should not be poisoned");
        (global.settings, global.explicit)
    };
    if !settings.is_enabled() || (local_endpoint && !explicit) {
        return raw;
    }
    // 空 key 与 None 同属无鉴权端点，归一后共享同一限流器（同一配额主体）。
    let key: EndpointKey = (
        api_base.to_string(),
        api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    );
    let limiter = {
        let mut registry = REGISTRY
            .lock()
            .expect("rate-limit registry lock should not be poisoned");
        registry
            .entry(key)
            .or_insert_with(|| Arc::new(EndpointLimiter::new(settings, api_base.to_string())))
            .clone()
    };
    gate_with_limiter(raw, limiter)
}

/// 用指定限流器包装裸 provider：`gate_provider` 的装配尾部，也供测试
/// 绕过全局注册表直接构造。
pub(crate) fn gate_with_limiter(
    raw: Arc<dyn LlmProvider>,
    limiter: Arc<EndpointLimiter>,
) -> Arc<dyn LlmProvider> {
    Arc::new(GatedProvider {
        inner: raw,
        limiter,
    })
}

#[cfg(test)]
#[path = "../../../tests/unit/llm-client/rate_limit_test.rs"]
mod tests;
