use super::*;
use tempfile::TempDir;

fn temp_state() -> (TempDir, AppState) {
    let temp_dir = TempDir::new().expect("create temp dir");
    let config_path = temp_dir.path().join("nested").join("config.toml");
    let workspace = temp_dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("create workspace");
    let state = AppState::new(config_path, workspace).expect("create app state");
    (temp_dir, state)
}

#[test]
fn persist_active_provider_selection_writes_llm_to_app_config_path() {
    let (_temp_dir, mut state) = temp_state();
    let config_path = state.config_path.clone();

    persist_active_provider_selection(
        &mut state,
        "openai".to_string(),
        "gpt-4o".to_string(),
        Some("OPENAI_API_KEY".to_string()),
        "https://api.openai.com/v1".to_string(),
    );

    let saved = Config::load_from(&config_path).expect("load saved config");
    assert_eq!(saved.llm.provider, "openai");
    assert_eq!(saved.llm.model, "gpt-4o");
    assert_eq!(saved.llm.api_key_env.as_deref(), Some("OPENAI_API_KEY"));
    assert_eq!(saved.llm.api_base, "https://api.openai.com/v1");
}

#[test]
fn persisted_selection_settings_switching_provider_uses_new_provider_defaults() {
    let (_temp_dir, mut state) = temp_state();
    state.agent_config.llm.provider = "openai".to_string();
    state.agent_config.llm.model = "gpt-4o".to_string();
    state.agent_config.llm.api_key_env = Some("OPENAI_API_KEY".to_string());
    state.agent_config.llm.api_base = "https://api.openai.com/v1".to_string();

    let (api_key_env, api_base) = persisted_selection_settings(&state.agent_config, "deepseek");

    assert_eq!(api_key_env.as_deref(), Some("DEEPSEEK_API_KEY"));
    assert_eq!(api_base, "https://api.deepseek.com/v1");
}

#[test]
fn persisted_selection_settings_same_provider_preserves_existing_config() {
    let (_temp_dir, mut state) = temp_state();
    state.agent_config.llm.provider = "openai".to_string();
    state.agent_config.llm.model = "gpt-4o".to_string();
    state.agent_config.llm.api_key_env = Some("CUSTOM_OPENAI_KEY".to_string());
    state.agent_config.llm.api_base = "https://proxy.example/v1".to_string();

    let (api_key_env, api_base) = persisted_selection_settings(&state.agent_config, "openai");

    assert_eq!(api_key_env.as_deref(), Some("CUSTOM_OPENAI_KEY"));
    assert_eq!(api_base, "https://proxy.example/v1");
}

#[test]
fn default_api_base_for_openai_is_explicit() {
    assert_eq!(
        default_api_base_for_provider("openai"),
        "https://api.openai.com/v1"
    );
}

#[test]
fn coding_plan_provider_defaults_are_explicit() {
    assert_eq!(
        default_api_key_env_for_provider("minimax").as_deref(),
        Some("MINIMAX_API_KEY")
    );
    assert_eq!(
        default_api_base_for_provider("minimax"),
        "https://api.minimaxi.com/v1"
    );
    assert_eq!(
        default_api_key_env_for_provider("kimi").as_deref(),
        Some("MOONSHOT_API_KEY")
    );
    assert_eq!(
        default_api_base_for_provider("kimi"),
        "https://api.moonshot.cn/v1"
    );
    assert_eq!(
        default_api_key_env_for_provider("minimax-coding-plan").as_deref(),
        Some("MINIMAX_API_KEY")
    );
    assert_eq!(
        default_api_base_for_provider("minimax-coding-plan"),
        "https://api.minimax.io/v1"
    );
    assert_eq!(
        default_api_key_env_for_provider("kimi-coding-plan").as_deref(),
        Some("KIMI_API_KEY")
    );
    assert_eq!(
        default_api_base_for_provider("kimi-coding-plan"),
        "https://api.kimi.com/coding/v1"
    );
}

