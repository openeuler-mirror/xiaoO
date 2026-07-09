use std::process::Stdio;

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_types::common::HookerId;
use agent_types::hook::{HookInvokeMetadata, HookPointId};
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

/// Shared, error-agnostic core for the plugin hookers (`chat`, `session`,
/// `llm`, `tool`). Each concrete adaptor embeds a `PluginHookerCore` and
/// delegates the structural concerns — identity, hook point, the plugin
/// command subprocess, and JSON payload helpers — to it, supplying only the
/// error constructor (`Fn(String) -> E`) appropriate to its domain. This
/// removes the byte-identical `serialize_*` / `run_plugin_command` /
/// `read_required_*` bodies that were previously copy-pasted across every
/// adaptor.
pub(crate) struct PluginHookerCore {
    id: HookerId,
    hook_point: HookPointId,
    command: String,
    definition: Value,
}

impl PluginHookerCore {
    pub(crate) fn new(
        id: HookerId,
        hook_point: HookPointId,
        command: String,
        definition: Value,
    ) -> Self {
        Self {
            id,
            hook_point,
            command,
            definition,
        }
    }

    pub(crate) fn id(&self) -> &HookerId {
        &self.id
    }

    pub(crate) fn hook_point(&self) -> &HookPointId {
        &self.hook_point
    }

    pub(crate) fn definition(&self) -> &Value {
        &self.definition
    }

    /// Replace the hook point. Used by adaptor unit tests to point a shared
    /// `adaptor_for` helper at the specific hook point under test.
    #[cfg(test)]
    pub(crate) fn set_hook_point(&mut self, hook_point: HookPointId) {
        self.hook_point = hook_point;
    }

    /// Build the `hooker` info block emitted in every plugin payload: the
    /// hooker id, its hook point, the command to run, and the ambient agent
    /// id (read from the runtime's agent context).
    pub(crate) fn serialize_hooker_info(&self, runtime: &dyn RuntimeView) -> Value {
        json!({
            "id": self.id.0,
            "hook_point": self.hook_point.0,
            "command": self.command,
            "agent_id": runtime.agent_context().metadata().agent_id,
        })
    }

    /// Build the `metadata` block emitted in every plugin payload: the
    /// trace/span identifiers carried by [`HookInvokeMetadata`].
    pub(crate) fn serialize_metadata(&self, metadata: &HookInvokeMetadata) -> Value {
        json!({
            "trace_id": metadata.trace_id,
            "span_id": metadata.span_id,
            "parent_span_id": metadata.parent_span_id,
        })
    }

    /// Run the configured plugin command, feeding `payload` to its stdin and
    /// parsing the stdout as JSON. `make_err` lifts a human-readable message
    /// into the adaptor's own error type, so the subprocess/IO/JSON/timeout
    /// failure path is shared verbatim across all four plugin hookers.
    /// `timeout_ms` controls the hard cap on subprocess runtime: `Some(ms)`
    /// kills the child after `ms` milliseconds. All adaptors (chat / llm /
    /// tool / session state) pass `Some(PLUGIN_HOOK_COMMAND_TIMEOUT_MS)` so a
    /// hung script cannot leak the driving task indefinitely — even the
    /// fire-and-forget session state hook, whose background task would
    /// otherwise linger forever. Delegates to the shared
    /// [`run_plugin_subprocess`] driver.
    pub(crate) async fn run_plugin_command<E>(
        &self,
        payload: &Value,
        make_err: impl Fn(String) -> E,
        timeout_ms: Option<u64>,
    ) -> Result<Value, E> {
        run_plugin_subprocess(&self.id, &self.command, payload, make_err, timeout_ms).await
    }

    /// Read a required string field from a plugin's JSON response. Used to
    /// pull the `result` tag before matching on it; `make_err` produces the
    /// adaptor-specific "missing field" error.
    pub(crate) fn read_required_string_field<'a, E>(
        &self,
        output: &'a Value,
        field_name: &str,
        make_err: impl Fn(String) -> E,
    ) -> Result<&'a str, E> {
        output
            .get(field_name)
            .and_then(Value::as_str)
            .ok_or_else(|| {
                make_err(format!(
                    "plugin hooker '{}' response must contain string field '{}'",
                    self.id.0, field_name
                ))
            })
    }

    /// Read and lowercase the `result` tag from a plugin's JSON response.
    pub(crate) fn read_required_result_tag<E>(
        &self,
        output: &Value,
        make_err: impl Fn(String) -> E,
    ) -> Result<String, E> {
        Ok(self
            .read_required_string_field(output, "result", make_err)?
            .to_lowercase())
    }
}

