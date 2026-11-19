use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use agent_contracts::backend::{
    capability::{
        exec::{ExecOutput, ExecRequest, ExecResult, LineSink, OperationExec},
        export::{ExportFileRequest, OperationExport},
        filesystem::{
            OperationFileSystem, ReadBytesRequest, TempPathRequest, WriteBytesOutcome,
            WriteBytesRequest, WriteMode,
        },
        path::{OperationPathResolver, ResolveBase, ResolvePathRequest},
        search::{GlobRequest, GrepMode, GrepRequest, GrepResult, OperationSearch},
    },
    BackendPath, OperationBackend, OperationBackendCapabilities, OperationError, PathKind,
    PathNamespace, PathStat, SharedExportedFileHandle,
};
use async_trait::async_trait;
use glob::Pattern;

const MAX_TEXT_BYTES: usize = 1024 * 1024;
const STAT: &str = r#"
out=$(LC_ALL=C stat --dereference --format='%f %s %Y %y' -- "$1" 2>&1)
status=$?
if (( status == 0 )); then
    printf '%s\n' "$out"
elif [[ $out == *': No such file or directory' ]]; then
    exit 44
else
    exit 1
fi
"#;

const GLOB_FILTER: &str = r#"
set -o pipefail
export LC_ALL=C
root=$1 pattern=$2
shift 2
for path; do
    relative=${path#./}
    printf '%s\0' "$relative" "$root/$relative"
done |
{ grep -Pz -- "$pattern" || [[ $? == 1 ]]; } |
while IFS= read -r -d '' match; do
    printf './%s\0' "${match#"$root"/}"
done | sort -zu
"#;

#[derive(Clone, Copy, Default)]
pub struct ClientOperationCapabilities {
    pub read: bool,
    pub write: bool,
    pub terminal: bool,
}

impl ClientOperationCapabilities {
    pub fn can_read(self) -> bool {
        self.terminal && self.read
    }

    pub fn can_write(self) -> bool {
        self.can_read() && self.write
    }
}

pub struct ClientCommand {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub timeout: Duration,
    pub output_byte_limit: u64,
}

/// Requests within an existing, authorized client session and prompt.
#[async_trait]
pub trait ClientOperations: Send + Sync {
    async fn read_text(&self, path: &str) -> Result<String, OperationError>;
    async fn write_text(&self, path: &str, content: String) -> Result<(), OperationError>;
    async fn run_terminal(&self, command: ClientCommand) -> Result<ExecResult, OperationError>;
}

pub struct ClientBackend {
    id: String,
    root: BackendPath,
    capabilities: ClientOperationCapabilities,
    operations: Arc<dyn ClientOperations>,
}

impl ClientBackend {
    pub fn new(
        id: String,
        cwd: &Path,
        capabilities: ClientOperationCapabilities,
        operations: Arc<dyn ClientOperations>,
    ) -> Result<Self, OperationError> {
        let root = cwd
            .to_str()
            .filter(|path| !path.contains('\0') && cwd.is_absolute())
            .ok_or_else(invalid_path)?;
        let root = normalize(Path::new(root))?;
        let root = BackendPath::new(&id, PathNamespace::Workspace, utf8(&root)?, "");
        Ok(Self {
            id,
            root,
            capabilities,
            operations,
        })
    }

    fn resolve(&self, path: &Path) -> Result<BackendPath, OperationError> {
        let root = Path::new(self.root.native());
        // An absolute path must enter the workspace at its prefix and never
        // walk above it, even if it would return to the root later.
        let relative = path.strip_prefix(root).map_err(|_| invalid_path())?;
        let normalized = normalize(relative)?;
        Ok(self
            .root
            .with_native(utf8(&root.join(&normalized))?, utf8(&normalized)?))
    }

    fn check_path(&self, path: &BackendPath) -> Result<BackendPath, OperationError> {
        path.assert_owned_by(&self.id)?;
        self.resolve(Path::new(path.native()))
    }

    async fn query(
        &self,
        cwd: &BackendPath,
        command: &str,
        args: Vec<String>,
        limit: u64,
    ) -> Result<ExecResult, OperationError> {
        let result = self
            .terminal(
                cwd,
                command.to_owned(),
                args,
                Duration::from_secs(20),
                limit,
            )
            .await?;
        if result.timed_out || matches!(result.output, ExecOutput::Combined { truncated: true }) {
            return Err(failed("client metadata or search output was incomplete"));
        }
        Ok(result)
    }

    async fn terminal(
        &self,
        cwd: &BackendPath,
        command: String,
        args: Vec<String>,
        timeout: Duration,
        output_byte_limit: u64,
    ) -> Result<ExecResult, OperationError> {
        if !self.capabilities.terminal {
            return Err(unsupported("client terminal capability is required"));
        }
        let cwd = self.check_path(cwd)?.native().to_owned();
        if command.contains('\0') || args.iter().any(|arg| arg.contains('\0')) {
            return Err(failed("command arguments contain NUL"));
        }
        self.operations
            .run_terminal(ClientCommand {
                command,
                args,
                cwd,
                timeout,
                output_byte_limit,
            })
            .await
    }

    async fn find(
        &self,
        cwd: &BackendPath,
        filters: Vec<String>,
    ) -> Result<Vec<BackendPath>, OperationError> {
        let mut args = strings(&[".", "-mindepth", "1"]);
        args.extend(filters);
        let result = self.query(cwd, "find", args, 64 * 1024).await?;
        let output = command_output(result, &[0])?;
        if !output.is_empty() && !output.ends_with('\0') {
            return Err(failed("invalid client path listing"));
        }
        let mut entries = output
            .split_terminator('\0')
            .map(|path| {
                let relative = path.strip_prefix("./").ok_or_else(invalid_path)?;
                let relative = Path::new(relative);
                if relative.is_absolute() {
                    return Err(invalid_path());
                }
                self.resolve(&Path::new(cwd.native()).join(normalize(relative)?))
            })
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort_by(|a, b| a.native().cmp(b.native()));
        entries.dedup();
        Ok(entries)
    }
}

fn normalize(path: &Path) -> Result<PathBuf, OperationError> {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::RootDir | Component::Normal(_) => result.push(part.as_os_str()),
            Component::CurDir => (),
            Component::ParentDir if result.pop() => (),
            _ => return Err(invalid_path()),
        }
    }
    Ok(result)
}

