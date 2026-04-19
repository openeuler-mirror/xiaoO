use agent_contracts::backend::{
    capability::{
        exec::{ExecRequest, ExecResult},
        export::ExportFileRequest,
        filesystem::{
            ReadBytesRequest, TempPathKind, TempPathRequest, WriteBytesRequest, WriteMode,
        },
        path::{ResolveBase, ResolvePathRequest},
        search::{GlobRequest, GrepMode, GrepRequest},
    },
    BackendPath, ExportedFile, ExportedFileSource, OperationBackendBuilder, OperationBackendConfig,
    OperationError, PathKind, PathStat,
};
use operation_backend::OperationBackendBuilderImpl;
use serde::Deserialize;
use serde_json::{json, Value};

pub(crate) fn run_from_env() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let parsed = parse_cli_args(args.as_slice())?;
    let config = load_backend_config(parsed.config_path.as_str())?;
    let request = serde_json::from_str::<BackendCliRequest>(parsed.request_json.as_str())
        .map_err(|error| format!("invalid request json: {error}"))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("failed to build tokio runtime: {error}"))?;

    runtime.block_on(async move {
        let builder = OperationBackendBuilderImpl::new();
        let backend = builder
            .build(&config)
            .await
            .map_err(|error| format!("backend build failed: {error}"))?;
        let request_result = handle_request(backend.as_ref(), request).await;
        let shutdown_result = backend.shutdown().await;

        let output = request_result.map_err(|error| format!("request failed: {error}"))?;
        shutdown_result.map_err(|error| format!("backend shutdown failed: {error}"))?;
        println!(
            "{}",
            serde_json::to_string(&output).map_err(|error| error.to_string())?
        );
        Ok(())
    })
}

struct ParsedCli {
    config_path: String,
    request_json: String,
}

fn parse_cli_args(args: &[String]) -> Result<ParsedCli, String> {
    if args.len() != 5 {
        return Err("usage: xiaoo-backend-cli --config <path> request --json <json>".to_string());
    }
    if args[0] != "--config" {
        return Err("missing --config".to_string());
    }
    if args[2] != "request" {
        return Err("only the 'request' subcommand is currently supported".to_string());
    }
    if args[3] != "--json" {
        return Err("missing --json".to_string());
    }
    if args[4].is_empty() {
        return Err("missing request json".to_string());
    }

    Ok(ParsedCli {
        config_path: args[1].clone(),
        request_json: args[4].clone(),
    })
}

fn load_backend_config(path: &str) -> Result<OperationBackendConfig, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read config file: {error}"))?;
    let stripped = strip_jsonc_comments(raw.as_str());
    let file_config = serde_json::from_str::<BackendConfigFile>(stripped.as_str())
        .map_err(|error| format!("invalid config json: {error}"))?;
    Ok(OperationBackendConfig::new(
        file_config.backend.kind,
        file_config.backend.options,
    ))
}