/// Default hard cap on how long a single plugin hooker subprocess may run, in
/// milliseconds, used by every plugin hooker (chat / llm / tool / session
/// state). Plugin hookers are short shell scripts (read stdin JSON, write
/// stdout JSON); a hung script (deadlock, infinite loop, blocking network
/// call without its own timeout) would otherwise block the async task
/// driving it forever. After this duration the child is killed
/// (`kill_on_drop`) and the hooker fails with a timeout error. The
/// fire-and-forget session state hook passes the same cap so a hung plugin
/// script cannot leak its background `tokio::spawn` task permanently.
pub(crate) const PLUGIN_HOOK_COMMAND_TIMEOUT_MS: u64 = 30_000;

/// Shared subprocess driver for plugin hookers: spawn `sh -c <command>`,
/// feed `payload` to stdin, await stdout, kill the child on timeout, and
/// parse stdout as JSON. `make_err` lifts a human-readable message into the
/// adaptor's own error type so the failure path (including timeout) is
/// shared verbatim across the chat / session / llm / tool plugin hookers.
/// `timeout_ms` is `Some(ms)` for all callers; a `None` value would wait
/// indefinitely, which is never desirable — even the fire-and-forget
/// session state hook passes a cap to avoid leaking its background task.
/// This is `async` so it never blocks the async executor thread driving the
/// agent loop regardless of the timeout choice.
pub(crate) async fn run_plugin_subprocess<E>(
    id: &HookerId,
    command: &str,
    payload: &Value,
    make_err: impl Fn(String) -> E,
    timeout_ms: Option<u64>,
) -> Result<Value, E> {
    let payload_bytes = serde_json::to_vec(payload).map_err(|error| {
        make_err(format!(
            "failed to serialize plugin command payload for hooker '{}': {}",
            id.0, error
        ))
    })?;

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            make_err(format!(
                "failed to spawn plugin command for hooker '{}' (command='{}'): {}",
                id.0, command, error
            ))
        })?;

    // Drive the full subprocess interaction — stdin write then stdout wait —
    // as a single future so one `timeout` covers both phases. The stdin
    // write alone (when the plugin ignores stdin and the payload exceeds the
    // pipe buffer, ~64KB) would otherwise block the async task forever:
    // `write_all` cannot complete until the child drains the pipe, and the
    // previous implementation only wrapped `wait_with_output`, which runs
    // after the write. `kill_on_drop(true)` on the `Child` ensures the
    // subprocess is reaped when this future is dropped on timeout, whether
    // the timeout fires during the stdin write or while awaiting stdout.
    let run = async {
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&payload_bytes).await.map_err(|error| {
                make_err(format!(
                    "failed to write stdin for plugin hooker '{}' (command='{}'): {}",
                    id.0, command, error
                ))
            })?;
        }
        child.wait_with_output().await.map_err(|error| {
            make_err(format!(
                "failed to wait for plugin hooker '{}' (command='{}'): {}",
                id.0, command, error
            ))
        })
    };

    let output = match timeout_ms {
        Some(ms) => match timeout(Duration::from_millis(ms), run).await {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => return Err(err),
            Err(_) => {
                // `run`'s future owns the `Child`; dropping it here triggers
                // `kill_on_drop`, reaping the hung subprocess.
                return Err(make_err(format!(
                    "plugin hooker '{}' command '{}' timed out after {}ms",
                    id.0, command, ms
                )));
            }
        },
        None => run.await?,
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(make_err(format!(
            "plugin hooker '{}' command '{}' exited with status {}{}",
            id.0,
            command,
            output.status,
            if stderr.is_empty() {
                String::new()
            } else {
                format!(": {}", stderr)
            }
        )));
    }

    serde_json::from_slice(&output.stdout).map_err(|error| {
        make_err(format!(
            "plugin hooker '{}' command '{}' returned invalid JSON: {}",
            id.0, command, error
        ))
    })
}