/// Known-good terminals: TERM / TERM_PROGRAM sniffing must recognise every
/// terminal that implements OSC 52, including `TERM`-suffix variants such as
/// `alacritty-direct` / `foot-extra`, plus the WT_SESSION (Windows Terminal)
/// and KITTY_WINDOW_ID (kitty) signals.
#[test]
fn osc52_supported_by_terminal_detects_known_good_terminals() {
    // TERM-based detection (kitty / Alacritty / WezTerm / ghostty / foot /
    // rio / contour / mintty), including suffixed variants.
    for term in [
        "xterm-kitty",
        "alacritty",
        "alacritty-direct",
        "wezterm",
        "xterm-ghostty",
        "ghostty",
        "foot",
        "foot-extra",
        "rio",
        "contour",
        "mintty",
    ] {
        assert!(
            osc52_supported_by_terminal(Some(term), None, false, false),
            "TERM={term} should be detected as OSC 52 capable"
        );
    }
    // TERM_PROGRAM-based detection (VS Code / iTerm2 / WezTerm / …).
    for program in [
        "vscode",
        "iTerm.app",
        "WezTerm",
        "ghostty",
        "mintty",
        "Hyper",
        "Tabby",
        "WarpTerminal",
    ] {
        assert!(
            osc52_supported_by_terminal(None, Some(program), false, false),
            "TERM_PROGRAM={program} should be detected as OSC 52 capable"
        );
    }
    // Windows Terminal (WT_SESSION) and kitty (KITTY_WINDOW_ID — kitty
    // refuses to set TERM_PROGRAM, see kovidgoyal/kitty#3317).
    assert!(osc52_supported_by_terminal(None, None, true, false));
    assert!(osc52_supported_by_terminal(None, None, false, true));
}

/// Unknown terminals must be reported as *not* supporting OSC 52 — the safe
/// answer that makes the UI show the copy-failure toast instead of claiming
/// success while the clipboard silently kept its old contents.
#[test]
fn osc52_supported_by_terminal_rejects_unknown_terminals() {
    // PuTTY / MobaXterm / FinalShell / Xshell all report plain `xterm*`;
    // VTE and stock xterm disable OSC 52 by default; screen/vt100/linux
    // never had it.
    for term in [
        "xterm",
        "xterm-256color",
        "screen",
        "screen-256color",
        "vt100",
        "linux",
        "putty",
        "putty-256color",
    ] {
        assert!(
            !osc52_supported_by_terminal(Some(term), None, false, false),
            "TERM={term} must not be assumed to support OSC 52"
        );
    }
    // Unknown / absent everything → not confirmed.
    assert!(!osc52_supported_by_terminal(None, None, false, false));
    assert!(!osc52_supported_by_terminal(Some(""), None, false, false));
    // Apple Terminal / GNOME Terminal report TERM_PROGRAM variants that
    // are not in the known-good list.
    assert!(!osc52_supported_by_terminal(
        Some("xterm-256color"),
        Some("Apple_Terminal"),
        false,
        false
    ));
}

/// Inside tmux the pane `TERM` is masked (`screen*`/`tmux*`), so a tmux
/// environment alone must NOT confirm OSC 52 delivery: with
/// `allow-passthrough off` (the tmux >= 3.3 default) the DCS passthrough is
/// silently dropped, and even when it reaches the outer terminal that
/// terminal may not implement OSC 52. Delivery only counts when a signal
/// that survives the tmux session environment snapshot identifies a known
/// terminal: `TERM_PROGRAM`, `WT_SESSION` or `KITTY_WINDOW_ID`.
#[test]
fn osc52_supported_by_terminal_inside_tmux_requires_outer_signal() {
    // A default tmux pane — TERM masked, no known-outer signal propagated —
    // is NOT confirmed (the conservative flip from the previous "any tmux
    // counts as supported" behaviour).
    for term in ["screen", "screen-256color", "tmux-256color"] {
        assert!(
            !osc52_supported_by_terminal(Some(term), None, false, false),
            "tmux pane TERM={term} alone must not confirm OSC 52 delivery"
        );
    }
    // TERM_PROGRAM snapshot propagated from the outer terminal.
    for program in ["vscode", "iTerm.app", "WezTerm"] {
        assert!(osc52_supported_by_terminal(
            Some("tmux-256color"),
            Some(program),
            false,
            false
        ));
    }
    // WT_SESSION (Windows Terminal) and KITTY_WINDOW_ID (kitty) snapshots.
    assert!(osc52_supported_by_terminal(
        Some("screen-256color"),
        None,
        true,
        false
    ));
    assert!(osc52_supported_by_terminal(
        Some("tmux-256color"),
        None,
        false,
        true
    ));
    // An unknown TERM_PROGRAM leaking through the snapshot changes nothing.
    assert!(!osc52_supported_by_terminal(
        Some("tmux-256color"),
        Some("Apple_Terminal"),
        false,
        false
    ));
}

