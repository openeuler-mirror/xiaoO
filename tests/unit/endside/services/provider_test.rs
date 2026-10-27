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
    let agent_config = std::sync::Arc::make_mut(&mut state.agent_config);
    agent_config.llm.provider = "openai".to_string();
    agent_config.llm.model = "gpt-4o".to_string();
    agent_config.llm.api_key_env = Some("OPENAI_API_KEY".to_string());
    agent_config.llm.api_base = "https://api.openai.com/v1".to_string();

    let (api_key_env, api_base) = persisted_selection_settings(&state.agent_config, "deepseek");

    assert_eq!(api_key_env.as_deref(), Some("DEEPSEEK_API_KEY"));
    assert_eq!(api_base, "https://api.deepseek.com/v1");
}

#[test]
fn persisted_selection_settings_same_provider_preserves_existing_config() {
    let (_temp_dir, mut state) = temp_state();
    let agent_config = std::sync::Arc::make_mut(&mut state.agent_config);
    agent_config.llm.provider = "openai".to_string();
    agent_config.llm.model = "gpt-4o".to_string();
    agent_config.llm.api_key_env = Some("CUSTOM_OPENAI_KEY".to_string());
    agent_config.llm.api_base = "https://proxy.example/v1".to_string();

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
    // is NOT confirmed: any tmux without a known outer signal counts as
    // unsupported (conservative default).
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

// ── Official model-list fetching ────────────────────────────────────────────

fn model(id: &str) -> ModelInfo {
    ModelInfo {
        id: id.to_string(),
        name: id.to_string(),
    }
}

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
    let agent_config = std::sync::Arc::make_mut(&mut state.agent_config);
    agent_config.llm.provider = "openai".to_string();
    agent_config.llm.model = "gpt-4o".to_string();
    agent_config.llm.api_key_env = Some("OPENAI_API_KEY".to_string());
    agent_config.llm.api_base = "https://api.openai.com/v1".to_string();

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
    let agent_config = std::sync::Arc::make_mut(&mut state.agent_config);
    agent_config.llm.provider = "openai".to_string();
    agent_config.llm.api_base = "https://proxy.example/v1".to_string();
    agent_config.llm.api_key_env = Some("XIAOO_TEST_MODEL_FETCH_KEY".to_string());

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
    let agent_config = std::sync::Arc::make_mut(&mut state.agent_config);
    agent_config.llm.provider = "openai".to_string();
    agent_config.llm.api_key_env = Some(ENV.to_string());

    // Edition 2021: std::env mutation is not yet `unsafe`. The variable
    // name is test-local and cannot collide with other tests running in
    // parallel.
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
    let with_control = model_infos_from_summaries(vec![summary("m4", Some("shown\u{1b}[2Jname"))]);
    assert_eq!(
        with_control[0].name, "shown[2Jname",
        "display_name 与 id 走同一套消毒：控制字符必须剥掉"
    );
}

#[test]
fn pin_configured_model_inserts_missing_active_model_at_front() {
    let pinned = pin_configured_model(vec![model("a"), model("b")], Some("gateway-model"));

    assert_eq!(ids(&pinned), vec!["gateway-model", "a", "b"]);
    assert_eq!(pinned[0].name, "gateway-model (configured)");
}

#[test]
fn pin_configured_model_keeps_list_untouched_when_present_or_blank() {
    assert_eq!(
        ids(&pin_configured_model(
            vec![model("a"), model("b")],
            Some("B")
        )),
        vec!["a", "b"],
        "已在列表里（忽略大小写）时不插入"
    );
    assert_eq!(
        ids(&pin_configured_model(vec![model("a")], Some("   "))),
        vec!["a"],
        "全空白的配置模型视为未配置"
    );
    assert_eq!(
        ids(&pin_configured_model(vec![model("a")], None)),
        vec!["a"]
    );
}

/// OpenAI-compatible protocol family: `GET {base}/models` + Bearer,
/// parsing `data[].id`. Hits a local mockito endpoint; no network.
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

/// The Anthropic family goes through a dedicated adapter (x-api-key +
/// anthropic-version), and display_name / context length come from the
/// response body — this case covers protocol dispatch and display-name
/// mapping together, guarding against "treat every provider as
/// OpenAI-compatible" assumptions.
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
    // An empty list is not a transport error: "keep the list + record
    // failure" is the caller's (App's) decision.
    assert!(outcome.result.expect("空列表本身解析成功").is_empty());
}

