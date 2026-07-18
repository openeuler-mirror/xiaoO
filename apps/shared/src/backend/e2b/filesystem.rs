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
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde_json::json;
use std::io::Cursor;
use std::sync::Arc;

use super::backend::{connect_json, http_error, shell_quote, E2bBackendState};
use super::exec::E2bExec;

pub(crate) struct E2bFileSystem {
    state: Arc<E2bBackendState>,
    exec: E2bExec,
}

impl E2bFileSystem {
    pub(crate) fn new(state: Arc<E2bBackendState>) -> Self {
        Self {
            exec: E2bExec::new(Arc::clone(&state)),
            state,
        }
    }
    async fn upload_raw(&self, path: &BackendPath, content: Vec<u8>) -> Result<(), OperationError> {
        let response = self
            .state
            .envd_request(Method::POST, "/files")
            .query(&[("path", path.0.as_str())])
            .header(CONTENT_TYPE, "application/octet-stream")
            .header(ACCEPT, "application/json")
            .body(content)
            .send()
            .await
            .map_err(|error| OperationError::Transport {
                message: format!("failed to upload e2b file {}: {error}", path.0),
            })?;

        if response.status().is_success() {
            return Ok(());
        }
        Err(http_error("upload e2b file", response).await)
    }
}

struct E2bExportedFileHandle {
    state: Arc<E2bBackendState>,
    path: BackendPath,
    metadata: ExportedFileMeta,
}

#[async_trait]
impl ExportedFileHandle for E2bExportedFileHandle {
    fn metadata(&self) -> &ExportedFileMeta {
        &self.metadata
    }

    async fn open_read(&self) -> Result<ExportedFileReader, OperationError> {
        let fs = E2bFileSystem::new(Arc::clone(&self.state));
        let content = fs
            .read_bytes(ReadBytesRequest {
                path: self.path.clone(),
            })
            .await?;
        Ok(Box::new(Cursor::new(content)))
    }
}

#[async_trait]
impl OperationFileSystem for E2bFileSystem {
    async fn stat(&self, path: &BackendPath) -> Result<PathStat, OperationError> {
        self.state.ensure_active()?;
        let response: Result<StatResponse, OperationError> = connect_json(
            &self.state,
            "/filesystem.Filesystem/Stat",
            json!({ "path": path.0 }),
        )
        .await;

        match response {
            Ok(response) => Ok(path_stat_from_entry(response.entry)),
            Err(OperationError::NotFound { .. }) => Ok(PathStat {
                exists: false,
                kind: None,
                size_bytes: None,
                modified_at: None,
            }),
            Err(error) => Err(error),
        }
    }

    async fn read_bytes(&self, request: ReadBytesRequest) -> Result<Vec<u8>, OperationError> {
        self.state.ensure_active()?;
        let response = self
            .state
            .envd_request(Method::GET, "/files")
            .query(&[("path", request.path.0.as_str())])
            .header(ACCEPT, "application/octet-stream")
            .send()
            .await
            .map_err(|error| OperationError::Transport {
                message: format!("failed to download e2b file {}: {error}", request.path.0),
            })?;

        if response.status() == StatusCode::NOT_FOUND {
            return Err(OperationError::NotFound {
                path: request.path.0,
            });
        }
        if !response.status().is_success() {
            return Err(http_error("download e2b file", response).await);
        }

        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|error| OperationError::Transport {
                message: format!("failed to read e2b file response: {error}"),
            })
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
        let before = self.stat(&request.path).await?;

        match request.mode {
            WriteMode::AtomicOverwrite => {
                let temp_path = self
                    .temp_path(TempPathRequest {
                        kind: TempPathKind::File,
                        preferred_parent: None,
                        prefix: Some(".xiaoo-atomic-".to_string()),
                        suffix: Some(".tmp".to_string()),
                    })
                    .await?;
                self.upload_raw(&temp_path, request.content).await?;
                let destination = shell_quote(request.path.0.as_str());
                let script = format!(
                    "parent=$(dirname {destination}) && mkdir -p \"$parent\" && mv -f {} {destination}",
                    shell_quote(temp_path.0.as_str()),
                );
                let output = self.exec.run_shell_script(script.as_str(), None).await?;
                if output.exit_code != Some(0) {
                    return Err(OperationError::ExecutionFailed {
                        message: String::from_utf8_lossy(output.stderr.as_slice()).to_string(),
                    });
                }
            }
            WriteMode::Create | WriteMode::Overwrite => {
                self.upload_raw(&request.path, request.content).await?;
            }
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
            TempPathKind::File => ": > \"$path\"",
            TempPathKind::Directory => "mkdir \"$path\"",
        };
        let script = format!(
            "mkdir -p {quoted_parent}\nprefix={prefix}\nsuffix={suffix}\nwhile true; do\n  path=\"{parent}/$prefix$(date +%s%N)-$RANDOM$suffix\"\n  if [ ! -e \"$path\" ]; then\n    {creation}\n    printf '%s' \"$path\"\n    exit 0\n  fi\ndone",
            parent = parent.0,
        );
        let output = self.exec.run_shell_script(script.as_str(), None).await?;
        if output.exit_code != Some(0) {
            return Err(OperationError::ExecutionFailed {
                message: String::from_utf8_lossy(output.stderr.as_slice()).to_string(),
            });
        }
        let text = String::from_utf8_lossy(output.stdout.as_slice())
            .trim()
            .to_string();
        Ok(BackendPath(text))
    }
}

#[async_trait]
impl OperationExport for E2bFileSystem {
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
        Ok(Arc::new(E2bExportedFileHandle {
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

fn path_stat_from_entry(entry: Option<EntryInfo>) -> PathStat {
    let Some(entry) = entry else {
        return PathStat {
            exists: false,
            kind: None,
            size_bytes: None,
            modified_at: None,
        };
    };
    PathStat {
        exists: true,
        kind: file_type_to_kind(entry.file_type.as_deref()),
        size_bytes: entry.size.and_then(|size| size.as_u64()),
        modified_at: None,
    }
}

fn file_type_to_kind(value: Option<&str>) -> Option<PathKind> {
    match value {
        Some("FILE_TYPE_FILE") => Some(PathKind::File),
        Some("FILE_TYPE_DIRECTORY") => Some(PathKind::Directory),
        Some("FILE_TYPE_SYMLINK") => Some(PathKind::Symlink),
        Some("FILE_TYPE_UNSPECIFIED") | None => None,
        Some(_) => Some(PathKind::Other),
    }
}

#[derive(Debug, Deserialize)]
struct StatResponse {
    entry: Option<EntryInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntryInfo {
    #[serde(rename = "type")]
    file_type: Option<String>,
    size: Option<SizeValue>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SizeValue {
    Number(u64),
    String(String),
}

impl SizeValue {
    fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Number(value) => Some(*value),
            Self::String(value) => value.parse().ok(),
        }
    }
}
