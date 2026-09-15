use super::super::manifest::{DeclarativeToolManifest, EffectSection, ExecSection};
use super::*;

fn test_executor(tool_dir: PathBuf) -> DeclarativeToolExecutor {
    let loaded = LoadedDeclarativeTool {
        manifest_path: tool_dir.join("test_tool.toml"),
        tool_dir,
        manifest: DeclarativeToolManifest {
            name: "test_tool".to_string(),
            description: "test tool".to_string(),
            timeout_ms: 5000,
            output: None,
            effect: EffectSection::default(),
            input_schema: toml::Value::Table(toml::map::Map::new()),
            exec: ExecSection {
                command: "sh".to_string(),
                args: Vec::new(),
                stdin: StdinMode::Json,
                stdout: StdoutMode::Text,
                env: Vec::new(),
            },
        },
        input_schema_json: json!({}),
    };
    let spec = Arc::new(DeclarativeToolSpec::from_loaded_tool(&loaded));
    DeclarativeToolExecutor::from_loaded_tool(spec, &loaded)
}

#[test]
fn expands_tilde_for_explicit_tool_paths() {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let executor = test_executor(PathBuf::from("/workspace/.xiaoo/tools"));
    let resolved = executor.resolve_arg_token("~/.xiaoo/tools/md_to_html.mjs");
    assert_eq!(
        resolved,
        PathBuf::from(home)
            .join(".xiaoo/tools/md_to_html.mjs")
            .to_string_lossy()
            .into_owned()
    );
}

#[test]
fn resolves_dot_relative_tool_paths_from_manifest_dir() {
    let executor = test_executor(PathBuf::from("/home/user/.xiaoo/tools"));

    assert_eq!(
        executor.resolve_arg_token("./md_to_html.mjs"),
        PathBuf::from("/home/user/.xiaoo/tools")
            .join("./md_to_html.mjs")
            .to_string_lossy()
            .into_owned()
    );
    assert_eq!(
        executor.resolve_command_token("./runner.sh"),
        PathBuf::from("/home/user/.xiaoo/tools")
            .join("./runner.sh")
            .to_string_lossy()
            .into_owned()
    );
}

#[test]
fn keeps_workspace_relative_args_unchanged() {
    let executor = test_executor(PathBuf::from("/home/user/.xiaoo/tools"));

    assert_eq!(
        executor.resolve_arg_token(".xiaoo/tools/echo_payload.sh"),
        ".xiaoo/tools/echo_payload.sh"
    );
}