async fn handle_request(
    backend: &dyn agent_contracts::backend::OperationBackend,
    request: BackendCliRequest,
) -> Result<Value, OperationError> {
    match request {
        BackendCliRequest::ResolvePath { raw_path, base } => {
            let resolved = backend
                .paths()
                .resolve_path(ResolvePathRequest {
                    raw_path,
                    base: base.into_contract(),
                })
                .await?;
            Ok(json!({ "path": resolved.0 }))
        }
        BackendCliRequest::Stat { path } => {
            let backend_path = BackendPath(path.clone());
            let stat = backend.files().stat(&backend_path).await?;
            Ok(path_stat_json(path.as_str(), &stat))
        }
        BackendCliRequest::ReadBytes { path } => {
            let bytes = backend
                .files()
                .read_bytes(ReadBytesRequest {
                    path: BackendPath(path.clone()),
                })
                .await?;
            Ok(json!({
                "path": path,
                "text": String::from_utf8_lossy(bytes.as_slice()).to_string()
            }))
        }
        BackendCliRequest::WriteBytes {
            path,
            content,
            mode,
        } => {
            let outcome = backend
                .files()
                .write_bytes(WriteBytesRequest {
                    path: BackendPath(path),
                    content: content.into_bytes(),
                    mode: mode.into_contract(),
                })
                .await?;
            Ok(json!({
                "path": outcome.path.0,
                "created": outcome.created
            }))
        }
        BackendCliRequest::CreateDirAll { path } => {
            let backend_path = BackendPath(path.clone());
            backend.files().create_dir_all(&backend_path).await?;
            Ok(json!({ "path": path, "created": true }))
        }
        BackendCliRequest::Glob {
            pattern,
            base_dir,
            limit,
        } => {
            let entries = backend
                .search()
                .glob(GlobRequest {
                    pattern,
                    base_dir: Some(BackendPath(base_dir)),
                    limit,
                })
                .await?;
            Ok(json!({
                "entries": entries.into_iter().map(|entry| entry.0).collect::<Vec<_>>()
            }))
        }
        BackendCliRequest::Grep {
            query,
            base_dir,
            include,
            mode,
            head_limit,
        } => {
            let result = backend
                .search()
                .grep(GrepRequest {
                    query,
                    base_dir: BackendPath(base_dir),
                    include,
                    mode: mode.into_contract(),
                    head_limit,
                })
                .await?;
            Ok(json!({ "entries": result.entries }))
        }
        BackendCliRequest::TempPath {
            kind,
            preferred_parent,
            prefix,
            suffix,
        } => {
            let path = backend
                .files()
                .temp_path(TempPathRequest {
                    kind: kind.into_contract(),
                    preferred_parent: preferred_parent.map(BackendPath),
                    prefix,
                    suffix,
                })
                .await?;
            Ok(json!({ "path": path.0 }))
        }
        BackendCliRequest::ExportFile {
            path,
            preferred_name,
        } => {
            let exported = backend
                .export()
                .export_file(ExportFileRequest {
                    path: BackendPath(path),
                    preferred_name,
                })
                .await?;
            Ok(exported_file_json(&exported))
        }
        BackendCliRequest::Exec {
            command,
            args,
            shell,
            cwd,
            timeout_ms,
        } => {
            let result = backend
                .exec()
                .exec(ExecRequest {
                    command,
                    args,
                    shell,
                    cwd: cwd.map(BackendPath),
                    timeout_ms,
                })
                .await?;
            Ok(exec_result_json(&result))
        }
    }
}

fn path_stat_json(path: &str, stat: &PathStat) -> Value {
    json!({
        "path": path,
        "exists": stat.exists,
        "kind": stat.kind.map(path_kind_name),
        "size_bytes": stat.size_bytes,
        "modified_at_ms": stat.modified_at.map(|value| {
            value.duration_since(std::time::SystemTime::UNIX_EPOCH).map(|duration| duration.as_millis()).unwrap_or(0)
        })
    })
}

fn exported_file_json(file: &ExportedFile) -> Value {
    let source = match &file.source {
        ExportedFileSource::HostPath(path) => json!({
            "kind": "HostPath",
            "path": path.to_string_lossy().to_string()
        }),
        ExportedFileSource::Bytes(bytes) => json!({
            "kind": "Bytes",
            "size_bytes": bytes.len()
        }),
    };
    json!({
        "file_name": file.file_name,
        "size_bytes": file.size_bytes,
        "media_type": file.media_type,
        "source": source
    })
}

fn exec_result_json(result: &ExecResult) -> Value {
    json!({
        "stdout": String::from_utf8_lossy(result.stdout.as_slice()).to_string(),
        "stderr": String::from_utf8_lossy(result.stderr.as_slice()).to_string(),
        "exit_code": result.exit_code,
        "timed_out": result.timed_out
    })
}

fn path_kind_name(kind: PathKind) -> &'static str {
    match kind {
        PathKind::File => "file",
        PathKind::Directory => "directory",
        PathKind::Symlink => "symlink",
        PathKind::Other => "other",
    }
}

