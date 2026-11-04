//! 准入闸门单测。触及进程级静态（GLOBAL/REGISTRY）的用例持
//! [`STATIC_STATE_LOCK`] 串行，其余用例用独立 limiter 实例并行。

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

use agent_types::llm::{
    AssistantMessage, ChatMessage, ContentBlock, MessageRole, ReasoningEffort, StopReason, Usage,
};
use async_trait::async_trait;

/// GLOBAL/REGISTRY 是进程级静态，并发测试会互相踩；触及它们的用例持此锁
/// 串行。用 tokio Mutex 而非 std：guard 需要跨 await 持有。
static STATIC_STATE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 恢复 GLOBAL/REGISTRY 到"非显式默认"的出厂态。测试与 src 是父子模块
/// 关系，直接改私有静态即可，避免在 src 里留 cfg(test) 门控项
/// （tests-hygiene 门禁只允许唯一形式的 #[cfg(test)] #[path] mod 声明）。
fn reset_for_test() {
    *GLOBAL
        .lock()
        .expect("rate-limit global lock should not be poisoned") = GlobalState {
        settings: RateLimitSettings::default(),
        explicit: false,
    };
    REGISTRY
        .lock()
        .expect("rate-limit registry lock should not be poisoned")
        .clear();
}

// ── 测试数据构造 ─────────────────────────────────────────────────────────────

fn make_request(text: &str) -> LlmRequest {
    LlmRequest {
        messages: vec![ChatMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            message_id: None,
            timestamp_ms: 0,
            api_usage_tokens: None,
            reasoning_content: None,
            estimated_tokens: None,
        }],
        tools: vec![],
        tool_choice: Default::default(),
        max_tokens: None,
        temperature: None,
        response_format: Default::default(),
        reasoning_effort: ReasoningEffort::Off,
    }
}

fn make_response() -> LlmResponse {
    LlmResponse {
        message: AssistantMessage {
            text: Some("probe".to_string()),
            reasoning_content: None,
            tool_calls: vec![],
            usage: Usage {
                cache_write_tokens: 0,
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
                cached_tokens: 0,
            },
            stop_reason: StopReason::EndTurn,
        },
        kv_cache_chunk_hashes: vec![],
    }
}

/// 记录在途/峰值/总数与每次请求进场时刻的探针 provider。`body_delay`
/// 模拟请求体耗时（流式路径下即 permit 应被持有的时长）。
struct ProbeProvider {
    in_flight: AtomicUsize,
    peak: AtomicUsize,
    total: AtomicUsize,
    fired_at: Mutex<Vec<tokio::time::Instant>>,
    body_delay: Option<Duration>,
    caps: ProviderCapabilities,
}

impl ProbeProvider {
    fn new(body_delay: Option<Duration>) -> Self {
        Self {
            in_flight: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            total: AtomicUsize::new(0),
            fired_at: Mutex::new(Vec::new()),
            body_delay,
            caps: ProviderCapabilities {
                supports_streaming: true,
                supports_tool_calls: false,
                supports_json_mode: false,
                max_context_window: 4096,
                model_name: "probe-model".to_string(),
            },
        }
    }

    async fn enter(&self) {
        let in_flight = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(in_flight, Ordering::SeqCst);
        self.total.fetch_add(1, Ordering::SeqCst);
        self.fired_at
            .lock()
            .expect("fired_at lock should not be poisoned")
            .push(tokio::time::Instant::now());
        if let Some(delay) = self.body_delay {
            tokio::time::sleep(delay).await;
        }
    }

