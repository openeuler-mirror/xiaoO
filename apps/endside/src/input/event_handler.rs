use anyhow::Result;
use crossterm::event::Event;

use crate::app::App;

impl App {
    pub async fn handle_event(&mut self, event: Event) -> Result<()> {
        match event {
            Event::Mouse(mouse_event) => self.handle_mouse_event(mouse_event),
            Event::Paste(text) => self.handle_paste_event(&text),
            Event::Key(key) => self.handle_key_event(key).await,
            _ => Ok(()),
        }
    }
}
