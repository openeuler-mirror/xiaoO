use crossterm::event::{Event, KeyCode, KeyModifiers};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Default)]
pub struct Input {
    value: String,
    cursor: usize,
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

    pub fn reset(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }

    pub fn handle(&mut self, request: InputRequest) {
        match request {
            InputRequest::InsertChar(ch) => self.insert_char(ch),
        }
    }

    fn insert_char(&mut self, ch: char) {
        let mut chars: Vec<char> = self.value.chars().collect();
        let cursor = self.cursor.min(chars.len());
        chars.insert(cursor, ch);
        self.value = chars.into_iter().collect();
        self.cursor = cursor.saturating_add(1);
    }

    fn backspace(&mut self) {
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
        let mut chars: Vec<char> = self.value.chars().collect();
        let cursor = self.cursor.min(chars.len());
        if cursor >= chars.len() {
            return;
        }
        chars.remove(cursor);
        self.value = chars.into_iter().collect();
        self.cursor = cursor;
    }
}

impl EventHandler for Input {
    fn handle_event(&mut self, event: &Event) {
        let Event::Key(key) = event else {
            return;
        };

        match key.code {
            KeyCode::Char(_ch) if key.modifiers.contains(KeyModifiers::CONTROL) => {}
            KeyCode::Char(ch) => self.insert_char(ch),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(self.value.chars().count());
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.value.chars().count(),
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
