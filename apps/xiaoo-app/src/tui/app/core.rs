use anyhow::Result;
use crossterm::cursor::SetCursorStyle;
use crossterm::event::EventStream;
use crossterm::execute;
use futures_util::{FutureExt, StreamExt};
use ratatui::Terminal;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::time::interval;

use crate::app_state::AppState;
use crate::config::Config;
use crate::gateway_runtime::GatewayRuntime;

pub struct App {
    pub(crate) state: AppState,
    pub(crate) gateway: GatewayRuntime,
}

const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(500);

impl App {
    pub fn new(config_path: PathBuf, workspace: PathBuf) -> Result<Self, anyhow::Error> {
        Ok(Self {
            state: AppState::new(config_path, workspace)?,
            gateway: GatewayRuntime::new(),
        })
    }

    pub fn new_with_config(
        config: &Config,
        config_path: PathBuf,
        workspace: PathBuf,
    ) -> Result<Self, anyhow::Error> {
        Ok(Self {
            state: AppState::new_with_config(config, config_path, workspace)?,
            gateway: GatewayRuntime::new(),
        })
    }

    pub async fn run(
        &mut self,
        terminal: &mut Terminal<impl ratatui::backend::Backend>,
    ) -> Result<()> {
        let mut render_interval = interval(Duration::from_millis(16));
        let mut event_stream = EventStream::new();
        let _ = execute!(io::stdout(), SetCursorStyle::BlinkingBar);
        set_cursor_color(self.state.theme.border_active);
        let mut cursor_visible = true;
        let mut last_cursor_blink_toggle = Instant::now();

        loop {
            if self.state.chat_state.is_loading {
                self.state.loading_tick = (self.state.loading_tick + 1) % 12;
            }
            if last_cursor_blink_toggle.elapsed() >= CURSOR_BLINK_INTERVAL {
                cursor_visible = !cursor_visible;
                last_cursor_blink_toggle = Instant::now();
            }
            terminal.draw(|frame| self.ui(frame))?;
            if cursor_visible {
                terminal.show_cursor()?;
            } else {
                terminal.hide_cursor()?;
            }

            tokio::select! {
                _ = render_interval.tick() => {}
                maybe_event = event_stream.next().fuse() => {
                    if let Some(Ok(event)) = maybe_event {
                        self.handle_event(event).await?;
                    }
                }
            }

            self.gateway.poll_stream_updates(&mut self.state);

            if self.state.should_quit {
                break;
            }
        }
        self.gateway.close_sessions().await;
        reset_cursor_color();
        terminal.show_cursor()?;
        Ok(())
    }
}

fn set_cursor_color(color: ratatui::style::Color) {
    let Some(value) = color_to_ansi(color) else {
        return;
    };
    let _ = io::stdout().write_all(format!("\x1b]12;{value}\x07").as_bytes());
    let _ = io::stdout().flush();
}

fn reset_cursor_color() {
    let _ = io::stdout().write_all(b"\x1b]112\x07");
    let _ = io::stdout().flush();
}

fn color_to_ansi(color: ratatui::style::Color) -> Option<String> {
    match color {
        ratatui::style::Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
        ratatui::style::Color::Black => Some("black".to_string()),
        ratatui::style::Color::Red => Some("red".to_string()),
        ratatui::style::Color::Green => Some("green".to_string()),
        ratatui::style::Color::Yellow => Some("yellow".to_string()),
        ratatui::style::Color::Blue => Some("blue".to_string()),
        ratatui::style::Color::Magenta => Some("magenta".to_string()),
        ratatui::style::Color::Cyan => Some("cyan".to_string()),
        ratatui::style::Color::Gray => Some("gray".to_string()),
        ratatui::style::Color::DarkGray => Some("darkgray".to_string()),
        ratatui::style::Color::LightRed => Some("lightred".to_string()),
        ratatui::style::Color::LightGreen => Some("lightgreen".to_string()),
        ratatui::style::Color::LightYellow => Some("lightyellow".to_string()),
        ratatui::style::Color::LightBlue => Some("lightblue".to_string()),
        ratatui::style::Color::LightMagenta => Some("lightmagenta".to_string()),
        ratatui::style::Color::LightCyan => Some("lightcyan".to_string()),
        ratatui::style::Color::White => Some("white".to_string()),
        ratatui::style::Color::Indexed(index) => Some(index.to_string()),
        ratatui::style::Color::Reset => None,
    }
}
