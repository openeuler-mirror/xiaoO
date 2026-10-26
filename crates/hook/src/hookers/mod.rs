mod builder;
mod builtin;
mod hook_point_category;
#[cfg(feature = "plugin_hook")]
mod plugin;

pub(crate) use builder::build_hookers;
pub use hook_point_category::{
    escape_hook_point_segment, resolve_hook_point_category, HookPointCategory,
};