/// A native clipboard write (wl-copy / xclip / xsel / arboard) is confirmed
/// delivery regardless of the terminal.
#[test]
fn clipboard_outcome_native_delivery_is_confirmed() {
    assert!(ClipboardOutcome::Native.delivered());
}

/// OSC 52 delivery is exactly the terminal heuristic: both sides read the
/// same env vars, so the equivalence holds whatever the test runner's TERM.
#[test]
fn clipboard_outcome_osc52_delivery_follows_terminal_heuristic() {
    assert_eq!(
        ClipboardOutcome::Osc52.delivered(),
        osc52_likely_supported()
    );
}

// ── 官方 model list 拉取 ───────────────────────────────────────────────────

fn summary(id: &str, display_name: Option<&str>) -> LlmModelSummary {
    LlmModelSummary {
        id: id.to_string(),
        display_name: display_name.map(str::to_string),
        provider: None,
        context_length: None,
        max_output_tokens: None,
    }
}

fn ids(models: &[ModelInfo]) -> Vec<&str> {
    models.iter().map(|model| model.id.as_str()).collect()
}

#[test]
fn model_fetch_request_switching_provider_uses_new_provider_defaults() {
    let (_temp_dir, mut state) = temp_state();
    state.agent_config.llm.provider = "openai".to_string();
    state.agent_config.llm.model = "gpt-4o".to_string();
    state.agent_config.llm.api_key_env = Some("OPENAI_API_KEY".to_string());
    state.agent_config.llm.api_base = "https://api.openai.com/v1".to_string();

    let request = model_fetch_request(&state.agent_config, "deepseek");

    assert_eq!(request.provider, "deepseek");
    assert_eq!(
        request.api_base, "https://api.deepseek.com/v1",
        "非激活 provider 必须用它的默认 base，而不是配置里那份 openai base"
    );
    assert_eq!(request.api_key_env.as_deref(), Some("DEEPSEEK_API_KEY"));
}

#[test]
fn model_fetch_request_same_provider_preserves_configured_api_base() {
    let (_temp_dir, mut state) = temp_state();
    state.agent_config.llm.provider = "openai".to_string();
    state.agent_config.llm.api_base = "https://proxy.example/v1".to_string();
    state.agent_config.llm.api_key_env = Some("XIAOO_TEST_MODEL_FETCH_KEY".to_string());

    let request = model_fetch_request(&state.agent_config, "openai");

    assert_eq!(
        request.api_base, "https://proxy.example/v1",
        "激活 provider 必须沿用配置里的自定义 base（私有网关场景）"
    );
    assert_eq!(
        request.api_key_env.as_deref(),
        Some("XIAOO_TEST_MODEL_FETCH_KEY")
    );
}

#[test]
fn model_fetch_request_reads_api_key_from_env() {
    const ENV: &str = "XIAOO_TEST_MODEL_FETCH_KEY";
    let (_temp_dir, mut state) = temp_state();
    state.agent_config.llm.provider = "openai".to_string();
    state.agent_config.llm.api_key_env = Some(ENV.to_string());

    // Edition 2021: std::env mutation is not yet `unsafe`。变量名是本测试专用的，
    // 不会与并行运行的其他测试相互干扰。
    std::env::set_var(ENV, "  sk-test-key  ");
    assert_eq!(
        model_fetch_request(&state.agent_config, "openai")
            .api_key
            .as_deref(),
        Some("sk-test-key"),
        "key 两侧空白必须被 trim（否则会带着空白发出去）"
    );

    std::env::set_var(ENV, "   ");
    assert!(
        model_fetch_request(&state.agent_config, "openai")
            .api_key
            .is_none(),
        "全空白的 key 必须视为缺失"
    );

    std::env::remove_var(ENV);
    assert!(model_fetch_request(&state.agent_config, "openai")
        .api_key
        .is_none());
}

