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
    BackendPath, ExportedFileHandle, OperationBackendBuildInput, OperationBackendBuilder,
    OperationBackendConfig, OperationError, PathKind, PathStat,
};
use operation_backend::OperationBackendBuilderImpl;
use serde::Deserialize;
use serde_json::{json, Value};

/// How the CLI received its work. The classic mode keeps
/// `--config <path> request --json <json>`; the envelope mode reads
/// `request --stdin` and carries the backend config plus the request in one
/// JSON document, so the request body never touches argv (no
/// `MAX_ARG_STRLEN` limit) and arbitrary bytes can pass through base64.
enum Invocation {
    ConfigFile {
        config_path: String,
        request_json: String,
    },
    StdinEnvelope,
}

pub(crate) fn run_from_env() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let invocation = parse_cli_args(args.as_slice())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("failed to build tokio runtime: {error}"))?;

    let (config, request) = match invocation {
        Invocation::ConfigFile {
            config_path,
            request_json,
        } => {
            let config = load_backend_config(config_path.as_str())?;
            let request = serde_json::from_str::<BackendCliRequest>(request_json.as_str())
                .map_err(|error| format!("invalid request json: {error}"))?;
            (config, request)
        }
        Invocation::StdinEnvelope => {
            let envelope_raw = read_stdin_to_string()?;
            let envelope = serde_json::from_str::<RequestEnvelope>(envelope_raw.as_str())
                .map_err(|error| format!("invalid request envelope: {error}"))?;
            (
                OperationBackendConfig::new(envelope.backend.kind, envelope.backend.options),
                envelope.request,
            )
        }
    };

    runtime.block_on(async move {
        let output = execute_request(config, request).await?;
        println!(
            "{}",
            serde_json::to_string(&output).map_err(|error| error.to_string())?
        );
        Ok(())
    })
}

/// Run one request against a freshly built backend and return its output.
/// Printing is left to the caller so tests can assert on the output value.
async fn execute_request(
    config: OperationBackendConfig,
    request: BackendCliRequest,
) -> Result<Value, String> {
    let builder = OperationBackendBuilderImpl::new();
    let backend = builder
        .build(&OperationBackendBuildInput {
            config: Some(config),
            ..OperationBackendBuildInput::default()
        })
        .await
        .map_err(|error| format!("backend build failed: {error}"))?;
    let request_result = handle_request(backend.as_ref(), request).await;
    let shutdown_result = backend.shutdown().await;

    let output = request_result.map_err(|error| format!("request failed: {error}"))?;
    shutdown_result.map_err(|error| format!("backend shutdown failed: {error}"))?;
    Ok(output)
}

/// Upper bound for the stdin envelope document (64 MiB). The envelope
/// channel exists to carry payloads past the kernel argv limit, but a
/// runaway upstream pipe must fail with a clear error instead of
/// exhausting memory.
const MAX_ENVELOPE_BYTES: usize = 64 * 1024 * 1024;

fn read_stdin_to_string() -> Result<String, String> {
    read_envelope_limited(std::io::stdin(), MAX_ENVELOPE_BYTES)
}