    fn exit(&self) {
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl LlmProvider for ProbeProvider {
    async fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        self.enter().await;
        self.exit();
        Ok(make_response())
    }

    async fn complete_stream(
        &self,
        _request: &LlmRequest,
        on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
    ) -> Result<LlmResponse, LlmError> {
        self.enter().await;
        on_chunk(StreamChunk {
            reset: false,
            delta_text: Some("probe".to_string()),
            delta_reasoning: None,
            delta_tool_call: None,
        });
        self.exit();
        Ok(make_response())
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.caps
    }
}

fn limiter_with(max_concurrent: usize, requests_per_minute: u32) -> Arc<EndpointLimiter> {
    Arc::new(EndpointLimiter::new(
        RateLimitSettings {
            max_concurrent,
            requests_per_minute,
        },
        "http://probe".to_string(),
    ))
}

/// 并发发 N 次 complete，全部要求成功。
async fn run_concurrent_completes(gated: Arc<dyn LlmProvider>, count: usize) {
    let calls: Vec<_> = (0..count)
        .map(|_| {
            let gated = gated.clone();
            tokio::spawn(async move { gated.complete(&make_request("hi")).await.is_ok() })
        })
        .collect();
    let results = futures::future::join_all(calls).await;
    assert!(results
        .into_iter()
        .all(|result| result.expect("probe task panicked")));
}

// ── 令牌桶数学 ───────────────────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn token_bucket_burst_is_capacity_and_refill_is_continuous() {
    let t0 = tokio::time::Instant::now();
    let mut bucket = TokenBucket::new(4, 1.0, t0);
    // 初始突发额度 = 容量（= 并发上限）
    for _ in 0..4 {
        assert_eq!(bucket.try_acquire(t0), None);
    }
    // 桶空：第 5 个令牌要等一个补充周期
    assert_eq!(bucket.try_acquire(t0), Some(Duration::from_secs(1)));
    // 长时间静默后补充但封顶在容量：90s 只回 4 个（而非 90 个），
    // 取满后第 5 个仍要等
    let t90 = t0 + Duration::from_secs(90);
    for _ in 0..4 {
        assert_eq!(bucket.try_acquire(t90), None);
    }
    assert_eq!(bucket.try_acquire(t90), Some(Duration::from_secs(1)));
}

#[tokio::test(start_paused = true)]
async fn token_wait_delays_request_until_refill() {
    let limiter = limiter_with(8, 60);
    // 桶容量 8：前 8 次即刻放行（guard 逐个释放 permit）
    for _ in 0..8 {
        let _gate = limiter.acquire().await;
    }
    let started = tokio::time::Instant::now();
    let _gate = limiter.acquire().await;
    // 桶已空：第 9 个令牌要等一个补充周期（60 rpm → 1s）
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert!(started.elapsed() < Duration::from_secs(2));
}

// ── 并发与速率不变量 ─────────────────────────────────────────────────────────

#[tokio::test]
async fn concurrency_peak_is_capped_by_permits() {
    let provider = Arc::new(ProbeProvider::new(Some(Duration::from_millis(25))));
    let gated = gate_with_limiter(provider.clone(), limiter_with(4, 1200));
    // 装饰器透传 capabilities
    assert_eq!(gated.capabilities().model_name, "probe-model");
    run_concurrent_completes(gated, 20).await;
    assert_eq!(provider.total.load(Ordering::SeqCst), 20);
    assert!(provider.peak.load(Ordering::SeqCst) <= 4);
    // permit 确实在并行使用（非退化串行）
    assert!(provider.peak.load(Ordering::SeqCst) >= 2);
}

#[tokio::test(start_paused = true)]
async fn requests_in_any_sixty_second_window_stay_under_rpm_plus_burst() {
    let provider = Arc::new(ProbeProvider::new(None));
    let gated = gate_with_limiter(provider.clone(), limiter_with(8, 60));
    let started = tokio::time::Instant::now();
    run_concurrent_completes(gated, 100).await;
    assert_eq!(provider.total.load(Ordering::SeqCst), 100);
    let fired_at = provider
        .fired_at
        .lock()
        .expect("fired_at lock should not be poisoned")
        .clone();
    let in_first_minute = fired_at
        .iter()
        .filter(|instant| instant.duration_since(started) <= Duration::from_secs(60))
        .count();
    // 核心不变量：任意 60s 窗口放行 ≤ rpm + 桶容量。used to 按字面实现
    // "容量=rpm" 的令牌桶，最坏窗口放行 2×rpm，照样打爆配额。
    assert!(in_first_minute <= 68, "in_first_minute={in_first_minute}");
    // 100 个请求需要 (100-8) 个补充周期，每个 1s
    assert!(started.elapsed() >= Duration::from_secs(92));
}

// ── 流式 permit 语义 ────────────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn stream_permit_spans_body_and_releases_on_abort() {
    let slow = Arc::new(ProbeProvider::new(Some(Duration::from_secs(60))));
    let fast = Arc::new(ProbeProvider::new(None));
    let limiter = limiter_with(1, 600);
    let gated_slow = gate_with_limiter(slow.clone(), limiter.clone());
    let gated_slow2 = gate_with_limiter(slow.clone(), limiter.clone());
    let gated_fast = gate_with_limiter(fast.clone(), limiter.clone());

    let on_chunk1 = |_: StreamChunk| {};
    let on_chunk2 = |_: StreamChunk| {};
    let first = tokio::spawn(async move {
        gated_slow
            .complete_stream(&make_request("hi"), &on_chunk1)
            .await
    });
    // 等第一个流确实进场（新桶突发令牌即刻可用，无需推进时钟）
    while slow.in_flight.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }

    // 唯一 permit 已被第一个流占用：第二个流必须留在场外
    let second = tokio::spawn(async move {
        gated_slow2
            .complete_stream(&make_request("hi"), &on_chunk2)
            .await
    });
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert_eq!(slow.in_flight.load(Ordering::SeqCst), 1);
    second.abort();
    let _ = second.await;

    // abort 第一个流：permit 应随 future drop 立即释放，fast 无需等 60s 体
    first.abort();
    let _ = first.await;
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        gated_fast.complete(&make_request("hi")),
    )
    .await;
    assert!(outcome.is_ok_and(|result| result.is_ok()));
    assert_eq!(fast.total.load(Ordering::SeqCst), 1);
}