/// A missing key must fail **before any request is sent**: `expect(0)` +
/// `assert_async()` pin that the fake endpoint is never touched.
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

/// A provider without an official catalog must report `Unsupported`
/// before "missing key": minimax / gitcode cannot fetch an official list
/// even with a key configured, so a "go configure the key" hint is
/// misleading (the facade resolves the key before checking catalog
/// support, hence the TUI-side pre-check). `expect(0)` also proves this
/// path sends no request at all.
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
        .with_body(
            r#"{"data":[{"id":"evil\u001b[2Jmodel","display_name":"pwn\u001b]0;title\u0007"}]}"#,
        )
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
    // If a panicking task never reports back, the provider sits in Loading
    // until the stale backstop window (MODEL_LIST_STALE_AFTER) expires:
    // inside it needs_refresh(Loading) is false and the in-flight slot stays
    // taken, so nothing can fetch. This test pins "a panic must also fold
    // into Failed" — the user must not wait out the backstop window.
    let handle: tokio::task::JoinHandle<ProviderModelFetchOutcome> =
        tokio::spawn(async move { panic!("boom") });

    let outcome = join_fetch_outcome("openai-compatible".to_string(), handle).await;

    assert_eq!(outcome.provider, "openai-compatible");
    match outcome.result {
        Err(ModelFetchError::Failed(message)) => {
            assert!(
                message.contains("panicked"),
                "期望 panic 记成失败，实际 {message}"
            );
        }
        other => panic!("panic 必须转成 Failed，实际 {other:?}"),
    }
}

#[test]
fn invisible_format_characters_are_stripped_from_model_labels() {
    // Cf (format characters) slip through ratatui's `is_control`-based
    // grapheme filter: a bidi override (U+202E) can render a model name in a
    // completely different order, and zero-width characters can make two
    // distinct names look identical.
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
    // Gemini's 403 body gets spliced verbatim into the error string, and
    // whitespace may split the URL — matching on `://` alone would leave
    // `key=...` fragments on screen.
    let message = match map_model_fetch_error(
        crate::gateway::llm_assembly::LlmAssemblyError::ModelCatalog(
            "403 Forbidden key=sk-live-SECRET\nAuthorization: Bearer sk-live-SECRET".to_string(),
        ),
    ) {
        ModelFetchError::Failed(message) => message,
        other => panic!("应映射为 Failed，实际 {other:?}"),
    };

    assert!(
        !message.contains("sk-live-SECRET"),
        "密钥不得上屏：{message}"
    );
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
    // Self-signed-gateway case: redacting the whole URL would hide "which
    // endpoint failed the handshake", so only path/query go.
    let message = match map_model_fetch_error(
        crate::gateway::llm_assembly::LlmAssemblyError::ModelCatalog(
            "error sending request for url (https://gw.example.com/v1/models?key=sk-live-SECRET)"
                .to_string(),
        ),
    ) {
        ModelFetchError::Failed(message) => message,
        other => panic!("应映射为 Failed，实际 {other:?}"),
    };

    assert!(
        message.contains("https://gw.example.com/<redacted>"),
        "{message}"
    );
    assert!(
        !message.contains("sk-live-SECRET"),
        "密钥不得上屏/进日志：{message}"
    );
    assert!(!message.contains("/v1/models"), "路径也应抹掉：{message}");
}

#[test]
fn error_urls_without_a_path_do_not_leak_query_or_userinfo() {
    // Truncating at the first `/` alone would leave the whole authority
    // (query included) of a path-less URL like `https://host?key=…` on
    // screen; same for `user:pw@` userinfo.
    let message = sanitize_external_message(
        "request failed for https://gw.example.com?key=sk-live-NOPATH \
         and https://user:pw@host.example.com/v1/models?key=sk-live-PATH \
         and https://host.example.com#token=sk-live-FRAGMENT",
    );

    assert!(
        !message.contains("sk-live-NOPATH"),
        "无路径 URL 的 query 不得上屏：{message}"
    );
    assert!(
        !message.contains("sk-live-PATH"),
        "带路径 URL 的 query 不得上屏：{message}"
    );
    assert!(
        !message.contains("sk-live-FRAGMENT"),
        "fragment 不得上屏：{message}"
    );
    assert!(!message.contains("pw@"), "userinfo 不得上屏：{message}");
    assert!(
        message.contains("https://gw.example.com/<redacted>")
            && message.contains("https://host.example.com/<redacted>"),
        "主机名要保留（排查端点用）：{message}"
    );
}