/// Read one envelope document from `reader`, rejecting documents past
/// `max_bytes` and non-UTF-8 input with dedicated errors so a bad upstream
/// gets a precise message instead of a JSON parser failure.
fn read_envelope_limited<R: std::io::Read>(reader: R, max_bytes: usize) -> Result<String, String> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    // Read at most one byte beyond the cap: the extra byte only signals
    // that the document is oversized.
    reader
        .take((max_bytes + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read request envelope from stdin: {error}"))?;
    if bytes.len() > max_bytes {
        return Err(format!(
            "request envelope exceeds the {max_bytes}-byte stdin limit"
        ));
    }
    String::from_utf8(bytes)
        .map_err(|error| format!("request envelope from stdin is not valid UTF-8: {error}"))
}

fn parse_cli_args(args: &[String]) -> Result<Invocation, String> {
    if args.len() >= 2 && args[0] == "request" && args[1] == "--stdin" {
        if args.len() != 2 {
            return Err(
                "request --stdin takes no additional arguments (the envelope is read from stdin)"
                    .to_string(),
            );
        }
        return Ok(Invocation::StdinEnvelope);
    }
    if args.len() != 5 {
        return Err(
            "usage: xiaoo-backend-cli --config <path> request --json <json> | request --stdin"
                .to_string(),
        );
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

    Ok(Invocation::ConfigFile {
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

/// Envelope for the `request --stdin` mode: one JSON document carrying the
/// backend section (same shape as the `--config` file's `backend` entry)
/// plus the request itself (same shape as the `--json` payload).
///
/// The CLI is a leaf executor: the request shape structurally carries no
/// plugin `extra` channel, so mutation externalization can never recurse
/// through it.
#[derive(Debug, Deserialize)]
struct RequestEnvelope {
    backend: BackendConfigEntry,
    request: BackendCliRequest,
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
            Ok(json!({ "path": resolved.native().to_string() }))
        }
        BackendCliRequest::Stat { path } => {
            let backend_path = BackendPath::from_raw(path.clone());
            let stat = backend.files().stat(&backend_path).await?;
            Ok(path_stat_json(path.as_str(), &stat))
        }
        BackendCliRequest::ReadBytes { path } => {
            use base64::Engine as _;
            let bytes = backend
                .files()
                .read_bytes(ReadBytesRequest {
                    path: BackendPath::from_raw(path.clone()),
                })
                .await?;
            Ok(json!({
                "path": path,
                "text": String::from_utf8_lossy(bytes.as_slice()).to_string(),
                "content_b64": base64::engine::general_purpose::STANDARD.encode(bytes.as_slice()),
            }))
        }
        BackendCliRequest::WriteBytes {
            path,
            content,
            content_b64,
            mode,
        } => {
            let content = decode_write_content(content, content_b64)
                .map_err(|message| OperationError::ExecutionFailed { message })?;
            let outcome = backend
                .files()
                .write_bytes(WriteBytesRequest {
                    path: BackendPath::from_raw(path),
                    content,
                    mode: mode.into_contract(),
                    // The CLI is a leaf executor: requests never carry the
                    // plugin extra channel, so any backend-side mutation
                    // externalization cannot recurse through it.
                    extra: None,
                })
                .await?;
            Ok(json!({
                "path": outcome.path.native().to_string(),
                "created": outcome.created
            }))
        }
        BackendCliRequest::CreateDirAll { path } => {
            let backend_path = BackendPath::from_raw(path.clone());
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
                    base_dir: Some(BackendPath::from_raw(base_dir)),
                    limit,
                })
                .await?;
            Ok(json!({
                "entries": entries.into_iter().map(|entry| entry.native().to_string()).collect::<Vec<_>>()
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
                    base_dir: BackendPath::from_raw(base_dir),
                    include,
                    mode: mode.into_contract(),
                    head_limit,
                })
                .await?;
            Ok(json!({
                "entries": result.entries,
            }))
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
                    preferred_parent: preferred_parent.map(BackendPath::from_raw),
                    prefix,
                    suffix,
                })
                .await?;
            Ok(json!({ "path": path.native() }))
        }
        BackendCliRequest::ExportFile {
            path,
            preferred_name,
        } => {
            let exported = backend
                .export()
                .export_file(ExportFileRequest {
                    path: BackendPath::from_raw(path),
                    preferred_name,
                })
                .await?;
            Ok(exported_file_json(exported.as_ref()))
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
                    cwd: cwd.map(BackendPath::from_raw),
                    timeout_ms,
                    ..Default::default()
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

fn exported_file_json(file: &dyn ExportedFileHandle) -> Value {
    let metadata = file.metadata();
    json!({
        "file_name": metadata.file_name,
        "size_bytes": metadata.size_bytes,
        "media_type": metadata.media_type,
        "reader": { "kind": "opaque" }
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

/// Resolve the write_bytes content: exactly one of the plain-UTF-8 `content`
/// and the base64 `content_b64` fields must be present. The base64 channel
/// keeps the write_bytes API byte-exact (the plain field is a String and
/// cannot carry arbitrary bytes).
fn decode_write_content(
    content: Option<String>,
    content_b64: Option<String>,
) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    match (content, content_b64) {
        (Some(text), None) => Ok(text.into_bytes()),
        (None, Some(encoded)) => base64::engine::general_purpose::STANDARD
            .decode(encoded.as_str())
            .map_err(|error| format!("invalid content_b64: {error}")),
        (Some(_), Some(_)) => {
            Err("write_bytes: content and content_b64 are mutually exclusive".to_string())
        }
        (None, None) => Err("write_bytes: one of content or content_b64 is required".to_string()),
    }
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
        /// Plain UTF-8 content (classic `--json` usage).
        #[serde(default)]
        content: Option<String>,
        /// Binary-safe content, standard base64. Exactly one of
        /// `content` / `content_b64` must be present.
        content_b64: Option<String>,
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
        #[serde(alias = "pattern")]
        query: String,
        #[serde(alias = "target")]
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
            ResolveBaseInput::Explicit(path) => ResolveBase::Explicit(BackendPath::from_raw(path)),
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

#[cfg(test)]
#[path = "../../../../../tests/unit/operation_backend/bin/backend_cli/app_test.rs"]
mod tests;
