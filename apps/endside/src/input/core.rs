use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
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

    /// Move the cursor to a character index (clamped to the value length).
    /// The selection anchor, if any, is left untouched — dragging uses this
    /// to extend a selection.
    pub fn set_cursor(&mut self, cursor: usize) {
        self.cursor = cursor.min(self.value.chars().count());
    }

    /// Get display value (for password input, return masked string like "****")
    pub fn display_value(&self, is_secret: bool) -> String {
        if is_secret {
            "*".repeat(self.value.chars().count())
        } else {
            self.value.clone()
        }
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
        // Convert char indices to byte indices. A range boundary equal to
        // the total char count has no `char_indices` entry (there is no
        // index "one past the end"), so the byte offset falls back to the
        // full byte length in that case.
        let mut byte_start = 0;
        let mut byte_end = 0;
        let mut found_start = false;
        let mut found_end = false;
        let mut char_idx = 0;
        for (byte_idx, _ch) in self.value.char_indices() {
            if char_idx == range.start && !found_start {
                byte_start = byte_idx;
                found_start = true;
            }
            if char_idx == range.end {
                byte_end = byte_idx;
                found_end = true;
                break;
            }
            char_idx += 1;
        }
        if !found_end {
            byte_end = self.value.len();
        }
        if !found_start {
            byte_start = self.value.len();
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
        } else {
            // No (non-empty) selection: drop any stale anchor so the freshly
            // inserted char does not silently start a new selection. Without
            // this, a mouse click (which sets anchor == cursor) would make the
            // SECOND inserted char replace the first — typing "ab" yielded
            // "b", and pasting "/root/…" dropped the leading "/".
            self.selection_anchor = None;
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

    fn move_cursor_up(&mut self) {
        let chars: Vec<char> = self.value.chars().collect();
        if self.cursor == 0 {
            return;
        }

        let current_line_start = self.current_line_start(&chars);
        if current_line_start == 0 {
            self.cursor = 0;
            return;
        }

        let current_col = self.cursor - current_line_start;
        let prev_line_end = current_line_start.saturating_sub(1);
        let prev_line_start = self.line_start_before(&chars, prev_line_end);
        let prev_line_len = prev_line_end.saturating_sub(prev_line_start);

        self.cursor = prev_line_start + current_col.min(prev_line_len);
    }

    fn move_cursor_down(&mut self) {
        let chars: Vec<char> = self.value.chars().collect();
        let total = chars.len();
        if self.cursor >= total {
            return;
        }

        let current_line_start = self.current_line_start(&chars);
        let current_line_end = self.current_line_end(&chars, total);

        if current_line_end >= total {
            self.cursor = total;
            return;
        }

        let current_col = self.cursor - current_line_start;
        let next_line_start = current_line_end + 1;
        let next_line_end = self.current_line_end(&chars, total);
        let next_line_len = next_line_end.saturating_sub(next_line_start);

        self.cursor = next_line_start + current_col.min(next_line_len);
    }

    fn current_line_start(&self, chars: &[char]) -> usize {
        chars
            .iter()
            .take(self.cursor)
            .enumerate()
            .rev()
            .find(|(_, &c)| c == '\n')
            .map(|(i, _)| i + 1)
            .unwrap_or(0)
    }

    fn current_line_end(&self, chars: &[char], total: usize) -> usize {
        chars
            .iter()
            .skip(self.cursor)
            .position(|&c| c == '\n')
            .map(|p| self.cursor + p)
            .unwrap_or(total)
    }

    fn line_start_before(&self, chars: &[char], position: usize) -> usize {
        chars
            .iter()
            .take(position)
            .enumerate()
            .rev()
            .find(|(_, &c)| c == '\n')
            .map(|(i, _)| i + 1)
            .unwrap_or(0)
    }

    // ── readline-style editing primitives ─────────────────────────────────────
    // These implement the readline/emacs editing shortcuts (see
    // `handle_input_key` below; the bindings are hard-coded, not configurable).
    // Plain-key handling in `handle_event` keeps the classic behavior for the
    // native editing keys (arrows / Home / End / Backspace / Delete / Shift).

    fn move_to_line_start(&mut self) {
        let chars: Vec<char> = self.value.chars().collect();
        self.cursor = self.current_line_start(&chars);
        self.selection_anchor = None;
    }

    fn move_to_line_end(&mut self) {
        let chars: Vec<char> = self.value.chars().collect();
        let total = chars.len();
        self.cursor = self.current_line_end(&chars, total);
        self.selection_anchor = None;
    }

    fn move_left(&mut self) {
        if let Some(range) = self.selected_range() {
            self.cursor = range.start;
        } else {
            self.cursor = self.cursor.saturating_sub(1);
        }
        self.selection_anchor = None;
    }

    fn move_right(&mut self) {
        if let Some(range) = self.selected_range() {
            self.cursor = range.end;
        } else {
            self.cursor = (self.cursor + 1).min(self.value.chars().count());
        }
        self.selection_anchor = None;
    }

    /// Move to the start of the previous word (over whitespace, then one word).
    fn word_left(&mut self) {
        if let Some(range) = self.selected_range() {
            self.cursor = range.start;
            self.selection_anchor = None;
            return;
        }
        let chars: Vec<char> = self.value.chars().collect();
        let mut i = self.cursor;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        self.cursor = i;
        self.selection_anchor = None;
    }

    /// Move to the start of the next word (over one word, then whitespace).
    fn word_right(&mut self) {
        if let Some(range) = self.selected_range() {
            self.cursor = range.end;
            self.selection_anchor = None;
            return;
        }
        let chars: Vec<char> = self.value.chars().collect();
        let n = chars.len();
        let mut i = self.cursor;
        if i >= n {
            self.selection_anchor = None;
            return;
        }
        while i < n && !chars[i].is_whitespace() {
            i += 1;
        }
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        self.cursor = i;
        self.selection_anchor = None;
    }

    /// Delete a character range `[start, end)` (char indices) and put the
    /// caret at the deletion point.
    fn delete_char_range(&mut self, start: usize, end: usize) {
        let mut chars: Vec<char> = self.value.chars().collect();
        if start >= end || end > chars.len() {
            return;
        }
        chars.drain(start..end);
        self.value = chars.into_iter().collect();
        self.cursor = start;
        self.selection_anchor = None;
    }

    fn delete_word_backward(&mut self) {
        if self.selected_range().is_some() {
            self.delete_selected();
            return;
        }
        let chars: Vec<char> = self.value.chars().collect();
        let end = self.cursor;
        if end == 0 {
            return;
        }
        let mut start = end;
        while start > 0 && chars[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        self.delete_char_range(start, end);
    }

    fn delete_word_forward(&mut self) {
        if self.selected_range().is_some() {
            self.delete_selected();
            return;
        }
        let chars: Vec<char> = self.value.chars().collect();
        let n = chars.len();
        let start = self.cursor;
        if start >= n {
            return;
        }
        let mut end = start;
        while end < n && chars[end].is_whitespace() {
            end += 1;
        }
        while end < n && !chars[end].is_whitespace() {
            end += 1;
        }
        self.delete_char_range(start, end);
    }

    /// readline `Ctrl+U`: delete from the line start up to the caret.
    fn kill_to_line_start(&mut self) {
        if self.selected_range().is_some() {
            self.delete_selected();
            return;
        }
        let chars: Vec<char> = self.value.chars().collect();
        let end = self.cursor;
        let start = self.current_line_start(&chars);
        self.delete_char_range(start, end);
    }

    /// readline `Ctrl+K`: delete from the caret to the line end, *including*
    /// the trailing newline when there is one, so the following line joins the
    /// current one.
    fn kill_to_line_end(&mut self) {
        if self.selected_range().is_some() {
            self.delete_selected();
            return;
        }
        let chars: Vec<char> = self.value.chars().collect();
        let total = chars.len();
        let start = self.cursor;
        if start >= total {
            return;
        }
        let mut end = start;
        while end < total && chars[end] != '\n' {
            end += 1;
        }
        if end < total {
            end += 1; // swallow the newline as well
        }
        self.delete_char_range(start, end);
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
            // Ctrl+A (line start) and Ctrl+X (cut) are resolved one layer up —
            // by `handle_input_key` and by the key event handler in
            // event_key.rs — so this widget never selects-all or cuts by
            // itself.
            //
            // Ignore all other Ctrl+letter combos (handled elsewhere); never
            // insert control characters from them. Exception: Ctrl+Alt+letter
            // (AltGr on European layouts, where AltGr == Ctrl+Alt) is printable
            // input and must reach the insert arm below.
            KeyCode::Char(_ch) if ctrl && !key.modifiers.intersects(KeyModifiers::ALT) => {}
            // Never insert control characters: some terminals/forwarders
            // surface raw ASCII controls (e.g. ESC as `\u{1b}`, NUL) as
            // `Char` events instead of dedicated key codes. Inserting them
            // corrupts the input line. (Backspace-compat chars are handled
            // by `is_backspace_compat` before the match.)
            KeyCode::Char(ch) if !ch.is_control() => self.insert_char(ch),
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
            KeyCode::Up => {
                self.move_cursor_up();
                self.selection_anchor = None;
            }
            KeyCode::Down => {
                self.move_cursor_down();
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

/// The three modifier bits a terminal can actually deliver: Ctrl / Alt /
/// Shift. Super/⌘ is intercepted by the terminal emulator, so it never
/// reaches the app.
fn chord_mask(modifiers: KeyModifiers) -> KeyModifiers {
    modifiers & (KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT)
}

/// True when `key` is exactly the chord `code` + `mods`. Modifier bits are
/// compared *exactly* (so `Ctrl+Shift+J` never matches `Ctrl+J`), and letters
/// are matched case-insensitively because terminals deliver either
/// `Char('a')` or `Char('A')` for the same physical key.
pub(crate) fn is_chord(key: &KeyEvent, code: KeyCode, mods: KeyModifiers) -> bool {
    fn normalize(code: KeyCode) -> KeyCode {
        match code {
            KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
            other => other,
        }
    }
    normalize(key.code) == normalize(code) && chord_mask(key.modifiers) == chord_mask(mods)
}

/// Apply the hard-coded readline (emacs) editing shortcuts to `input`;
/// unmatched chords fall through to the widget's plain-key handling.
///
/// | chord | action |
/// |---|---|
/// | `Ctrl+A` / `Ctrl+E` | move to line start / end |
/// | `Ctrl+B` / `Ctrl+F` | move one char left / right |
/// | `Alt+B` / `Alt+F` | move one word left / right |
/// | `Ctrl+D` | delete one char forward |
/// | `Ctrl+W` / `Alt+D` | delete one word backward / forward |
/// | `Ctrl+U` / `Ctrl+K` | kill to line start / end |
///
/// These bindings are hard-coded — there is no keymap configuration. The
/// app-level shortcuts (submit / newline / history / copy / cut) are resolved
/// by the caller (`event_key.rs`) and are deliberately not consumed here; in a
/// bare `Input` (dialog fields) they degrade to the widget's plain handling,
/// which ignores control chords and inserts plain text only.
///
/// Returns `true` when the chord was fully consumed (an editing action was
/// applied, or an unbound Alt+letter was dropped).
pub(crate) fn handle_input_key(input: &mut Input, key: KeyEvent) -> bool {
    use KeyCode::*;
    const CTRL: KeyModifiers = KeyModifiers::CONTROL;
    const ALT: KeyModifiers = KeyModifiers::ALT;

    if is_chord(&key, Char('a'), CTRL) {
        input.move_to_line_start();
    } else if is_chord(&key, Char('e'), CTRL) {
        input.move_to_line_end();
    } else if is_chord(&key, Char('b'), CTRL) {
        input.move_left();
    } else if is_chord(&key, Char('f'), CTRL) {
        input.move_right();
    } else if is_chord(&key, Char('b'), ALT) {
        input.word_left();
    } else if is_chord(&key, Char('f'), ALT) {
        input.word_right();
    } else if is_chord(&key, Char('d'), CTRL) {
        input.delete();
    } else if is_chord(&key, Char('w'), CTRL) {
        input.delete_word_backward();
    } else if is_chord(&key, Char('d'), ALT) {
        input.delete_word_forward();
    } else if is_chord(&key, Char('u'), CTRL) {
        input.kill_to_line_start();
    } else if is_chord(&key, Char('k'), CTRL) {
        input.kill_to_line_end();
    } else {
        // Unbound Alt+letter (pure ALT without CONTROL — AltGr stays a valid
        // input) is dropped in EVERY input field from this single guard: the
        // main editor reaches this helper with the same keys, and the dialog
        // inputs call it directly. Alt+letter therefore never types the bare
        // letter (a bound Alt chord was applied above and returned `true`).
        if let Char(_) = key.code {
            if key.modifiers.contains(ALT) && !key.modifiers.contains(CTRL) {
                return true;
            }
        }
        input.handle_event(&Event::Key(key));
        return false;
    }
    true
}

#[cfg(test)]
#[path = "../../../../tests/unit/endside/input/core_test.rs"]
mod tests;
