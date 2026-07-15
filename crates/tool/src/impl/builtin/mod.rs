mod ask_user_question;
mod bash;
mod count_text_length;
mod file_edit;
pub mod file_read;
mod file_write;
mod glob;
mod grep;
mod join_subagent;
mod lsp;
mod print_hello_world;
mod send_file;
mod skill;
mod spawn_subagent;
mod todo_write;
mod tool_source;
mod webfetch;
mod websearch;

#[cfg(test)]
mod test_support;

use agent_contracts::backend::capability::filesystem::WriteMode;

pub use todo_write::open_todo_lines;
pub use tool_source::BuiltinToolSource;

fn preferred_overwrite_mode(supports_atomic_write: bool) -> WriteMode {
    if supports_atomic_write {
        WriteMode::AtomicOverwrite
    } else {
        WriteMode::Overwrite
    }
}
