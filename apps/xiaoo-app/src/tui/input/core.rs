use crossterm::event::{Event, KeyCode, KeyModifiers};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Default)]
pub struct Input {
    value: String,
    cursor: usize,
    /// The character-index where a selection started, if any.
    selection_anchor: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
pub enum InputRequest {
    InsertChar(char),
}

pub trait EventHandler {
    fn handle_event(&mut self, event: &Event);
}

impl Input {
    pub fn with_value(mut self, value: String) -> Self {
        self.cursor = value.chars().count();
        self.value = value;
        self
    }

    pub fn with_cursor(mut self, cursor: usize) -> Self {
        self.cursor = cursor.min(self.value.chars().count());
        self
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn visual_cursor(&self) -> usize {
        self.value
            .chars()
            .take(self.cursor)
            .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
            .sum()
    }

    // ── Selection helpers ─────────────────────────────────────────────────────

    /// Set the selection anchor at the current cursor position.
    pub fn set_anchor(&mut self) {
        self.selection_anchor = Some(self.cursor);
    }

    /// Returns the selected character range as `start..end` (inclusive start,
    /// exclusive end), where `start <= end`.  Returns `None` when there is no
    /// anchor or the anchor equals the cursor (empty selection).
    pub fn selected_range(&self) -> Option<std::ops::Range<usize>> {
        let anchor = self.selection_anchor?;
        if anchor == self.cursor {
            return None;
        }
        let start = anchor.min(self.cursor);
        let end = anchor.max(self.cursor);
        Some(start..end)
    }

    /// Returns the selected text slice, or `None` when nothing is selected.
    pub fn selected_text(&self) -> Option<&str> {
        let range = self.selected_range()?;
        // Convert char indices to byte indices.
        let mut byte_start = 0;
        let mut byte_end = 0;
        let mut char_idx = 0;
        for (byte_idx, _ch) in self.value.char_indices() {
            if char_idx == range.start {
                byte_start = byte_idx;
            }
            if char_idx == range.end {
                byte_end = byte_idx;
                break;
            }
            char_idx += 1;
        }
        // Handle the case where end == value.len() in chars.
        if char_idx < range.end {
            byte_end = self.value.len();
        }
        Some(&self.value[byte_start..byte_end])
    }

    /// Clear any active selection (anchor is removed; cursor stays).
    pub fn clear_selection(&mut self) {
        self.selection_anchor = None;
    }

    /// Delete the selected range, move cursor to the start of the deleted
    /// range, and return the deleted text.  Returns `None` when nothing was
    /// selected.
    pub fn delete_selected(&mut self) -> Option<String> {
        let range = self.selected_range()?;
        let deleted: String = self
            .value
            .chars()
            .skip(range.start)
            .take(range.end - range.start)
            .collect();
        let before: String = self.value.chars().take(range.start).collect();
        let after: String = self.value.chars().skip(range.end).collect();
        self.value = format!("{}{}", before, after);
        self.cursor = range.start;
        self.selection_anchor = None;
        Some(deleted)
    }

    // ──────────────────────────────────────────────────────────────────────────

    pub fn reset(&mut self) {
        self.value.clear();
        self.cursor = 0;
        self.selection_anchor = None;
    }

    pub fn handle(&mut self, request: InputRequest) {
        match request {
            InputRequest::InsertChar(ch) => self.insert_char(ch),
        }
    }

    fn insert_char(&mut self, ch: char) {
        // If there's a selection, replace it.
        if self.selected_range().is_some() {
            self.delete_selected();
        }
        let mut chars: Vec<char> = self.value.chars().collect();
        let cursor = self.cursor.min(chars.len());
        chars.insert(cursor, ch);
        self.value = chars.into_iter().collect();
        self.cursor = cursor.saturating_add(1);
    }

    fn backspace(&mut self) {
        if self.selected_range().is_some() {
            self.delete_selected();
            return;
        }
        if self.cursor == 0 {
            return;
        }
        let mut chars: Vec<char> = self.value.chars().collect();
        let cursor = self.cursor.min(chars.len());
        chars.remove(cursor - 1);
        self.value = chars.into_iter().collect();
        self.cursor = cursor - 1;
    }

    fn delete(&mut self) {
        if self.selected_range().is_some() {
            self.delete_selected();
            return;
        }
        let mut chars: Vec<char> = self.value.chars().collect();
        let cursor = self.cursor.min(chars.len());
        if cursor >= chars.len() {
            return;
        }
        chars.remove(cursor);
        self.value = chars.into_iter().collect();
        self.cursor = cursor;
    }

    fn is_backspace_compat(key: &crossterm::event::KeyEvent) -> bool {
        match key.code {
            KeyCode::Backspace => true,
            // Some terminals send Ctrl+H for Backspace.
            KeyCode::Char('h') | KeyCode::Char('H') => {
                key.modifiers.contains(KeyModifiers::CONTROL)
            }
            // Others surface raw ASCII control characters for BS/DEL.
            KeyCode::Char('\u{8}') | KeyCode::Char('\u{7f}') => true,
            _ => false,
        }
    }
}

impl EventHandler for Input {
    fn handle_event(&mut self, event: &Event) {
        let Event::Key(key) = event else {
            return;
        };

        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        if Self::is_backspace_compat(key) {
            self.backspace();
            return;
        }

        match key.code {
            // Ctrl+A – select all
            KeyCode::Char('a') if ctrl => {
                self.selection_anchor = Some(0);
                self.cursor = self.value.chars().count();
            }
            // Ctrl+X – cut (handled externally via selected_text + delete_selected;
            // here we just delete so the caller can detect the selection first)
            KeyCode::Char('x') if ctrl => {
                // Deletion is handled by the key event handler in event_key.rs
                // which reads selected_text() before calling delete_selected().
                // We do nothing here so event_key.rs can intercept Ctrl+X first.
            }
            // Ignore all other Ctrl+letter combos (handled elsewhere).
            KeyCode::Char(_ch) if ctrl => {}
            KeyCode::Char(ch) => self.insert_char(ch),
            KeyCode::Delete => self.delete(),
            // Shift+Left – extend selection leftward
            KeyCode::Left if shift => {
                if self.selection_anchor.is_none() {
                    self.set_anchor();
                }
                self.cursor = self.cursor.saturating_sub(1);
            }
            // Shift+Right – extend selection rightward
            KeyCode::Right if shift => {
                if self.selection_anchor.is_none() {
                    self.set_anchor();
                }
                self.cursor = (self.cursor + 1).min(self.value.chars().count());
            }
            // Shift+Home – extend selection to start of line
            KeyCode::Home if shift => {
                if self.selection_anchor.is_none() {
                    self.set_anchor();
                }
                // Move to start of current line.
                let before: Vec<char> = self.value.chars().take(self.cursor).collect();
                let line_start = before
                    .iter()
                    .rposition(|&c| c == '\n')
                    .map(|p| p + 1)
                    .unwrap_or(0);
                self.cursor = line_start;
            }
            // Shift+End – extend selection to end of line
            KeyCode::End if shift => {
                if self.selection_anchor.is_none() {
                    self.set_anchor();
                }
                let total = self.value.chars().count();
                let rest_start = self.cursor;
                let line_end = self
                    .value
                    .chars()
                    .skip(rest_start)
                    .position(|c| c == '\n')
                    .map(|p| rest_start + p)
                    .unwrap_or(total);
                self.cursor = line_end;
            }
            // Unmodified navigation clears selection
            KeyCode::Left => {
                // If there's a selection, jump to its start; otherwise move left.
                if let Some(range) = self.selected_range() {
                    self.cursor = range.start;
                } else {
                    self.cursor = self.cursor.saturating_sub(1);
                }
                self.selection_anchor = None;
            }
            KeyCode::Right => {
                if let Some(range) = self.selected_range() {
                    self.cursor = range.end;
                } else {
                    self.cursor = (self.cursor + 1).min(self.value.chars().count());
                }
                self.selection_anchor = None;
            }
            KeyCode::Home => {
                let before: Vec<char> = self.value.chars().take(self.cursor).collect();
                let line_start = before
                    .iter()
                    .rposition(|&c| c == '\n')
                    .map(|p| p + 1)
                    .unwrap_or(0);
                self.cursor = line_start;
                self.selection_anchor = None;
            }
            KeyCode::End => {
                let total = self.value.chars().count();
                let rest_start = self.cursor;
                let line_end = self
                    .value
                    .chars()
                    .skip(rest_start)
                    .position(|c| c == '\n')
                    .map(|p| rest_start + p)
                    .unwrap_or(total);
                self.cursor = line_end;
                self.selection_anchor = None;
            }
            _ => {}
        }
    }
}

impl From<&str> for Input {
    fn from(value: &str) -> Self {
        Self::default().with_value(value.to_string())
    }
}

impl From<String> for Input {
    fn from(value: String) -> Self {
        Self::default().with_value(value)
    }
}

#[cfg(test)]
mod tests {
    use super::{EventHandler, Input};
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn backspace_key_deletes_previous_character() {
        let mut input = Input::default().with_value("hello".to_string());
        input.handle_event(&Event::Key(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )));

        assert_eq!(input.value(), "hell");
        assert_eq!(input.cursor(), 4);
    }

    #[test]
    fn ctrl_h_is_treated_as_backspace() {
        let mut input = Input::default().with_value("hello".to_string());
        input.handle_event(&Event::Key(KeyEvent::new(
            KeyCode::Char('h'),
            KeyModifiers::CONTROL,
        )));

        assert_eq!(input.value(), "hell");
        assert_eq!(input.cursor(), 4);
    }

    #[test]
    fn del_control_character_is_treated_as_backspace() {
        let mut input = Input::default().with_value("hello".to_string());
        input.handle_event(&Event::Key(KeyEvent::new(
            KeyCode::Char('\u{7f}'),
            KeyModifiers::NONE,
        )));

        assert_eq!(input.value(), "hell");
        assert_eq!(input.cursor(), 4);
    }
}