#[test]
fn model_infos_from_summaries_prefers_display_name_and_dedupes_and_skips_empty_ids() {
    let infos = model_infos_from_summaries(vec![
        summary("m1", Some("Model One")),
        summary("m2", Some("   ")),
        summary("   ", Some("blank id")),
        summary("m1", Some("duplicate")),
        summary(" m3 ", None),
    ]);

    assert_eq!(
        ids(&infos),
        vec!["m1", "m2", "m3"],
        "顺序保持官方返回顺序，id 去重、空 id 跳过、id 两侧空白 trim"
    );
    assert_eq!(infos[0].name, "Model One", "display_name 优先");
    assert_eq!(infos[1].name, "m2", "display_name 全空白时回落到 id");
    assert_eq!(infos[2].name, "m3", "无 display_name 时回落到 id");
    let with_control = model_infos_from_summaries(vec![
        summary("m4", Some("shown\u{1b}[2Jname")),
    ]);
    assert_eq!(
        with_control[0].name, "shown[2Jname",
        "display_name 与 id 走同一套消毒：控制字符必须剥掉"
    );
}

/// OpenAI 兼容协议族：`GET {base}/models` + Bearer，解析 `data[].id`。
/// 打的是 mockito 起的本地假端点，不出网。
#[tokio::test]
async fn fetch_provider_models_parses_openai_style_model_list() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/v1/models")
        .match_header("authorization", "Bearer test-key")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"object":"list","data":[{"id":"m1"},{"id":"m2"}]}"#)
        .create_async()
        .await;

    let outcome = fetch_provider_models(ProviderModelFetchRequest {
        provider: "openai-compatible".to_string(),
        api_base: format!("{}/v1", server.url()),
        api_key_env: None,
        api_key: Some("test-key".to_string()),
    })
    .await;

    mock.assert_async().await;
    let models = outcome.result.expect("官方列表应解析成功");
    assert_eq!(outcome.provider, "openai-compatible");
    assert_eq!(
        ids(&models),
        vec!["m1", "m2"],
        "顺序必须与官方返回一致（不排序）"
    );
}

/// Anthropic 协议族走的是专属适配器（x-api-key + anthropic-version），
/// 且 display_name / 上下文长度来自响应体——这条用例同时覆盖协议分派与
/// 显示名映射，避免"所有 provider 都当 OpenAI 兼容"的想当然。
#[tokio::test]
async fn fetch_provider_models_uses_anthropic_protocol_shape() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/v1/models")
        .match_header("x-api-key", "sk-ant-test")
        .match_header("anthropic-version", "2023-06-01")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"data":[{"id":"claude-x","display_name":"Claude X","max_input_tokens":200000}]}"#,
        )
        .create_async()
        .await;

    let outcome = fetch_provider_models(ProviderModelFetchRequest {
        provider: "anthropic".to_string(),
        api_base: format!("{}/v1", server.url()),
        api_key_env: None,
        api_key: Some("sk-ant-test".to_string()),
    })
    .await;

    mock.assert_async().await;
    let models = outcome.result.expect("anthropic 列表应可解析");
    assert_eq!(ids(&models), vec!["claude-x"]);
    assert_eq!(models[0].name, "Claude X");
}

#[tokio::test]
async fn fetch_provider_models_reports_http_error() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/v1/models")
        .with_status(401)
        .with_body("unauthorized")
        .create_async()
        .await;

    let outcome = fetch_provider_models(ProviderModelFetchRequest {
        provider: "openai-compatible".to_string(),
        api_base: format!("{}/v1", server.url()),
        api_key_env: None,
        api_key: Some("bad-key".to_string()),
    })
    .await;

    mock.assert_async().await;
    match outcome.result {
        Err(ModelFetchError::Failed(message)) => {
            assert!(
                message.contains("401"),
                "错误文案应带状态码，便于用户排查：{message}"
            );
        }
        other => panic!("HTTP 401 应映射为 Failed，实际 {other:?}"),
    }
}

#[tokio::test]
async fn fetch_provider_models_reports_empty_list_as_ok_empty() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/v1/models")
        .with_status(200)
        .with_body(r#"{"data":[]}"#)
        .create_async()
        .await;

    let outcome = fetch_provider_models(ProviderModelFetchRequest {
        provider: "openai-compatible".to_string(),
        api_base: format!("{}/v1", server.url()),
        api_key_env: None,
        api_key: Some("test-key".to_string()),
    })
    .await;

    mock.assert_async().await;
    // 空列表不是传输错误：由调用方（App）决定"不清空列表 + 记失败"。
    assert!(outcome
        .result
        .expect("空列表本身解析成功")
        .is_empty());
}

