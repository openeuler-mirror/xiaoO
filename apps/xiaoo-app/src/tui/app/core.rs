use anyhow::Result;
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{Event, EventStream, MouseEventKind};
use crossterm::execute;
use futures_util::{FutureExt, StreamExt};
use ratatui::Terminal;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::time::sleep;

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
        let mut event_stream = EventStream::new();
        let mut pending_event: Option<Event> = None;
        let _ = execute!(io::stdout(), SetCursorStyle::BlinkingBar);
        set_cursor_color(self.state.theme.border_active);
        let mut cursor_visible = true;
        let mut last_cursor_blink_toggle = Instant::now();
        let mut needs_redraw = true;

        loop {
            if needs_redraw {
                terminal.draw(|frame| self.ui(frame))?;
                needs_redraw = false;
            }
            if last_cursor_blink_toggle.elapsed() >= CURSOR_BLINK_INTERVAL {
                cursor_visible = !cursor_visible;
                last_cursor_blink_toggle = Instant::now();
                if cursor_visible {
                    terminal.show_cursor()?;
                } else {
                    terminal.hide_cursor()?;
                }
            }

            let active_refresh =
                self.state.chat_state.is_loading || self.gateway.needs_active_refresh();
            let tick_duration = if active_refresh {
                Duration::from_millis(16)
            } else {
                Duration::from_millis(250)
            };

            let mut handled_event = None;
            if let Some(event) = pending_event.take() {
                self.handle_event(event.clone()).await?;
                needs_redraw = true;
                handled_event = Some(event);
            } else {
                tokio::select! {
                    _ = sleep(tick_duration) => {
                        if self.state.chat_state.is_loading {
                            self.state.loading_tick = (self.state.loading_tick + 1) % 12;
                            needs_redraw = true;
                        }
                    }
                    maybe_event = event_stream.next().fuse() => {
                        if let Some(Ok(event)) = maybe_event {
                            self.handle_event(event.clone()).await?;
                            needs_redraw = true;
                            handled_event = Some(event);
                        }
                    }
                }
            }

            if let Some(event) = handled_event.as_ref() {
                discard_redundant_boundary_scrolls(
                    event,
                    &self.state,
                    &mut event_stream,
                    &mut pending_event,
                );
            }

            needs_redraw |= self.gateway.poll_stream_updates(&mut self.state);

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

fn discard_redundant_boundary_scrolls(
    handled_event: &Event,
    state: &AppState,
    event_stream: &mut EventStream,
    pending_event: &mut Option<Event>,
) {
    let boundary_kind = match handled_event {
        Event::Mouse(mouse)
            if mouse.kind == MouseEventKind::ScrollDown
                && state.chat_state.scroll_offset >= state.chat_state.max_scroll_offset() =>
        {
            Some(MouseEventKind::ScrollDown)
        }
        Event::Mouse(mouse)
            if mouse.kind == MouseEventKind::ScrollUp && state.chat_state.scroll_offset == 0 =>
        {
            Some(MouseEventKind::ScrollUp)
        }
        _ => None,
    };

    let Some(boundary_kind) = boundary_kind else {
        return;
    };
    let opposite_kind = match boundary_kind {
        MouseEventKind::ScrollDown => MouseEventKind::ScrollUp,
        MouseEventKind::ScrollUp => MouseEventKind::ScrollDown,
        _ => return,
    };

    for _ in 0..128 {
        let Some(ready) = event_stream.next().now_or_never() else {
            break;
        };
        let Some(Ok(event)) = ready else {
            break;
        };

        match &event {
            Event::Mouse(mouse) if mouse.kind == boundary_kind => {
                continue;
            }
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Moved => {
                continue;
            }
            Event::Mouse(mouse) if mouse.kind == opposite_kind => {
                *pending_event = Some(event);
                return;
            }
            _ => {
                *pending_event = Some(event);
                return;
            }
        }
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
