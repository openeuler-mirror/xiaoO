//! Interactive options + optional supplementary input (above TUI input area).
//!
//! ## Backend wiring (pre-built)
//! - **Inbound**: Any thread constructs [`PromptRequest`] and calls `App::open_interaction_prompt` (see `app.rs`).
//! - **Outbound**: Pass user selection back via `UnboundedSender<UserPromptResult>` passed during opening;
//!   upper layer can write to session, HTTP POST, or merge into next `ChatMessage`.
//! - Inbound: `SessionTurnUpdate::InteractionPrompt` opens this panel via `poll_stream_updates`.

use crate::input::Input;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, List, ListItem, Padding, Paragraph, Wrap},
    Frame,
};
use serde::{Deserialize, Serialize};

use super::theme::Theme;
use super::utils::sanitize_terminal_text;

/// Single selectable option (can align with JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptChoice {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
}

/// Backend → TUI: Request user to select from list, optionally allow supplementary input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptRequest {
    pub request_id: String,
    pub title: String,
    pub body: Option<String>,
    pub choices: Vec<PromptChoice>,
    #[serde(default)]
    pub allow_custom_input: bool,
    /// Multi-select: Space toggles selection in list, Enter submits `PromptResolution::Multi`.
    #[serde(default)]
    pub multi_select: bool,
    pub default_index: Option<usize>,
    /// Whether this is password input (hide display)
    #[serde(default)]
    pub is_secret: bool,
}

