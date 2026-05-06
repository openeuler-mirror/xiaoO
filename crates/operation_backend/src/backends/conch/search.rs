use crate::backends::conch::backend::{shell_quote, ConchBackendState};
use crate::backends::conch::exec::ConchExec;
use agent_contracts::backend::{
    capability::{
        search::{GlobRequest, GrepMode, GrepRequest, GrepResult},
        OperationSearch,
    },
    BackendPath, OperationError,
};
use async_trait::async_trait;
use glob::Pattern;
use std::path::Path;
use std::sync::Arc;

pub(crate) struct ConchSearch {
    exec: ConchExec,
}

impl ConchSearch {
    pub(crate) fn new(state: Arc<ConchBackendState>) -> Self {
        Self {
            exec: ConchExec::new(state),
        }
    }
}

#[async_trait]
impl OperationSearch for ConchSearch {
    async fn glob(&self, request: GlobRequest) -> Result<Vec<BackendPath>, OperationError> {
        let base_dir = request
            .base_dir
            .unwrap_or_else(|| self.exec.state().workspace_root.clone());
        let pattern = Pattern::new(request.pattern.as_str()).map_err(|error| {
            OperationError::InvalidPath {
                message: format!("invalid glob pattern: {error}"),
            }
        })?;
        let script = format!(
            "find {} -mindepth 1 -print",
            shell_quote(base_dir.0.as_str()),
        );
        let output = self.exec.run_shell_script(script.as_str(), None).await?;
        let mut paths = String::from_utf8_lossy(output.stdout.as_slice())
            .lines()
            .filter(|line| !line.is_empty())
            .filter_map(|line| {
                let path = Path::new(line);
                let relative = path
                    .strip_prefix(base_dir.0.as_str())
                    .ok()
                    .and_then(|value| value.to_str())
                    .unwrap_or("")
                    .trim_start_matches('/');
                if pattern.matches(relative) || pattern.matches_path(path) {
                    Some(BackendPath(line.to_string()))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        paths.sort_by(|a, b| a.0.cmp(&b.0));
        if let Some(limit) = request.limit {
            paths.truncate(limit);
        }
        Ok(paths)
    }

    async fn grep(&self, request: GrepRequest) -> Result<GrepResult, OperationError> {
        let mut cmd = vec!["grep -r -H".to_string()];

        match &request.mode {
            GrepMode::FilesWithMatches => {
                cmd.push("-l".to_string());
            }
            GrepMode::Content => {}
            GrepMode::Count => {
                cmd.push("-c".to_string());
            }
        }

        if let Some(include) = &request.include {
            cmd.push(format!("--include={}", shell_quote(include)));
        }

        cmd.push("--".to_string());
        cmd.push(shell_quote(&request.query));
        cmd.push(shell_quote(request.base_dir.0.as_str()));

        let script = cmd.join(" ");
        let output = self.exec.run_shell_script(script.as_str(), None).await?;

        // grep exits with 1 when no matches found — that is not an error
        let exit_code = output.exit_code.unwrap_or(-1);
        if exit_code != 0 && exit_code != 1 {
            return Err(OperationError::ExecutionFailed {
                message: String::from_utf8_lossy(output.stderr.as_slice()).to_string(),
            });
        }

        let stdout_text = String::from_utf8_lossy(output.stdout.as_slice());
        let mut entries = stdout_text
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        if let Some(limit) = request.head_limit {
            entries.truncate(limit);
        }

        Ok(GrepResult { entries })
    }
}
