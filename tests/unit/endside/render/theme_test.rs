use super::*;

#[test]
fn apple_terminal_defaults_to_ansi256_theme() {
    let env = TerminalThemeEnv {
        term_program: Some("apple_terminal".to_string()),
        ..TerminalThemeEnv::default()
    };

    let theme = Theme::from_terminal_env(&env);

    assert_eq!(theme.background, Color::Indexed(233));
    assert_eq!(theme.status_bar_bg, Color::Indexed(233));
}

#[test]
fn colorterm_truecolor_selects_rgb_theme() {
    let env = TerminalThemeEnv {
        colorterm: Some("truecolor".to_string()),
        ..TerminalThemeEnv::default()
    };

    let theme = Theme::from_terminal_env(&env);

    assert_eq!(theme.background, Color::Rgb(16, 16, 20));
    assert_eq!(theme.primary, Color::Rgb(80, 180, 255));
}

#[test]
fn truecolor_light_theme_uses_explicit_light_background() {
    let env = TerminalThemeEnv {
        colorterm: Some("truecolor".to_string()),
        colorfgbg: Some("0;15".to_string()),
        ..TerminalThemeEnv::default()
    };

    let theme = Theme::from_terminal_env(&env);

    assert_eq!(theme.background, Color::Rgb(248, 248, 250));
    assert_eq!(theme.foreground, Color::Rgb(30, 30, 30));
}

#[test]
fn colorfgbg_light_background_selects_light_palette() {
    let env = TerminalThemeEnv {
        term: Some("xterm-256color".to_string()),
        colorfgbg: Some("0;15".to_string()),
        ..TerminalThemeEnv::default()
    };

    let theme = Theme::from_terminal_env(&env);

    assert_eq!(theme.background, Color::Indexed(255));
    assert_eq!(theme.foreground, Color::Indexed(235));
}

#[test]
fn no_color_light_background_selects_light_basic_theme() {
    let env = TerminalThemeEnv {
        colorfgbg: Some("0;15".to_string()),
        no_color: true,
        ..TerminalThemeEnv::default()
    };

    let theme = Theme::from_terminal_env(&env);

    assert_eq!(theme.background, Color::White);
    assert_eq!(theme.foreground, Color::Black);
}

#[test]
fn toggled_theme_keeps_terminal_color_capability() {
    let env = TerminalThemeEnv {
        term_program: Some("apple_terminal".to_string()),
        ..TerminalThemeEnv::default()
    };

    let theme = Theme::from_terminal_env(&env).toggled();

    assert!(theme.is_light());
    assert_eq!(theme.background, Color::Indexed(255));
    assert_eq!(theme.primary, Color::Indexed(25));
}
