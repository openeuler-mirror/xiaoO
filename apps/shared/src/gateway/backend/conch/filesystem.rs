use crate::gateway::backend::conch::agent;
use crate::gateway::backend::conch::backend::{shell_quote, ConchBackendState, ConchUploadFile};
use crate::gateway::backend::conch::exec::ConchExec;
use agent_contracts::backend::{
    capability::{
        export::ExportFileRequest,
        filesystem::{
            ReadBytesRequest, TempPathKind, TempPathRequest, WriteBytesOutcome, WriteBytesRequest,
            WriteMode,
        },
        OperationExport, OperationFileSystem,
    },
    BackendPath, ExportedFileHandle, ExportedFileMeta, ExportedFileReader, OperationError,
    PathKind, PathStat, SharedExportedFileHandle,
};
use async_trait::async_trait;
use std::io::Cursor;
use std::sync::Arc;

pub(crate) struct ConchFileSystem {
    state: Arc<ConchBackendState>,
    exec: ConchExec,
}

impl ConchFileSystem {
    pub(crate) fn new(state: Arc<ConchBackendState>) -> Self {
        Self {
            exec: ConchExec::new(Arc::clone(&state)),
            state,
        }
    }
}

struct ConchExportedFileHandle {
    state: Arc<ConchBackendState>,
    path: BackendPath,
    metadata: ExportedFileMeta,
}

#[async_trait]
impl ExportedFileHandle for ConchExportedFileHandle {
    fn metadata(&self) -> &ExportedFileMeta {
        &self.metadata
    }

    async fn open_read(&self) -> Result<ExportedFileReader, OperationError> {
        let content = agent::get_file(&self.state, self.path.0.as_str()).await?;
        Ok(Box::new(Cursor::new(content)))
    }
}

#[async_trait]
impl OperationFileSystem for ConchFileSystem {
    async fn stat(&self, path: &BackendPath) -> Result<PathStat, OperationError> {
        self.state.ensure_active()?;
        let quoted = shell_quote(path.0.as_str());
        let script = format!(
            "if [ -e {quoted} ]; then\n  if [ -f {quoted} ]; then kind=file; elif [ -d {quoted} ]; then kind=directory; elif [ -L {quoted} ]; then kind=symlink; else kind=other; fi\n  if [ -f {quoted} ]; then size=$(wc -c < {quoted}); else size=; fi\n  mtime=$(stat -c %Y {quoted} 2>/dev/null || true)\n  printf 'exists=true\\nkind=%s\\nsize=%s\\nmtime=%s\\n' \"$kind\" \"$size\" \"$mtime\"\nelse\n  printf 'exists=false\\n'\nfi"
        );
        let output = self.exec.run_shell_script(script.as_str(), None).await?;
        parse_stat_output(String::from_utf8_lossy(output.stdout.as_slice()).as_ref())
    }

    async fn read_bytes(&self, request: ReadBytesRequest) -> Result<Vec<u8>, OperationError> {
        agent::get_file(&self.state, request.path.0.as_str()).await
    }

    async fn write_bytes(
        &self,
        request: WriteBytesRequest,
    ) -> Result<WriteBytesOutcome, OperationError> {
        self.state.ensure_active()?;
        if matches!(request.mode, WriteMode::Create) {
            let stat = self.stat(&request.path).await?;
            if stat.exists {
                return Err(OperationError::AlreadyExists {
                    path: request.path.0,
                });
            }
        }
        if matches!(request.mode, WriteMode::AtomicOverwrite) {
            return Err(OperationError::Unsupported {
                message: "conch backend does not support atomic overwrite".to_string(),
            });
        }
        let before = self.stat(&request.path).await?;
        let uploaded = agent::post_files(
            &self.state,
            vec![ConchUploadFile {
                filepath: request.path.0.clone(),
                content: request.content,
            }],
        )
        .await?;
        if uploaded != 1 {
            return Err(OperationError::Transport {
                message: format!("unexpected uploaded_count from conch agent: {uploaded}"),
            });
        }
        Ok(WriteBytesOutcome {
            path: request.path,
            created: !before.exists,
        })
    }

