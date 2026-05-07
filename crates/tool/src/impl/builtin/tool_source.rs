use agent_contracts::tool::{DiscoveredTool, ToolSource};

use super::ask_user_question::discover_ask_user_question;
use super::bash::discover_bash;
use super::count_text_length::discover_count_text_length;
use super::file_edit::discover_file_edit;
use super::file_read::discover_file_read;
use super::file_write::discover_file_write;
use super::glob::discover_glob;
use super::grep::discover_grep;
use super::join_subagent::discover_join_subagent;
use super::lsp::discover_lsp;
use super::print_hello_world::discover_print_hello_world;
use super::send_file::discover_send_file;
use super::skill::discover_skill;
use super::spawn_subagent::discover_spawn_subagent;
use super::todo_write::discover_todo_write;
use super::webfetch::discover_webfetch;
use super::websearch::discover_web_search;
use crate::r#impl::ToolRuntimeServices;

/// A built-in tool source.
pub struct BuiltinToolSource {
    services: ToolRuntimeServices,
}

impl BuiltinToolSource {
    /// Creates a new built-in tool source.
    pub fn new(services: ToolRuntimeServices) -> Self {
        Self { services }
    }
}

impl ToolSource for BuiltinToolSource {
    fn discover(&self) -> Vec<DiscoveredTool> {
        vec![
            discover_ask_user_question(),
            discover_print_hello_world(),
            discover_count_text_length(),
            discover_file_edit(self.services.clone()),
            discover_file_read(self.services.clone()),
            discover_file_write(self.services.clone()),
            discover_bash(),
            discover_glob(),
            discover_grep(),
            discover_webfetch(),
            discover_web_search(),
            discover_spawn_subagent(self.services.clone()),
            discover_join_subagent(self.services.clone()),
            discover_skill(),
            discover_todo_write(),
            discover_send_file(),
            discover_lsp(self.services.clone()),
        ]
    }
}