/// User operation result (TUI → backend / session).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPromptResult {
    pub request_id: String,
    pub resolution: PromptResolution,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PromptResolution {
    Single {
        choice_id: String,
        supplement: Option<String>,
    },
    /// Reserved, corresponds to `PromptRequest::multi_select`.
    Multi {
        choice_ids: Vec<String>,
    },
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptFocus {
    List,
    Supplement,
}

/// Runtime UI state (not involved in serialization).
pub struct InteractionPromptState {
    pub request: PromptRequest,
    pub selected: usize,
    /// Index in `choices` corresponding to first visible list row (for scrolling).
    pub list_scroll: usize,
    pub focus: PromptFocus,
    pub supplement: Input,
    /// When `request.multi_select` is true: per-choice selection.
    pub multi_checked: Vec<bool>,
}

impl InteractionPromptState {
    pub fn new(request: PromptRequest) -> Option<Self> {
        if request.choices.is_empty() {
            return None;
        }
        let n = request.choices.len();
        let selected = request.default_index.unwrap_or(0).min(n.saturating_sub(1));
        let mut multi_checked = vec![false; n];
        if request.multi_select {
            if let Some(di) = request.default_index {
                if di < n {
                    multi_checked[di] = true;
                }
            }
        }
        Some(Self {
            request,
            selected,
            list_scroll: 0,
            focus: PromptFocus::List,
            supplement: Input::default(),
            multi_checked,
        })
    }

    /// Toggle current row when `multi_select` (Space).
    pub fn toggle_multi_at_cursor(&mut self) {
        if !self.request.multi_select {
            return;
        }
        if let Some(c) = self.multi_checked.get_mut(self.selected) {
            *c = !*c;
        }
    }

    pub fn list_visible_max(&self) -> usize {
        if self.request.allow_custom_input {
            4
        } else {
            6
        }
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
        self.ensure_selected_visible();
    }

    pub fn move_down(&mut self) {
        let n = self.request.choices.len();
        if n > 0 && self.selected < n - 1 {
            self.selected += 1;
        }
        self.ensure_selected_visible();
    }

    pub fn page_up(&mut self) {
        let step = self.list_visible_max().max(1);
        self.selected = self.selected.saturating_sub(step);
        self.ensure_selected_visible();
    }

    pub fn page_down(&mut self) {
        let n = self.request.choices.len();
        let step = self.list_visible_max().max(1);
        if n > 0 {
            self.selected = (self.selected + step).min(n - 1);
        }
        self.ensure_selected_visible();
    }

    fn ensure_selected_visible(&mut self) {
        let vmax = self.list_visible_max();
        let n = self.request.choices.len();
        if n <= vmax {
            self.list_scroll = 0;
            return;
        }
        if self.selected < self.list_scroll {
            self.list_scroll = self.selected;
        } else if self.selected >= self.list_scroll + vmax {
            self.list_scroll = self.selected + 1 - vmax;
        }
    }

    pub fn toggle_focus(&mut self) {
        if !self.request.allow_custom_input {
            return;
        }
        self.focus = match self.focus {
            PromptFocus::List => PromptFocus::Supplement,
            PromptFocus::Supplement => PromptFocus::List,
        }
    }
}

/// Calculate prompt block height (including border), for use in `Constraint::Length`.
/// `inner_width` is the inner width of the dialog (excluding borders), used to wrap title.
pub fn interaction_prompt_outer_height(req: &PromptRequest, inner_width: u16) -> u16 {
    let border = 2u16;
    let title_h = if inner_width > 0 {
        wrap_text_to_lines(&req.title, inner_width as usize).len() as u16
    } else {
        1
    };
    let body_h = if req.body.as_ref().map_or(false, |s| !s.is_empty()) {
        1
    } else {
        0
    };
    let list_cap = if req.allow_custom_input { 4 } else { 6 };
    let list_h = req.choices.len().min(list_cap) as u16;
    let sup_h = if req.allow_custom_input { 3 } else { 0 };
    let total = border + title_h + body_h + list_h + sup_h;
    total.max(border + 1)
}

/// Wrap text into multiple lines based on character display width.
/// Hard line breaks (`\n`) are preserved: the text is first split into
/// segments, each wrapped to `max_width` columns, and empty segments kept as
/// blank lines so `\n\n` renders as a blank line.
/// Returns a Vec of strings, each fitting within max_width columns.
fn wrap_text_to_lines(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 || text.is_empty() {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    for segment in text.split('\n') {
        if segment.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut current_line = String::new();
        let mut current_width = 0usize;
        for ch in segment.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
            if current_width + cw > max_width {
                lines.push(current_line);
                current_line = String::new();
                current_width = 0;
            }
            current_line.push(ch);
            current_width += cw;
        }
        lines.push(current_line);
    }

    lines
}

pub fn render_interaction_prompt(
    f: &mut Frame,
    area: Rect,
    state: &InteractionPromptState,
    theme: &Theme,
    list_hit_area: &mut Option<Rect>,
    supplement_hit_area: &mut Option<Rect>,
) {
    *list_hit_area = None;
    *supplement_hit_area = None;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border_active))
        .style(Style::default().bg(theme.background));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Calculate title lines based on inner width
    let title_lines_vec = if inner.width > 0 {
        wrap_text_to_lines(&state.request.title, inner.width as usize)
    } else {
        vec![state.request.title.clone()]
    };
    let title_lines = title_lines_vec.len() as u16;

    let vmax = state.list_visible_max();
    let list_h = state.request.choices.len().min(vmax) as u16;

    let mut constraints: Vec<Constraint> = Vec::new();
    // Title area
    constraints.push(Constraint::Length(title_lines));
    if state.request.body.as_ref().map_or(false, |s| !s.is_empty()) {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(list_h));
    if state.request.allow_custom_input {
        constraints.push(Constraint::Length(3));
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    // Render title as first element
    let mut idx = 0usize;
    let title_text = Text::from(
        title_lines_vec
            .iter()
            .map(|line| {
                Line::styled(
                    line.clone(),
                    Style::default()
                        .fg(theme.foreground)
                        .add_modifier(Modifier::BOLD),
                )
            })
            .collect::<Vec<Line>>(),
    );
    let title = Paragraph::new(title_text);
    f.render_widget(title, chunks[idx]);
    idx += 1;

    if state.request.body.as_ref().map_or(false, |s| !s.is_empty()) {
        let body = state.request.body.as_deref().unwrap_or_default();
        let line = if body.chars().count() > 256 {
            let s: String = body.chars().take(253).chain("...".chars()).collect();
            s
        } else {
            body.to_string()
        };
        let p = Paragraph::new(line)
            .style(Style::default().fg(theme.muted))
            .wrap(Wrap { trim: true });
        f.render_widget(p, chunks[idx]);
        idx += 1;
    }

    let list_chunk = chunks[idx];
    let start = state
        .list_scroll
        .min(state.request.choices.len().saturating_sub(1));
    let end = (start + vmax).min(state.request.choices.len());

    let items: Vec<ListItem> = state.request.choices[start..end]
        .iter()
        .enumerate()
        .map(|(i, ch)| {
            let global_i = start + i;
            let is_sel = global_i == state.selected && state.focus == PromptFocus::List;
            let style = if is_sel {
                Style::default()
                    .fg(theme.foreground)
                    .bg(theme.selection)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.foreground)
            };
            let mark = if state.request.multi_select {
                let on = state.multi_checked.get(global_i).copied().unwrap_or(false);
                if on {
                    sanitize_terminal_text("[✓] ")
                } else {
                    "[ ] ".to_string()
                }
            } else {
                String::new()
            };
            let mut spans = vec![Span::styled(format!("{}{} ", mark, ch.label), style)];
            if let Some(d) = &ch.description {
                spans.push(Span::styled(
                    sanitize_terminal_text(&format!(" — {}", d)),
                    Style::default().fg(theme.muted),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::NONE)
            .style(Style::default().bg(theme.background)),
    );
    f.render_widget(list, list_chunk);
    *list_hit_area = Some(list_chunk);

    if state.request.allow_custom_input {
        let sup_area = chunks[idx + 1];
        let sup_focus = state.focus == PromptFocus::Supplement;
        let sup_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(if sup_focus {
                theme.border_active
            } else {
                theme.border
            }))
            .title(if state.request.is_secret {
                " 密码输入 "
            } else {
                " 补充（可选） "
            })
            .padding(Padding::horizontal(1));
        let sup_inner = sup_block.inner(sup_area);
        // Use display_value for password masking
        let val = state.supplement.display_value(state.request.is_secret);
        let p = Paragraph::new(val)
            .style(Style::default().fg(theme.foreground).bg(theme.input_bg))
            .block(sup_block);
        f.render_widget(p, sup_area);
        *supplement_hit_area = Some(sup_area);

        if sup_focus && sup_inner.width > 0 && sup_inner.height > 0 {
            let vc = state.supplement.visual_cursor() as u16;
            let cx = sup_inner
                .x
                .saturating_add(vc.min(sup_inner.width.saturating_sub(2)));
            let cy = sup_inner.y;
            f.set_cursor_position((cx, cy));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::wrap_text_to_lines;

    #[test]
    fn preserves_hard_newlines() {
        assert_eq!(
            wrap_text_to_lines("line one\nline two", 100),
            vec!["line one".to_string(), "line two".to_string()]
        );
    }

    #[test]
    fn keeps_blank_line_for_double_newline() {
        assert_eq!(
            wrap_text_to_lines("a\n\nb", 100),
            vec!["a".to_string(), String::new(), "b".to_string()]
        );
    }

    #[test]
    fn wraps_each_segment_by_width() {
        assert_eq!(
            wrap_text_to_lines("abc\nabcdef", 4),
            vec!["abc".to_string(), "abcd".to_string(), "ef".to_string()]
        );
    }

    #[test]
    fn empty_input_returns_single_empty_line() {
        assert_eq!(wrap_text_to_lines("", 10), vec![String::new()]);
    }
}
