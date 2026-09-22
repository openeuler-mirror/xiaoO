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
    assert_eq!(ClipboardOutcome::Osc52.delivered(), osc52_likely_supported());
}