/// The implementation encodes Cf / Zl / Zp as 22 ranges (independently
/// cross-checked codepoint by codepoint against `unicodedata` 15.1.0: 172
/// codepoints, 0 missed, 0 extra). This samples **per range**: the first,
/// second, middle, penultimate and last codepoint of every range must be
/// rejected — testing one representative character would not notice a range
/// shrunk by a single codepoint.
const INVISIBLE_RANGES: &[(u32, u32)] = &[
    (0x00AD, 0x00AD), // 软连字符
    (0x0600, 0x0605), // 阿拉伯数字标记等
    (0x061C, 0x061C), // 阿拉伯字母标记
    (0x06DD, 0x06DD),
    (0x070F, 0x070F), // 叙利亚缩写标记
    (0x0890, 0x0891),
    (0x08E2, 0x08E2),
    (0x180E, 0x180E),
    (0x200B, 0x200F), // 零宽 + bidi 标记
    (0x2028, 0x2029), // Zl / Zp
    (0x202A, 0x202E), // bidi 嵌入 / 覆盖
    (0x2060, 0x2064),
    (0x2066, 0x206F), // bidi 隔离 + 已弃用格式字符
    (0xFEFF, 0xFEFF),
    (0xFFF9, 0xFFFB),
    (0x110BD, 0x110BD),
    (0x110CD, 0x110CD),
    (0x13430, 0x1343F), // 埃及圣书体格式控制
    (0x1BCA0, 0x1BCA3), // 速记格式控制
    (0x1D173, 0x1D17A), // 乐谱格式控制
    (0xE0001, 0xE0001),
    (0xE0020, 0xE007F), // tag 字符
];

#[test]
fn invisible_format_character_ranges_are_rejected_at_their_ends_and_middle() {
    for &(start, end) in INVISIBLE_RANGES {
        let middle = (start + end) / 2;
        // Sample points must stay inside the range: for a one-codepoint
        // range, start+1 falls outside (U+00AD → U+00AE).
        let samples = [
            start,
            (start + 1).min(end),
            middle,
            end.saturating_sub(1).max(start),
            end,
        ];
        for codepoint in samples {
            let ch = char::from_u32(codepoint).expect("区间端点都是有效标量值");
            assert_eq!(
                sanitize_external_label(&format!("a{ch}b")),
                "ab",
                "U+{codepoint:04X} 属于 Cf / Zl / Zp，必须被剥掉"
            );
        }
    }

    // Legitimate characters must survive: CJK, emoji, and combining marks
    // (Mn, not part of Cf) all stay as-is.
    let normal = "模型-🚀-e\u{0301}";
    assert_eq!(sanitize_external_label(normal), normal);

    // "0 extra" needs pinning too: codepoints immediately outside each
    // range must not be caught (skipping control characters and adjacent
    // ranges' boundaries). The independent audit already verified the
    // implementation equals Cf∪Zl∪Zp exactly, codepoint by codepoint, via
    // `unicodedata`; this assertion keeps "no overreach" true in CI — note
    // it depends only on the range table above (the spec), never on the
    // implementation's classifier.
    let in_table = |codepoint: u32| {
        INVISIBLE_RANGES
            .iter()
            .any(|&(start, end)| (start..=end).contains(&codepoint))
    };
    for &(start, end) in INVISIBLE_RANGES {
        for codepoint in [start.checked_sub(1), end.checked_add(1)]
            .into_iter()
            .flatten()
        {
            let Some(ch) = char::from_u32(codepoint) else {
                continue;
            };
            if in_table(codepoint) || ch.is_control() {
                continue;
            }
            assert_eq!(
                sanitize_external_label(&format!("a{ch}b")),
                format!("a{ch}b"),
                "U+{codepoint:04X} 不属于 Cf / Zl / Zp，不该被剥掉"
            );
        }
    }
}
