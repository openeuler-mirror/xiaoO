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
        let script = format!(
            "find {} -name {} -print",
            shell_quote(base_dir.0.as_str()),
            shell_quote(request.pattern.as_str())
        );
        let output = self.exec.run_shell_script(script.as_str(), None).await?;
        let mut paths = String::from_utf8_lossy(output.stdout.as_slice())
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| BackendPath(line.to_string()))
            .collect::<Vec<_>>();
        if let Some(limit) = request.limit {
            paths.truncate(limit);
        }
        Ok(paths)
    }

    async fn grep(&self, request: GrepRequest) -> Result<GrepResult, OperationError> {
        let include = request.include.unwrap_or_else(|| "*".to_string());
        let grep_cmd = match request.mode {
            GrepMode::FilesWithMatches => "grep -l",
            GrepMode::Content => "grep -nH",
            GrepMode::Count => "grep -cH",
        };
        let mut script = format!(
            "find {} -type f -name {} -exec {} -- {} {{}} +",
            shell_quote(request.base_dir.0.as_str()),
            shell_quote(include.as_str()),
            grep_cmd,
            shell_quote(request.query.as_str())
        );
        if let Some(limit) = request.head_limit {
            script = format!("({script}) | head -n {limit}");
        }
        let output = self.exec.run_shell_script(script.as_str(), None).await?;
        let exit_code = output.exit_code.unwrap_or(-1);
        if exit_code != 0 && exit_code != 1 {
            return Err(OperationError::ExecutionFailed {
                message: String::from_utf8_lossy(output.stderr.as_slice()).to_string(),
            });
        }
        Ok(GrepResult {
            entries: String::from_utf8_lossy(output.stdout.as_slice())
                .lines()
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect(),
        })
    }
}