fn strip_jsonc_comments(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    let mut line_comment = false;
    let mut block_comment = false;

    while let Some(ch) = chars.next() {
        if line_comment {
            if ch == '\n' {
                line_comment = false;
                output.push(ch);
            }
            continue;
        }
        if block_comment {
            if ch == '*' && chars.peek() == Some(&'/') {
                chars.next();
                block_comment = false;
            }
            continue;
        }
        if in_string {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            output.push(ch);
            continue;
        }
        if ch == '/' {
            match chars.peek() {
                Some('/') => {
                    chars.next();
                    line_comment = true;
                    continue;
                }
                Some('*') => {
                    chars.next();
                    block_comment = true;
                    continue;
                }
                _ => {}
            }
        }
        output.push(ch);
    }

    output
}

#[derive(Debug, Deserialize)]
struct BackendConfigFile {
    backend: BackendConfigEntry,
}

#[derive(Debug, Deserialize)]
struct BackendConfigEntry {
    kind: String,
    options: Value,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", content = "args")]
enum BackendCliRequest {
    #[serde(rename = "resolve_path")]
    ResolvePath {
        raw_path: String,
        base: ResolveBaseInput,
    },
    #[serde(rename = "stat")]
    Stat { path: String },
    #[serde(rename = "read_bytes")]
    ReadBytes { path: String },
    #[serde(rename = "write_bytes")]
    WriteBytes {
        path: String,
        content: String,
        mode: WriteModeInput,
    },
    #[serde(rename = "create_dir_all")]
    CreateDirAll { path: String },
    #[serde(rename = "glob")]
    Glob {
        pattern: String,
        base_dir: String,
        limit: Option<usize>,
    },
    #[serde(rename = "grep")]
    Grep {
        query: String,
        base_dir: String,
        include: Option<String>,
        mode: GrepModeInput,
        head_limit: Option<usize>,
    },
    #[serde(rename = "temp_path")]
    TempPath {
        kind: TempPathKindInput,
        preferred_parent: Option<String>,
        prefix: Option<String>,
        suffix: Option<String>,
    },
    #[serde(rename = "export_file")]
    ExportFile {
        path: String,
        preferred_name: Option<String>,
    },
    #[serde(rename = "exec")]
    Exec {
        command: String,
        args: Vec<String>,
        shell: Option<String>,
        cwd: Option<String>,
        timeout_ms: Option<u64>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ResolveBaseInput {
    WorkspaceRoot,
    HomeDir,
    Explicit(String),
}

impl ResolveBaseInput {
    fn into_contract(self) -> ResolveBase {
        match self {
            ResolveBaseInput::WorkspaceRoot => ResolveBase::WorkspaceRoot,
            ResolveBaseInput::HomeDir => ResolveBase::HomeDir,
            ResolveBaseInput::Explicit(path) => ResolveBase::Explicit(BackendPath(path)),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WriteModeInput {
    Create,
    Overwrite,
    AtomicOverwrite,
}

impl WriteModeInput {
    fn into_contract(self) -> WriteMode {
        match self {
            WriteModeInput::Create => WriteMode::Create,
            WriteModeInput::Overwrite => WriteMode::Overwrite,
            WriteModeInput::AtomicOverwrite => WriteMode::AtomicOverwrite,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TempPathKindInput {
    File,
    Directory,
}

impl TempPathKindInput {
    fn into_contract(self) -> TempPathKind {
        match self {
            TempPathKindInput::File => TempPathKind::File,
            TempPathKindInput::Directory => TempPathKind::Directory,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum GrepModeInput {
    FilesWithMatches,
    Content,
    Count,
}

impl GrepModeInput {
    fn into_contract(self) -> GrepMode {
        match self {
            GrepModeInput::FilesWithMatches => GrepMode::FilesWithMatches,
            GrepModeInput::Content => GrepMode::Content,
            GrepModeInput::Count => GrepMode::Count,
        }
    }
}
