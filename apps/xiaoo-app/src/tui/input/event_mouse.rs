use anyhow::Result;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use crate::app::App;
use crate::interaction_prompt::PromptFocus;
use crate::render::scroll_offset_from_drag;

impl App {
    pub(crate) fn handle_mouse_event(&mut self, mouse_event: MouseEvent) -> Result<()> {
        if self.state.api_key_dialog.is_some() || self.state.provider_dialog.is_some() {
            return Ok(());
        }

        if self.state.interaction_prompt.is_some() {
            self.handle_interaction_prompt_mouse(mouse_event)?;
            return Ok(());
        }

        if self.state.slash_menu_visible() {
            self.handle_slash_popup_mouse(mouse_event)?;
            return Ok(());
        }

        self.handle_slash_popup_mouse(mouse_event)?;
        self.handle_transcript_mouse(mouse_event);
        Ok(())
    }

    fn handle_interaction_prompt_mouse(&mut self, mouse_event: MouseEvent) -> Result<()> {
        if self.state.interaction_prompt.is_none()
            || mouse_event.kind != MouseEventKind::Down(MouseButton::Left)
        {
            return Ok(());
        }

        if let Some(list_rect) = self.state.render_state.interaction_prompt_list_area {
            if mouse_in_rect(mouse_event.column, mouse_event.row, list_rect) {
                if let Some(prompt) = self.state.interaction_prompt.as_mut() {
                    prompt.focus = PromptFocus::List;
                    let row = (mouse_event.row.saturating_sub(list_rect.y)) as usize;
                    let visible_max = prompt.list_visible_max();
                    let index = prompt.list_scroll + row.min(visible_max.saturating_sub(1));
                    if index < prompt.request.choices.len() {
                        prompt.selected = index;
                    }
                }
                return Ok(());
            }
        }

        if let Some(supplement_rect) = self.state.render_state.interaction_prompt_supplement_area {
            if mouse_in_rect(mouse_event.column, mouse_event.row, supplement_rect) {
                if let Some(prompt) = self.state.interaction_prompt.as_mut() {
                    prompt.focus = PromptFocus::Supplement;
                }
            }
        }
        Ok(())
    }

    fn handle_slash_popup_mouse(&mut self, mouse_event: MouseEvent) -> Result<()> {
        if mouse_event.kind != MouseEventKind::Down(MouseButton::Left)
            || !self.state.slash_menu_visible()
        {
            return Ok(());
        }
        if let Some(inner) = self.state.render_state.slash_popup_inner {
            if mouse_in_rect(mouse_event.column, mouse_event.row, inner) {
                let row = (mouse_event.row - inner.y) as usize;
                let value = self.state.chat_state.input.value();
                let cursor = self.state.chat_state.input.cursor();
                if let Some(prefix) = crate::slash_complete::slash_typed_prefix(value, cursor) {
                    let candidates = crate::slash_complete::candidates_for_prefix(
                        &prefix,
                        &self.state.external_commands,
                    );
                    if row < candidates.len() {
                        self.state.slash.selected = row;
                        self.state.apply_slash_selection();
                    }
                }
            }
        }
        Ok(())
    }

    fn handle_transcript_mouse(&mut self, mouse_event: MouseEvent) {
        let Some(area) = self.state.render_state.messages_area else {
            return;
        };
        let in_scrollbar_zone = mouse_event.column >= area.x + area.width.saturating_sub(2)
            && mouse_event.column < area.x + area.width
            && mouse_event.row >= area.y
            && mouse_event.row < area.y + area.height;
        match mouse_event.kind {
            MouseEventKind::ScrollUp => {
                self.state.chat_state.scroll_up();
            }
            MouseEventKind::ScrollDown => {
                self.state.chat_state.scroll_down();
            }
            MouseEventKind::Down(MouseButton::Left) if in_scrollbar_zone => {
                self.state.chat_state.scrollbar_dragging = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(region) = self
                    .state
                    .render_state
                    .tool_toggle_regions
                    .iter()
                    .find(|region| mouse_in_rect(mouse_event.column, mouse_event.row, region.rect))
                    .copied()
                {
                    if let Some(message) =
                        self.state.chat_state.messages.get_mut(region.message_index)
                    {
                        if let Some(tool) = message.tool_state.as_mut() {
                            tool.expanded = !tool.expanded;
                        }
                    }
                }
            }
            MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left)
                if self.state.chat_state.scrollbar_dragging =>
            {
                let track_height = area.height as usize;
                let max_scroll = self.state.chat_state.max_scroll_offset();
                if track_height > 0 && max_scroll > 0 {
                    let rel_y = (mouse_event.row.saturating_sub(area.y) as usize)
                        .min(track_height.saturating_sub(1));
                    self.state
                        .chat_state
                        .set_scroll_offset(scroll_offset_from_drag(
                            rel_y,
                            track_height,
                            max_scroll,
                        ));
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.state.chat_state.scrollbar_dragging = false;
            }
            _ => {}
        }
    }
}

fn mouse_in_rect(column: u16, row: u16, rect: Rect) -> bool {
    column >= rect.x
        && column < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}
