//! 交互式选项 + 可选补充输入（TUI 输入区上方）。
//!
//! ## 与后端接线（预埋）
//! - **入站**：任一线程构造 [`PromptRequest`] 后调用 `App::open_interaction_prompt`（见 `app.rs`）。
//! - **出站**：通过打开时传入的 `UnboundedSender<UserPromptResult>` 将用户选择发回；
//!   上层可写入会话、HTTP POST 或合并进下一轮 `ChatMessage`。
//! - 入站：`SessionTurnUpdate::InteractionPrompt` 由 `poll_stream_updates` 打开本面板。

use crate::input::Input;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, Padding, Paragraph, Wrap},
    Frame,
};
use serde::{Deserialize, Serialize};

use super::theme::Theme;

/// 单个可选项（可与 JSON 对齐）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptChoice {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
}

/// 后端 → TUI：请求用户从列表中选择，并可选择是否允许补充输入。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptRequest {
    pub request_id: String,
    pub title: String,
    pub body: Option<String>,
    pub choices: Vec<PromptChoice>,
    #[serde(default)]
    pub allow_custom_input: bool,
    /// 多选：列表中 Space 切换选中，Enter 提交 `PromptResolution::Multi`。
    #[serde(default)]
    pub multi_select: bool,
    pub default_index: Option<usize>,
}

/// 用户操作结果（TUI → 后端 / 会话）。
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
    /// 预留，与 `PromptRequest::multi_select` 对应。
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

/// 运行时 UI 状态（不参与序列化）。
pub struct InteractionPromptState {
    pub request: PromptRequest,
    pub selected: usize,
    /// 列表首行对应 `choices` 的下标（用于滚动）。
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

/// 计算提示块占用高度（含边框），用于 `Constraint::Length`。
pub fn interaction_prompt_outer_height(req: &PromptRequest) -> u16 {
    let border = 2u16;
    let body_h = if req.body.as_ref().map_or(false, |s| !s.is_empty()) {
        1
    } else {
        0
    };
    let list_cap = if req.allow_custom_input { 4 } else { 6 };
    let list_h = req.choices.len().min(list_cap) as u16;
    let sup_h = if req.allow_custom_input { 3 } else { 0 };
    let total = border + body_h + list_h + sup_h;
    let max_outer = 11u16;
    total.min(max_outer).max(border + 1)
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

    let title = format!(" {} ", state.request.title);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border_active))
        .title(title)
        .style(Style::default().bg(theme.background));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut constraints: Vec<Constraint> = Vec::new();
    if state.request.body.as_ref().map_or(false, |s| !s.is_empty()) {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Min(1));
    if state.request.allow_custom_input {
        constraints.push(Constraint::Length(3));
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    let mut idx = 0usize;
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
    let vmax = state.list_visible_max();
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
                    "[✓] "
                } else {
                    "[ ] "
                }
            } else {
                ""
            };
            let mut spans = vec![Span::styled(format!("{}{} ", mark, ch.label), style)];
            if let Some(d) = &ch.description {
                spans.push(Span::styled(
                    format!(" — {}", d),
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
            .title(" 补充（可选） ")
            .padding(Padding::horizontal(1));
        let sup_inner = sup_block.inner(sup_area);
        let val = state.supplement.value().to_string();
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