// ── 全局注册表与设置开关 ────────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn registry_shares_limiter_across_providers_and_isolates_keys() {
    let _guard = STATIC_STATE_LOCK.lock().await;
    reset_for_test();
    // 未显式配置时进程内即为默认值
    assert_eq!(global_settings(), RateLimitSettings::default());

    let provider = Arc::new(ProbeProvider::new(None));
    let gated_a = gate_provider(
        provider.clone(),
        "http://gw",
        Some("key-1".to_string()),
        false,
    );
    // 装满突发额度（默认容量 8）
    for _ in 0..8 {
        assert!(gated_a.complete(&make_request("hi")).await.is_ok());
    }

    // 同端点新建 provider：共享同一限流器，第 9 次必须等补充（默认 40 rpm）
    let gated_b = gate_provider(
        provider.clone(),
        "http://gw",
        Some("key-1".to_string()),
        false,
    );
    let started = tokio::time::Instant::now();
    assert!(gated_b.complete(&make_request("hi")).await.is_ok());
    assert!(started.elapsed() >= Duration::from_secs(1));

    // 不同 key = 不同配额主体：独立限流器，立即放行
    let gated_c = gate_provider(
        provider.clone(),
        "http://gw",
        Some("key-2".to_string()),
        false,
    );
    let started = tokio::time::Instant::now();
    assert!(gated_c.complete(&make_request("hi")).await.is_ok());
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn disabled_settings_bypass_the_gate() {
    let _guard = STATIC_STATE_LOCK.lock().await;
    // 任一维为 0 即整体关闭
    for settings in [
        RateLimitSettings {
            max_concurrent: 0,
            requests_per_minute: 40,
        },
        RateLimitSettings {
            max_concurrent: 8,
            requests_per_minute: 0,
        },
    ] {
        reset_for_test();
        assert!(!settings.is_enabled());
        apply_global_settings(settings);

        let provider = Arc::new(ProbeProvider::new(Some(Duration::from_millis(25))));
        let gated = gate_provider(provider.clone(), "http://gw", Some("k".to_string()), false);
        run_concurrent_completes(gated, 20).await;
        assert_eq!(provider.total.load(Ordering::SeqCst), 20);
        // 20 个全部同时在体：峰值超过任何可能 permit 数，证明未包装
        assert!(provider.peak.load(Ordering::SeqCst) > 8);
    }
}

#[tokio::test]
async fn local_endpoint_skipped_under_defaults_but_gated_when_explicit() {
    let _guard = STATIC_STATE_LOCK.lock().await;
    reset_for_test();

    // 未显式配置：本地端点豁免（Ollama 无外部配额）
    let provider = Arc::new(ProbeProvider::new(Some(Duration::from_millis(25))));
    let gated = gate_provider(provider.clone(), "http://localhost:11434", None, true);
    run_concurrent_completes(gated, 20).await;
    assert!(provider.peak.load(Ordering::SeqCst) > 8);

    // 显式配置后：本地端点也受闸门
    apply_global_settings(RateLimitSettings {
        max_concurrent: 4,
        requests_per_minute: 1200,
    });
    let provider = Arc::new(ProbeProvider::new(Some(Duration::from_millis(25))));
    let gated = gate_provider(provider.clone(), "http://localhost:11434", None, true);
    run_concurrent_completes(gated, 20).await;
    assert_eq!(provider.total.load(Ordering::SeqCst), 20);
    assert!(provider.peak.load(Ordering::SeqCst) <= 4);
}

#[test]
fn default_settings_match_the_incident_gateway_margin() {
    let defaults = RateLimitSettings::default();
    // 8/40 对 10 并发/50 rpm 配额：两个维度的余量都是事故复盘的结论值，
    // 改默认值必须同步复核最坏窗口算术（见 TokenBucket 容量注释）
    assert_eq!(defaults.max_concurrent, DEFAULT_MAX_CONCURRENT);
    assert_eq!(defaults.requests_per_minute, DEFAULT_REQUESTS_PER_MINUTE);
    assert!(defaults.is_enabled());
}
