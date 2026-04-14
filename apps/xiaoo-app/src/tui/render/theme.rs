use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy)]
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
}

impl Theme {
    pub fn dark() -> Self {
        Self {
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
        }
    }

    pub fn light() -> Self {
        Self {
            background: Color::Reset,
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
        }
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
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}
