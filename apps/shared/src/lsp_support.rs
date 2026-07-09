use lsp::{AutoInstall, ServerConfig};

pub trait ExtraServerConfigView {
    fn id(&self) -> &str;
    fn extensions(&self) -> &[String];
    fn command(&self) -> &str;
    fn args(&self) -> &[String];
    fn root_markers(&self) -> &[String];
    fn language_id(&self) -> &str;
}

/// Convert user-supplied [`ExtraServerConfig`] entries to the lsp crate's
/// [`ServerConfig`] format. Strings are leaked to produce `'static` slices,
/// matching the format used by the built-in server table.
pub fn build_extra_server_configs<T: ExtraServerConfigView>(
    extra_servers: &[T],
) -> Vec<ServerConfig> {
    extra_servers
        .iter()
        .map(|c| {
            let id: &'static str = Box::leak(c.id().to_string().into_boxed_str());
            let command: &'static str = Box::leak(c.command().to_string().into_boxed_str());
            let language_id: &'static str = Box::leak(c.language_id().to_string().into_boxed_str());
            let extensions: &'static [&'static str] = Box::leak(
                c.extensions()
                    .iter()
                    .map(|e| -> &'static str { Box::leak(e.clone().into_boxed_str()) })
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            );
            let args: &'static [&'static str] = Box::leak(
                c.args()
                    .iter()
                    .map(|a| -> &'static str { Box::leak(a.clone().into_boxed_str()) })
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            );
            let root_markers: &'static [&'static str] = Box::leak(
                c.root_markers()
                    .iter()
                    .map(|m| -> &'static str { Box::leak(m.clone().into_boxed_str()) })
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            );
            ServerConfig {
                id,
                extensions,
                command,
                args,
                root_markers,
                language_id,
                initialization_options: None,
                auto_install: AutoInstall::None,
            }
        })
        .collect()
}