fn utf8(path: &Path) -> Result<&str, OperationError> {
    path.to_str()
        .filter(|value| !value.contains('\0'))
        .ok_or_else(invalid_path)
}

fn invalid_path() -> OperationError {
    OperationError::InvalidPath {
        message: "path must stay within the client workspace".into(),
    }
}

fn failed(message: &str) -> OperationError {
    OperationError::ExecutionFailed {
        message: message.into(),
    }
}

fn unsupported(message: &str) -> OperationError {
    OperationError::Unsupported {
        message: message.into(),
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn command_output(result: ExecResult, expected_codes: &[i32]) -> Result<String, OperationError> {
    if !result
        .exit_code
        .is_some_and(|code| expected_codes.contains(&code))
    {
        return Err(failed("client filesystem command failed"));
    }
    String::from_utf8(result.stdout).map_err(|_| failed("client output must be UTF-8"))
}

#[async_trait]
impl OperationBackend for ClientBackend {
    fn backend_id(&self) -> &str {
        &self.id
    }

    fn capabilities(&self) -> OperationBackendCapabilities {
        OperationBackendCapabilities {
            supports_atomic_write: false,
            supports_export_file: false,
            supports_grep: self.capabilities.terminal,
            supports_lsp: false,
        }
    }

    fn requires_text_files(&self) -> bool {
        true
    }
    fn paths(&self) -> &dyn OperationPathResolver {
        self
    }
    fn files(&self) -> &dyn OperationFileSystem {
        self
    }
    fn search(&self) -> &dyn OperationSearch {
        self
    }
    fn exec(&self) -> &dyn OperationExec {
        self
    }
    fn export(&self) -> &dyn OperationExport {
        self
    }

    async fn shutdown(&self) -> Result<(), OperationError> {
        Ok(())
    }
}

#[async_trait]
impl OperationPathResolver for ClientBackend {
    fn workspace_root(&self) -> &BackendPath {
        &self.root
    }
    fn home_dir(&self) -> Option<&BackendPath> {
        None
    }

    async fn resolve_path(
        &self,
        request: ResolvePathRequest,
    ) -> Result<BackendPath, OperationError> {
        let raw = Path::new(&request.raw_path);
        if request.raw_path.starts_with('~') {
            return Err(invalid_path());
        }
        let base = match request.base {
            ResolveBase::WorkspaceRoot => self.root.clone(),
            ResolveBase::Explicit(path) => self.check_path(&path)?,
            ResolveBase::HomeDir => return Err(invalid_path()),
        };
        self.resolve(&Path::new(base.native()).join(raw))
    }
}

fn parse_stat(text: &str) -> Option<PathStat> {
    let mut fields = text.trim_end().splitn(4, ' ');
    let mode = u32::from_str_radix(fields.next()?, 16).ok()?;
    let size = fields.next()?.parse().ok()?;
    let seconds: i64 = fields.next()?.parse().ok()?;
    let (_, fraction) = fields.next()?.split_once('.')?;
    let (nanos, _) = fraction.split_once(' ')?;
    let nanos: u32 = nanos.parse().ok()?;
    if nanos >= 1_000_000_000 {
        return None;
    }
    let time = if seconds >= 0 {
        UNIX_EPOCH.checked_add(Duration::from_secs(seconds as u64))?
    } else {
        UNIX_EPOCH.checked_sub(Duration::from_secs(seconds.unsigned_abs()))?
    };
    let kind = match mode & 0o170000 {
        0o100000 => PathKind::File,
        0o040000 => PathKind::Directory,
        0o120000 => PathKind::Symlink,
        _ => PathKind::Other,
    };
    Some(PathStat {
        exists: true,
        kind: Some(kind),
        size_bytes: Some(size),
        modified_at: Some(time.checked_add(Duration::from_nanos(nanos.into()))?),
    })
}

#[async_trait]
impl OperationFileSystem for ClientBackend {
    async fn stat(&self, path: &BackendPath) -> Result<PathStat, OperationError> {
        let path = self.check_path(path)?;
        let result = self
            .query(
                &self.root,
                "bash",
                strings(&["-c", STAT, "stat", path.native()]),
                512,
            )
            .await?;
        if result.exit_code == Some(44) {
            return Ok(PathStat {
                exists: false,
                kind: None,
                size_bytes: None,
                modified_at: None,
            });
        }
        let output = command_output(result, &[0])?;
        parse_stat(&output).ok_or_else(|| failed("invalid client stat output"))
    }

    async fn read_bytes(&self, request: ReadBytesRequest) -> Result<Vec<u8>, OperationError> {
        if !self.capabilities.can_read() {
            return Err(unsupported(
                "client read and terminal capabilities are required",
            ));
        }
        let path = self.check_path(&request.path)?;
        let stat = self.stat(&path).await?;
        if stat
            .size_bytes
            .is_some_and(|size| size > MAX_TEXT_BYTES as u64)
        {
            return Err(unsupported("client file must be UTF-8 text within 1 MiB"));
        }
        let text = self.operations.read_text(path.native()).await?;
        if text.len() > MAX_TEXT_BYTES || text.contains('\0') {
            return Err(unsupported("client file must be UTF-8 text within 1 MiB"));
        }
        Ok(text.into_bytes())
    }

    async fn write_bytes(
        &self,
        request: WriteBytesRequest,
    ) -> Result<WriteBytesOutcome, OperationError> {
        if !self.capabilities.can_write() {
            return Err(unsupported(
                "client write capability and exclusive access are required",
            ));
        }
        if request.mode == WriteMode::AtomicOverwrite || request.extra.is_some() {
            return Err(unsupported("write mode or plugin data are unsupported"));
        }
        let path = self.check_path(&request.path)?;
        let text = String::from_utf8(request.content)
            .map_err(|_| unsupported("client file must be UTF-8 text"))?;
        if text.len() > MAX_TEXT_BYTES || text.contains('\0') {
            return Err(unsupported("client file must be UTF-8 text within 1 MiB"));
        }
        let created = !self.stat(&path).await?.exists;
        if !created && request.mode == WriteMode::Create {
            return Err(OperationError::AlreadyExists {
                path: path.native().into(),
            });
        }
        self.operations.write_text(path.native(), text).await?;
        Ok(WriteBytesOutcome { path, created })
    }

    async fn create_dir_all(&self, path: &BackendPath) -> Result<(), OperationError> {
        if !self.capabilities.can_write() {
            return Err(unsupported(
                "client write capability and exclusive access are required",
            ));
        }
        let path = self.check_path(path)?;
        let result = self
            .query(
                &self.root,
                "mkdir",
                strings(&["-p", "--", path.native()]),
                512,
            )
            .await?;
        command_output(result, &[0])?;
        Ok(())
    }

    async fn create_dir_all_with_extra(
        &self,
        path: &BackendPath,
        extra: Option<&serde_json::Value>,
    ) -> Result<(), OperationError> {
        if extra.is_some() {
            return Err(unsupported("plugin data are unsupported"));
        }
        self.create_dir_all(path).await
    }

    async fn temp_path(&self, _: TempPathRequest) -> Result<BackendPath, OperationError> {
        Err(unsupported(
            "temporary files are unavailable in client sessions",
        ))
    }
}

#[async_trait]
impl OperationExec for ClientBackend {
    fn default_shell(&self) -> Option<&str> {
        Some("bash")
    }

    async fn exec(&self, request: ExecRequest) -> Result<ExecResult, OperationError> {
        if request.env.is_some() || request.extra.is_some() {
            return Err(unsupported(
                "environment overrides and plugin data are unsupported",
            ));
        }
        let (command, args) = match request.shell.as_deref() {
            Some("bash") if request.args.is_empty() => {
                ("bash".into(), vec!["-c".into(), request.command])
            }
            None if request.command == "rg" => {
                // The shell preserves each argv element and reports a missing
                // executable as exit 127, which permits the GNU grep fallback.
                let mut args = strings(&["-c", "exec \"$@\"", "rg", "rg"]);
                args.extend(request.args);
                ("bash".into(), args)
            }
            None => (request.command, request.args),
            _ => {
                return Err(unsupported(
                    "only bash shell commands and argv are supported",
                ))
            }
        };
        self.terminal(
            request.cwd.as_ref().unwrap_or(&self.root),
            command,
            args,
            Duration::from_millis(request.timeout_ms.unwrap_or(120_000).min(600_000)),
            16 * 1024,
        )
        .await
    }

    async fn exec_streaming(
        &self,
        request: ExecRequest,
        sink: Arc<dyn LineSink>,
    ) -> Result<ExecResult, OperationError> {
        let mut result = OperationExec::exec(self, request).await?;
        let stdout = std::mem::take(&mut result.stdout);
        result.stopped_early = false;
        // A buffered terminal has already exited. Only a successful search
        // may be paged, so the sink cannot mask a client command failure.
        if result.timed_out || !matches!(result.exit_code, Some(0 | 1)) {
            return Ok(result);
        }
        if matches!(result.output, ExecOutput::Combined { truncated: true }) {
            return Err(failed(
                "client search output was truncated; narrow the search",
            ));
        }
        result.stopped_early = String::from_utf8_lossy(&stdout)
            .lines()
            .any(|line| !sink.on_line(line));
        Ok(result)
    }
}

#[async_trait]
impl OperationSearch for ClientBackend {
    async fn glob(&self, request: GlobRequest) -> Result<Vec<BackendPath>, OperationError> {
        let base = request.base_dir.unwrap_or_else(|| self.root.clone());
        let pattern = Pattern::new(&request.pattern).map_err(|_| failed("invalid glob pattern"))?;
        let base = self.check_path(&base)?;
        let prefilter = glob_prefilter(pattern.as_str());
        let filters = strings(&[
            "-exec",
            "bash",
            "-c",
            GLOB_FILTER,
            "glob",
            base.native().trim_end_matches('/'),
            &prefilter,
            "{}",
            "+",
        ]);
        Ok(self
            .find(&base, filters)
            .await?
            .into_iter()
            .filter(|path| matches_pattern(&pattern, &base, path))
            .take(request.limit.unwrap_or(usize::MAX))
            .collect())
    }

    async fn grep(&self, request: GrepRequest) -> Result<GrepResult, OperationError> {
        if request.query.contains(['\0', '\n']) {
            return Err(failed("grep query must be a single literal line"));
        }
        let include = request
            .include
            .as_deref()
            .map(Pattern::new)
            .transpose()
            .map_err(|_| failed("invalid grep include pattern"))?;
        let files = self
            .find(&request.base_dir, strings(&["-type", "f", "-print0"]))
            .await?;
        let files: Vec<_> = files
            .into_iter()
            .filter(|path| {
                include
                    .as_ref()
                    .is_none_or(|pattern| matches_pattern(pattern, &request.base_dir, path))
            })
            .collect();
        if files.is_empty() {
            return Ok(GrepResult {
                entries: Vec::new(),
            });
        }
        let mode = match request.mode {
            GrepMode::FilesWithMatches => "-l",
            GrepMode::Content => "-H",
            GrepMode::Count => "-cH",
        };
        let mut args = strings(&["-aFZ", mode, "--", &request.query]);
        args.extend(files.iter().map(|file| file.native().to_owned()));
        let result = self
            .query(&request.base_dir, "grep", args, 64 * 1024)
            .await?;
        let output = command_output(result, &[0, 1])?;
        let mut remaining = output.as_str();
        let mut entries = Vec::new();
        while !remaining.is_empty() && entries.len() < request.head_limit.unwrap_or(usize::MAX) {
            let (file, rest) = remaining
                .split_once('\0')
                .ok_or_else(|| failed("invalid client grep output"))?;
            let file = self.resolve(Path::new(file))?;
            remaining = rest;
            if request.mode == GrepMode::FilesWithMatches {
                entries.push(file.native().to_owned());
                continue;
            }
            let (line, rest) = rest
                .split_once('\n')
                .ok_or_else(|| failed("invalid client grep output"))?;
            remaining = rest;
            if request.mode != GrepMode::Count || line != "0" {
                entries.push(format!("{}:{line}", file.native()));
            }
        }
        Ok(GrepResult { entries })
    }
}

/// Compile a validated Rust glob for matching UTF-8 paths in the client.
fn glob_prefilter(pattern: &str) -> String {
    use std::fmt::Write;

    let mut chars = pattern.chars().peekable();
    let mut filter = String::from(r"(*UTF)(?s)\A");
    while let Some(ch) = chars.next() {
        match ch {
            '*' => {
                if chars.next_if_eq(&'*').is_some() && chars.next_if_eq(&'/').is_some() {
                    filter.push_str("(?:.*/)?");
                } else {
                    filter.push_str(".*");
                }
            }
            '?' => filter.push('.'),
            '[' => {
                let negated = chars.next_if_eq(&'!').is_some();
                // A closing bracket in the first member position is literal.
                let members: Vec<_> = chars
                    .next()
                    .into_iter()
                    .chain(chars.by_ref().take_while(|&ch| ch != ']'))
                    .collect();
                let mut class = String::new();
                let mut members = members.as_slice();
                while let [first, ..] = members {
                    match members {
                        [start, '-', end, rest @ ..] => {
                            if start <= end {
                                write!(class, r"\x{{{:x}}}-\x{{{:x}}}", *start as u32, *end as u32)
                                    .unwrap();
                            }
                            members = rest;
                        }
                        [_, rest @ ..] => {
                            write!(class, r"\x{{{:x}}}", *first as u32).unwrap();
                            members = rest;
                        }
                        [] => unreachable!(),
                    }
                }
                if class.is_empty() {
                    filter.push_str(if negated { "." } else { "(?!)" });
                } else {
                    filter.push('[');
                    if negated {
                        filter.push('^');
                    }
                    filter.push_str(&class);
                    filter.push(']');
                }
            }
            literal => write!(filter, r"\x{{{:x}}}", literal as u32).unwrap(),
        }
    }
    filter.push_str(r"\z");
    filter
}

fn matches_pattern(pattern: &Pattern, base: &BackendPath, path: &BackendPath) -> bool {
    let path = Path::new(path.native());
    path.strip_prefix(base.native())
        .is_ok_and(|relative| pattern.matches_path(relative))
        || pattern.matches_path(path)
}

#[async_trait]
impl OperationExport for ClientBackend {
    async fn export_file(
        &self,
        _: ExportFileRequest,
    ) -> Result<SharedExportedFileHandle, OperationError> {
        Err(unsupported("client files cannot be exported"))
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/operation_backend/backends/client_test.rs"]
mod tests;
