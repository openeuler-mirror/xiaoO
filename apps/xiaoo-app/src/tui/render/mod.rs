mod header;
pub(crate) mod interaction_prompt;
pub(crate) mod markdown;
mod overlay;
pub(crate) mod provider_dialog;
mod root;
mod session_diff;
pub(crate) mod status_panel;
pub(crate) mod theme;
mod transcript;
mod utils;

pub(crate) use utils::{paste_into_input, scroll_offset_from_drag};
