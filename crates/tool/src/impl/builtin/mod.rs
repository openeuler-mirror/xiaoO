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
#[path = "../../../../../tests/unit/tool/impl/builtin/test_support.rs"]
mod test_support;

use agent_contracts::backend::capability::filesystem::WriteMode;

pub use todo_write::{
    checkpoint_todos, open_todo_lines, restore_checkpoint_todos, CheckpointTodoItem,
};
pub use tool_source::BuiltinToolSource;

pub use file_edit::FILE_EDIT_TOOL_NAME;
pub use file_write::FILE_WRITE_TOOL_NAME;
pub use skill::{SKILL_NAME_INPUT_KEY, SKILL_TOOL_NAME};

/// Input key carrying the target file path in the `file_write` / `file_edit`
/// schemas (same field for both tools). Anchored once here——the two specs'
/// JSON schemas, the typed inputs, and the agent loop's write-pivot
/// collection all reference this spelling.
pub const FILE_PATH_INPUT_KEY: &str = "file_path";

fn preferred_overwrite_mode(supports_atomic_write: bool) -> WriteMode {
    if supports_atomic_write {
        WriteMode::AtomicOverwrite
    } else {
        WriteMode::Overwrite
    }
}
