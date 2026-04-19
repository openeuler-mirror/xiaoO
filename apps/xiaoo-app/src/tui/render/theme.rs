use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub background: Color,
    pub foreground: Color,
    pub muted: Color,
    pub accent: Color,
    pub primary: Color,
    pub secondary: Color,
    pub error: Color,
    pub success: Color,
    pub border: Color,
    pub border_active: Color,
    pub user_message_bg: Color,
    pub assistant_message_bg: Color,
    pub input_bg: Color,
    pub selection: Color,
    pub code_bg: Color,
    pub code_fg: Color,
    pub status_bar_bg: Color,
    pub status_bar_fg: Color,
    pub tab_active: Color,
    pub tab_inactive: Color,
    pub gradient_green: Color,
    pub gradient_yellow: Color,
    pub gradient_red: Color,
    color_support: ColorSupport,
    background_mode: BackgroundMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColorSupport {
    TrueColor,
    Ansi256,
    Basic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackgroundMode {
    Dark,
    Light,
}

#[derive(Debug, Default)]
struct TerminalThemeEnv {
    term_program: Option<String>,
    colorterm: Option<String>,
    term: Option<String>,
    colorfgbg: Option<String>,
    no_color: bool,
}

impl Theme {
    pub fn detect() -> Self {
        let env = TerminalThemeEnv::capture();
        Self::from_terminal_env(&env)
    }

    pub fn dark() -> Self {
        Self::from_scheme(ColorSupport::TrueColor, BackgroundMode::Dark)
    }

    pub fn light() -> Self {
        Self::from_scheme(ColorSupport::TrueColor, BackgroundMode::Light)
    }

    fn from_terminal_env(env: &TerminalThemeEnv) -> Self {
        Self::from_scheme(env.color_support(), env.background_mode())
    }

    pub fn default_style(self) -> Style {
        Style::default().fg(self.foreground).bg(self.background)
    }

    pub fn muted_style(self) -> Style {
        Style::default().fg(self.muted)
    }

    pub fn accent_style(self) -> Style {
        Style::default().fg(self.accent)
    }

    pub fn primary_style(self) -> Style {
        Style::default().fg(self.primary)
    }

    pub fn error_style(self) -> Style {
        Style::default().fg(self.error)
    }

    pub fn success_style(self) -> Style {
        Style::default().fg(self.success)
    }

    pub fn is_light(self) -> bool {
        self.background_mode == BackgroundMode::Light
    }

    pub fn toggle_button_label(self) -> &'static str {
        if self.is_light() {
            "Theme: Light"
        } else {
            "Theme: Dark"
        }
    }

    pub fn toggled(self) -> Self {
        Self::from_scheme(self.color_support, self.background_mode.toggled())
    }

    pub fn bold_style(self) -> Style {
        Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(self.foreground)
    }

    pub fn border_style(self, active: bool) -> Style {
        if active {
            Style::default().fg(self.border_active)
        } else {
            Style::default().fg(self.border)
        }
    }

    pub fn code_style(self) -> Style {
        Style::default().fg(self.code_fg).bg(self.code_bg)
    }

    pub fn status_bar_style(self) -> Style {
        Style::default()
            .fg(self.status_bar_fg)
            .bg(self.status_bar_bg)
    }

    pub fn tab_style(self, active: bool) -> Style {
        if active {
            Style::default()
                .fg(self.tab_active)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(self.tab_inactive)
        }
    }

    fn from_scheme(color_support: ColorSupport, background_mode: BackgroundMode) -> Self {
        match (background_mode, color_support) {
            (BackgroundMode::Dark, ColorSupport::TrueColor) => Self {
                background: Color::Rgb(16, 16, 20),
                foreground: Color::Rgb(200, 200, 210),
                muted: Color::Rgb(90, 90, 110),
                accent: Color::Rgb(255, 165, 50),
                primary: Color::Rgb(80, 180, 255),
                secondary: Color::Rgb(100, 200, 180),
                error: Color::Rgb(255, 80, 80),
                success: Color::Rgb(80, 220, 100),
                border: Color::Rgb(60, 60, 75),
                border_active: Color::Rgb(80, 180, 255),
                user_message_bg: Color::Rgb(25, 30, 45),
                assistant_message_bg: Color::Rgb(22, 24, 32),
                input_bg: Color::Rgb(30, 32, 42),
                selection: Color::Rgb(50, 90, 160),
                code_bg: Color::Rgb(35, 38, 52),
                code_fg: Color::Rgb(180, 200, 150),
                status_bar_bg: Color::Rgb(30, 32, 42),
                status_bar_fg: Color::Rgb(140, 140, 160),
                tab_active: Color::Rgb(80, 180, 255),
                tab_inactive: Color::Rgb(90, 90, 110),
                gradient_green: Color::Rgb(80, 220, 100),
                gradient_yellow: Color::Rgb(255, 200, 50),
                gradient_red: Color::Rgb(255, 80, 80),
                color_support,
                background_mode,
            },
            (BackgroundMode::Light, ColorSupport::TrueColor) => Self {
                background: Color::Rgb(248, 248, 250),
                foreground: Color::Rgb(30, 30, 30),
                muted: Color::Rgb(70, 70, 70),
                accent: Color::Rgb(200, 100, 30),
                primary: Color::Rgb(30, 100, 180),
                secondary: Color::Rgb(50, 130, 170),
                error: Color::Rgb(200, 50, 50),
                success: Color::Rgb(40, 140, 60),
                border: Color::Rgb(150, 150, 150),
                border_active: Color::Rgb(50, 130, 170),
                user_message_bg: Color::Rgb(235, 235, 240),
                assistant_message_bg: Color::Rgb(245, 245, 250),
                input_bg: Color::Rgb(225, 225, 230),
                selection: Color::Rgb(100, 150, 200),
                code_bg: Color::Rgb(240, 240, 245),
                code_fg: Color::Rgb(60, 80, 50),
                status_bar_bg: Color::Rgb(230, 230, 235),
                status_bar_fg: Color::Rgb(100, 100, 120),
                tab_active: Color::Rgb(30, 100, 180),
                tab_inactive: Color::Rgb(120, 120, 140),
                gradient_green: Color::Rgb(40, 140, 60),
                gradient_yellow: Color::Rgb(200, 160, 30),
                gradient_red: Color::Rgb(200, 50, 50),
                color_support,
                background_mode,
            },
            (BackgroundMode::Dark, ColorSupport::Ansi256) => Self {
                background: Color::Indexed(233),
                foreground: Color::Indexed(252),
                muted: Color::Indexed(244),
                accent: Color::Indexed(214),
                primary: Color::Indexed(81),
                secondary: Color::Indexed(115),
                error: Color::Indexed(203),
                success: Color::Indexed(78),
                border: Color::Indexed(240),
                border_active: Color::Indexed(81),
                user_message_bg: Color::Indexed(236),
                assistant_message_bg: Color::Indexed(235),
                input_bg: Color::Indexed(236),
                selection: Color::Indexed(24),
                code_bg: Color::Indexed(236),
                code_fg: Color::Indexed(151),
                status_bar_bg: Color::Indexed(236),
                status_bar_fg: Color::Indexed(246),
                tab_active: Color::Indexed(81),
                tab_inactive: Color::Indexed(244),
                gradient_green: Color::Indexed(78),
                gradient_yellow: Color::Indexed(214),
                gradient_red: Color::Indexed(203),
                color_support,
                background_mode,
            },
            (BackgroundMode::Light, ColorSupport::Ansi256) => Self {
                background: Color::Indexed(255),
                foreground: Color::Indexed(235),
                muted: Color::Indexed(242),
                accent: Color::Indexed(166),
                primary: Color::Indexed(25),
                secondary: Color::Indexed(31),
                error: Color::Indexed(160),
                success: Color::Indexed(28),
                border: Color::Indexed(248),
                border_active: Color::Indexed(31),
                user_message_bg: Color::Indexed(254),
                assistant_message_bg: Color::Indexed(255),
                input_bg: Color::Indexed(253),
                selection: Color::Indexed(153),
                code_bg: Color::Indexed(254),
                code_fg: Color::Indexed(58),
                status_bar_bg: Color::Indexed(253),
                status_bar_fg: Color::Indexed(242),
                tab_active: Color::Indexed(25),
                tab_inactive: Color::Indexed(242),
                gradient_green: Color::Indexed(28),
                gradient_yellow: Color::Indexed(166),
                gradient_red: Color::Indexed(160),
                color_support,
                background_mode,
            },
            (BackgroundMode::Dark, ColorSupport::Basic) => Self {
                background: Color::Black,
                foreground: Color::White,
                muted: Color::DarkGray,
                accent: Color::Yellow,
                primary: Color::Cyan,
                secondary: Color::Green,
                error: Color::LightRed,
                success: Color::LightGreen,
                border: Color::DarkGray,
                border_active: Color::Cyan,
                user_message_bg: Color::Black,
                assistant_message_bg: Color::Black,
                input_bg: Color::DarkGray,
                selection: Color::Blue,
                code_bg: Color::DarkGray,
                code_fg: Color::White,
                status_bar_bg: Color::DarkGray,
                status_bar_fg: Color::Gray,
                tab_active: Color::Cyan,
                tab_inactive: Color::Gray,
                gradient_green: Color::LightGreen,
                gradient_yellow: Color::Yellow,
                gradient_red: Color::LightRed,
                color_support,
                background_mode,
            },
            (BackgroundMode::Light, ColorSupport::Basic) => Self {
                background: Color::White,
                foreground: Color::Black,
                muted: Color::DarkGray,
                accent: Color::Magenta,
                primary: Color::Blue,
                secondary: Color::Cyan,
                error: Color::Red,
                success: Color::Green,
                border: Color::Gray,
                border_active: Color::Blue,
                user_message_bg: Color::White,
                assistant_message_bg: Color::White,
                input_bg: Color::Gray,
                selection: Color::LightBlue,
                code_bg: Color::Gray,
                code_fg: Color::Black,
                status_bar_bg: Color::Gray,
                status_bar_fg: Color::DarkGray,
                tab_active: Color::Blue,
                tab_inactive: Color::DarkGray,
                gradient_green: Color::Green,
                gradient_yellow: Color::Yellow,
                gradient_red: Color::Red,
                color_support,
                background_mode,
            },
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::detect()
    }
}

impl TerminalThemeEnv {
    fn capture() -> Self {
        Self {
            term_program: read_env("TERM_PROGRAM"),
            colorterm: read_env("COLORTERM"),
            term: read_env("TERM"),
            colorfgbg: read_env("COLORFGBG"),
            no_color: std::env::var_os("NO_COLOR").is_some(),
        }
    }

    fn color_support(&self) -> ColorSupport {
        if self.no_color {
            return ColorSupport::Basic;
        }

        if matches!(self.term_program.as_deref(), Some("apple_terminal")) {
            return ColorSupport::Ansi256;
        }

        if self
            .colorterm
            .as_deref()
            .is_some_and(|value| value.contains("truecolor") || value.contains("24bit"))
        {
            return ColorSupport::TrueColor;
        }

        if self.term.as_deref().is_some_and(|value| {
            value.contains("direct") || value.contains("truecolor") || value.contains("24bit")
        }) {
            return ColorSupport::TrueColor;
        }

        if self
            .term
            .as_deref()
            .is_some_and(|value| value.contains("256color"))
        {
            return ColorSupport::Ansi256;
        }

        ColorSupport::Basic
    }

    fn background_mode(&self) -> BackgroundMode {
        self.colorfgbg
            .as_deref()
            .and_then(parse_background_mode_from_colorfgbg)
            .unwrap_or(BackgroundMode::Dark)
    }
}

impl BackgroundMode {
    fn toggled(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::Dark,
        }
    }
}

fn read_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

fn parse_background_mode_from_colorfgbg(value: &str) -> Option<BackgroundMode> {
    let background = value.rsplit(';').next()?.trim().parse::<u16>().ok()?;

    match background {
        0..=6 | 8 | 232..=243 => Some(BackgroundMode::Dark),
        7 | 9..=15 | 244..=255 => Some(BackgroundMode::Light),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apple_terminal_defaults_to_ansi256_theme() {
        let env = TerminalThemeEnv {
            term_program: Some("apple_terminal".to_string()),
            ..TerminalThemeEnv::default()
        };

        let theme = Theme::from_terminal_env(&env);

        assert_eq!(theme.background, Color::Indexed(233));
        assert_eq!(theme.status_bar_bg, Color::Indexed(236));
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
}
