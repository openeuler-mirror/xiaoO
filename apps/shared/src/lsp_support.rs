// 配置词汇再导出：应用层（endside support/config、serverside daemon_config）
// 需要命名这些类型以 serde 反序列化与持有 LSP 注册表句柄，无法下沉为粗粒度
// 函数，故作为配置词汇再导出。`AutoInstall` / `ServerConfig` 在尚未提供
// 粗粒度 lsp builder 之前仍需被应用直接构造；builder 落地后可吸收这两个。
// 本模块内 `build_extra_server_configs` 亦使用 `ServerConfig` / `AutoInstall`，
// 直接复用此 `pub use`，无需重复私有 import。
pub use lsp::{AutoInstall, LspServiceRegistry, ServerConfig};

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