    async fn create_dir_all(&self, path: &BackendPath) -> Result<(), OperationError> {
        let script = format!("mkdir -p {}", shell_quote(path.0.as_str()));
        let output = self.exec.run_shell_script(script.as_str(), None).await?;
        if output.exit_code == Some(0) {
            return Ok(());
        }
        Err(OperationError::ExecutionFailed {
            message: String::from_utf8_lossy(output.stderr.as_slice()).to_string(),
        })
    }

    async fn temp_path(&self, request: TempPathRequest) -> Result<BackendPath, OperationError> {
        let parent = request
            .preferred_parent
            .unwrap_or_else(|| self.state.temp_root.clone());
        let quoted_parent = shell_quote(parent.0.as_str());
        let prefix = shell_quote(request.prefix.as_deref().unwrap_or("tmp-"));
        let suffix = shell_quote(request.suffix.as_deref().unwrap_or(""));
        let creation = match request.kind {
            TempPathKind::File => "touch \"$path\"",
            TempPathKind::Directory => "mkdir \"$path\"",
        };
        let script = format!(
            "mkdir -p {quoted_parent}\nprefix={prefix}\nsuffix={suffix}\nwhile true; do\n  path=\"{parent}/$prefix$RANDOM$(date +%s%N)$suffix\"\n  if [ ! -e \"$path\" ]; then\n    {creation}\n    printf '%s' \"$path\"\n    break\n  fi\ndone",
            parent = parent.0,
        );
        let output = self.exec.run_shell_script(script.as_str(), None).await?;
        let text = String::from_utf8_lossy(output.stdout.as_slice())
            .trim()
            .to_string();
        Ok(BackendPath(text))
    }
}

#[async_trait]
impl OperationExport for ConchFileSystem {
    async fn export_file(
        &self,
        request: ExportFileRequest,
    ) -> Result<SharedExportedFileHandle, OperationError> {
        let stat = self.stat(&request.path).await?;
        if !stat.exists {
            return Err(OperationError::NotFound {
                path: request.path.0.clone(),
            });
        }
        if stat.kind != Some(PathKind::File) {
            return Err(OperationError::NotFile {
                path: request.path.0.clone(),
            });
        }
        let file_name = request.preferred_name.unwrap_or_else(|| {
            request
                .path
                .0
                .rsplit('/')
                .next()
                .unwrap_or("exported-file")
                .to_string()
        });
        Ok(Arc::new(ConchExportedFileHandle {
            state: Arc::clone(&self.state),
            path: request.path,
            metadata: ExportedFileMeta {
                file_name,
                size_bytes: stat.size_bytes,
                media_type: None,
            },
        }))
    }
}

fn parse_stat_output(output: &str) -> Result<PathStat, OperationError> {
    let mut exists = false;
    let mut kind = None;
    let mut size_bytes = None;
    let mut modified_at = None;
    for line in output.lines() {
        if let Some(value) = line.strip_prefix("exists=") {
            exists = value == "true";
        } else if let Some(value) = line.strip_prefix("kind=") {
            kind = match value {
                "file" => Some(PathKind::File),
                "directory" => Some(PathKind::Directory),
                "symlink" => Some(PathKind::Symlink),
                "other" => Some(PathKind::Other),
                _ => None,
            };
        } else if let Some(value) = line.strip_prefix("size=") {
            if !value.is_empty() {
                size_bytes = value.parse::<u64>().ok();
            }
        } else if let Some(value) = line.strip_prefix("mtime=") {
            if let Ok(seconds) = value.parse::<u64>() {
                modified_at = Some(
                    std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds),
                );
            }
        }
    }
    Ok(PathStat {
        exists,
        kind,
        size_bytes,
        modified_at,
    })
}