/// 缺 key 必须在**发请求之前**失败：`expect(0)` + `assert_async()` 断言这个
/// 假端点一次都没被访问过。
#[tokio::test]
async fn fetch_provider_models_reports_missing_api_key_env_without_http() {
    const ENV: &str = "XIAOO_TEST_UNSET_MODEL_KEY";
    std::env::remove_var(ENV);
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/v1/models")
        .with_status(200)
        .with_body(r#"{"data":[]}"#)
        .expect(0)
        .create_async()
        .await;

    let outcome = fetch_provider_models(ProviderModelFetchRequest {
        provider: "openai-compatible".to_string(),
        api_base: format!("{}/v1", server.url()),
        api_key_env: Some(ENV.to_string()),
        api_key: None,
    })
    .await;

    mock.assert_async().await;
    match outcome.result {
        Err(ModelFetchError::MissingApiKey(name)) => assert_eq!(name, ENV),
        other => panic!("缺 key 应映射为 MissingApiKey，实际 {other:?}"),
    }
}

/// 无官方 catalog 的 provider 必须优先报 `Unsupported`，而不是"缺 key"：
/// minimax / gitcode 即使配好 key 也拉不到官方列表，提示"去配 key"是误导
/// （门面会先解析 key 再判 catalog，所以在 TUI 侧前置这道判据）。
/// `expect(0)` 同时证明这条路径一个请求都不发。
#[tokio::test]
async fn fetch_provider_models_reports_unsupported_before_missing_api_key() {
    const ENV: &str = "XIAOO_TEST_UNSET_MINIMAX_KEY";
    std::env::remove_var(ENV);
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/v1/models")
        .with_status(200)
        .with_body(r#"{"data":[]}"#)
        .expect(0)
        .create_async()
        .await;

    let outcome = fetch_provider_models(ProviderModelFetchRequest {
        provider: "minimax".to_string(),
        api_base: format!("{}/v1", server.url()),
        api_key_env: Some(ENV.to_string()),
        api_key: None,
    })
    .await;

    mock.assert_async().await;
    assert!(
        matches!(outcome.result, Err(ModelFetchError::Unsupported)),
        "无 catalog 的 provider 应报 Unsupported（而不是缺 key），实际 {:?}",
        outcome.result
    );
}

#[tokio::test]
async fn fetched_model_labels_cannot_inject_terminal_control_sequences() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/v1/models")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"data":[{"id":"evil\u001b[2Jmodel","display_name":"pwn\u001b]0;title\u0007"}]}"#)
        .create_async()
        .await;

    let outcome = fetch_provider_models(ProviderModelFetchRequest {
        provider: "openai-compatible".to_string(),
        api_base: format!("{}/v1", server.url()),
        api_key_env: None,
        api_key: Some("test-key".to_string()),
    })
    .await;

    mock.assert_async().await;
    let models = outcome.result.expect("官方列表应解析成功");
    assert_eq!(models.len(), 1);
    assert_eq!(
        models[0].id, "evil[2Jmodel",
        "ESC 必须被剥掉，其余字符保持原样"
    );
    assert_eq!(
        models[0].name, "evil[2Jmodel",
        "该协议不返回 display_name 时，显示名回落为消毒后的 id"
    );
    assert!(
        !models[0]
            .id
            .chars()
            .chain(models[0].name.chars())
            .any(char::is_control),
        "进入渲染层的模型文案不得含任何控制字符"
    );
}

#[test]
fn fetch_failure_messages_drop_control_characters_and_redact_urls() {
    let raw = "HTTP status client error (401 Unauthorized) for url \
               (http://host/v1/models?key=sk-secret)\u{1b}[2J";
    let message = match map_model_fetch_error(
        crate::gateway::llm_assembly::LlmAssemblyError::ModelCatalog(raw.to_string()),
    ) {
        ModelFetchError::Failed(message) => message,
        other => panic!("应映射为 Failed，实际 {other:?}"),
    };

    assert!(
        message.contains("http://host/<redacted>"),
        "URL 必须只保留 scheme://host（便于定位端点），其余抹掉：{message}"
    );
    assert!(
        !message.contains("sk-secret"),
        "错误文案不得把 URL query 里的密钥带出屏幕：{message}"
    );
    assert!(
        !message.contains('\u{1b}'),
        "错误文案不得含控制字符：{message}"
    );
}

#[test]
fn externally_sourced_labels_are_length_capped() {
    let long = "x".repeat(MAX_EXTERNAL_TEXT_CHARS + 50);
    assert_eq!(
        sanitize_external_label(&long).chars().count(),
        MAX_EXTERNAL_TEXT_CHARS,
        "异常超长标签必须截断，避免把界面撑爆"
    );
}

#[tokio::test]
async fn a_panicking_fetch_task_still_reports_a_failure() {
    // 任务 panic 若不回传，provider 会永久停在 Loading：needs_refresh(Loading) 为假，
    // 且 start_model_fetch 的守卫会拦住包括 Ctrl+R 在内的所有重试。
    let handle: tokio::task::JoinHandle<ProviderModelFetchOutcome> =
        tokio::spawn(async move { panic!("boom") });

    let outcome = join_fetch_outcome("openai-compatible".to_string(), handle).await;

    assert_eq!(outcome.provider, "openai-compatible");
    match outcome.result {
        Err(ModelFetchError::Failed(message)) => {
            assert!(message.contains("panicked"), "期望 panic 记成失败，实际 {message}");
        }
        other => panic!("panic 必须转成 Failed，实际 {other:?}"),
    }
}

#[test]
fn invisible_format_characters_are_stripped_from_model_labels() {
    // Cf（格式字符）能穿过 ratatui 基于 `is_control` 的字素过滤：bidi 覆盖（U+202E）
    // 可以让模型名以完全不同的顺序渲染，零宽字符能让两个不同名字看起来一样。
    let infos = model_infos_from_summaries(vec![
        summary("evil\u{202e}model", None),
        summary("zero\u{200b}width", Some("soft\u{00ad}hyphen")),
    ]);

    assert_eq!(infos[0].id, "evilmodel");
    assert_eq!(infos[1].id, "zerowidth");
    assert_eq!(infos[1].name, "softhyphen");
}

#[test]
fn fetch_failure_messages_redact_bare_key_fields_and_bearer_tokens() {
    // gemini 的 403 body 会原样拼进错误串，且 URL 可能被空白拆开——只按 `://` 判断会
    // 把 `key=...` 片段留在屏幕上。
    let message = match map_model_fetch_error(
        crate::gateway::llm_assembly::LlmAssemblyError::ModelCatalog(
            "403 Forbidden key=sk-live-SECRET\nAuthorization: Bearer sk-live-SECRET".to_string(),
        ),
    ) {
        ModelFetchError::Failed(message) => message,
        other => panic!("应映射为 Failed，实际 {other:?}"),
    };

    assert!(!message.contains("sk-live-SECRET"), "密钥不得上屏：{message}");
    assert!(message.contains("<redacted>"), "应留下抹除痕迹：{message}");
}

#[test]
fn huge_model_lists_are_capped() {
    let many = (0..(MAX_MODELS_PER_PROVIDER + 500))
        .map(|index| summary(&format!("m{index}"), None))
        .collect();
    assert_eq!(
        model_infos_from_summaries(many).len(),
        MAX_MODELS_PER_PROVIDER,
        "异常巨大的目录必须截断（每帧都要为列表建行）"
    );
}

#[test]
fn redacted_urls_keep_the_host_so_tls_failures_stay_diagnosable() {
    // 自签名网关场景：整条 URL 全抹掉会让"哪个端点握手失败"无从判断，所以只抹路径/query。
    let message = match map_model_fetch_error(
        crate::gateway::llm_assembly::LlmAssemblyError::ModelCatalog(
            "error sending request for url (https://gw.example.com/v1/models?key=sk-live-SECRET)".to_string(),
        ),
    ) {
        ModelFetchError::Failed(message) => message,
        other => panic!("应映射为 Failed，实际 {other:?}"),
    };

    assert!(message.contains("https://gw.example.com/<redacted>"), "{message}");
    assert!(!message.contains("sk-live-SECRET"), "密钥不得上屏/进日志：{message}");
    assert!(!message.contains("/v1/models"), "路径也应抹掉：{message}");
}
