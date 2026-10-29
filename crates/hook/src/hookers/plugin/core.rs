use std::process::Stdio;

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_types::common::{workspace_root_string, HookerId};
use agent_types::hook::{HookInvokeMetadata, HookPointId};
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

use super::interaction::{
    build_interaction_followup_payload, parse_plugin_command_response,
    with_hooker_interaction_source, PluginCommandResponse,
};

/// Shared, error-agnostic core for the plugin hookers (`chat`, `session`,
/// `llm`, `tool`). Each concrete adaptor embeds a `PluginHookerCore` and
/// delegates the structural concerns — identity, hook point, the plugin
/// command subprocess, the common payload skeleton, result-field reading,
/// and the `ask_user` interaction round-trip — to it, supplying only the
/// error constructors (`Fn(String) -> E`, plus a timeout variant)
/// appropriate to its domain, so every adaptor shares one implementation
/// instead of carrying byte-identical `serialize_*` / `run_plugin_command`
/// / `build_*_payload` / `read_required_*` / interaction-flow copies.
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
    /// parsing the stdout as JSON. Delegates to [`run_plugin_subprocess`];
    /// the `make_err` / `make_timeout_err` contracts and the
    /// why-every-caller-passes-a-cap rationale live there, and the
    /// per-domain cap choices (short observers vs. the 10-minute
    /// llm/tool cap) on [`PLUGIN_HOOK_COMMAND_TIMEOUT_MS`].
    pub(crate) async fn run_plugin_command<E>(
        &self,
        payload: &Value,
        make_err: &impl Fn(String) -> E,
        make_timeout_err: &impl Fn(String, u64) -> E,
        timeout_ms: Option<u64>,
    ) -> Result<Value, E> {
        run_plugin_subprocess(
            &self.id,
            &self.command,
            payload,
            make_err,
            make_timeout_err,
            timeout_ms,
        )
        .await
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

    /// Read a required field (of any JSON shape) from a plugin's JSON
    /// response; `make_err` produces the adaptor-specific "missing field"
    /// error.
    pub(crate) fn read_required_value_field<'a, E>(
        &self,
        output: &'a Value,
        field_name: &str,
        make_err: impl Fn(String) -> E,
    ) -> Result<&'a Value, E> {
        output.get(field_name).ok_or_else(|| {
            make_err(format!(
                "plugin hooker '{}' response must contain field '{}'",
                self.id.0, field_name
            ))
        })
    }

    /// Build the payload skeleton shared by every plugin hooker stage: the
    /// stage tag, the `hooker` info block, the trace `metadata` block, the
    /// effective `policy`, and the hooker `definition`. `extra_fields`
    /// carries the stage-specific fields merged on top, so adding a new
    /// common payload field is a one-line change here instead of an edit in
    /// every adaptor's builder (the shotgun surgery this core exists to
    /// prevent).
    pub(crate) fn build_stage_payload(
        &self,
        stage: &str,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
        extra_fields: Vec<(&str, Value)>,
    ) -> Value {
        let mut payload = json!({
            "stage": stage,
            "hooker": self.serialize_hooker_info(runtime),
            "metadata": self.serialize_metadata(metadata),
            "policy": runtime.hookers().policy_for(self.id()).cloned(),
            "definition": self.definition().clone(),
        });
        for (name, value) in extra_fields {
            payload[name] = value;
        }
        payload
    }

    /// Drive a plugin command to its final output, transparently handling
    /// the `ask_user` interaction protocol shared by the chat / llm / tool
    /// plugin hookers: when a plugin responds with an `ask_user` directive,
    /// the question is surfaced through the runtime's interaction handle,
    /// and the command is re-invoked with an `interaction` block carrying
    /// the request, the user's answer, and the plugin's continuation token.
    /// Session hooks are ack-only observers and never enter this flow.
    /// Error mapping follows [`Self::run_plugin_command`].
    pub(crate) async fn resolve_plugin_output<E>(
        &self,
        initial_payload: Value,
        runtime: &dyn RuntimeView,
        make_err: &impl Fn(String) -> E,
        make_timeout_err: &impl Fn(String, u64) -> E,
        timeout_ms: Option<u64>,
    ) -> Result<Value, E> {
        let mut payload = initial_payload;

        loop {
            let output = self
                .run_plugin_command(&payload, make_err, make_timeout_err, timeout_ms)
                .await?;
            match parse_plugin_command_response(self.id(), output, make_err)? {
                PluginCommandResponse::Final(final_output) => return Ok(final_output),
                PluginCommandResponse::AskUser(directive) => {
                    let request = with_hooker_interaction_source(
                        self.id(),
                        self.hook_point(),
                        directive.request,
                    );
                    let response = runtime.interaction().ask(&request).await;
                    payload = build_interaction_followup_payload(
                        self.id(),
                        payload,
                        directive.continuation,
                        &request,
                        &response,
                        make_err,
                    )?;
                }
            }
        }
    }
}

/// Build the `workspace` field emitted in tool/chat plugin payloads: the
/// absolute workspace root from the runtime's agent context, as a JSON
/// string. `null` when the runtime carries no workspace (empty root), so
/// plugins can distinguish "no workspace bound" from a real path. The
/// plugin subprocess's own cwd is inherited from the host process and
/// must not be relied on, which is why the root is carried in-band.
///
/// A free function (not a `PluginHookerCore` method) because it reads only
/// the runtime, never the hooker's own state — the session adaptors, whose
/// workspace comes from the hook input rather than the runtime, do not use
/// it. The empty-root-to-null rule itself is defined once in
/// [`workspace_root_string`].
pub(crate) fn serialize_workspace_root(runtime: &dyn RuntimeView) -> Value {
    match workspace_root_string(&runtime.agent_context().workspace().root) {
        Some(root) => Value::String(root),
        None => Value::Null,
    }
}

/// Default hard cap on how long a chat or session plugin hooker subprocess
/// may run, in milliseconds. Plugin hookers are short shell scripts (read
/// stdin JSON, write stdout JSON); a hung script (deadlock, infinite loop,
/// blocking network call without its own timeout) would otherwise block the
/// async task driving it forever. After this duration the child is killed
/// (`kill_on_drop`) and the hooker fails with a timeout error. The llm and
/// tool adaptors keep their own longer 10-minute cap
/// (`PLUGIN_HOOKER_TIMEOUT_MS` in each adaptor) because their plugins may
/// proxy slow LLM/tool work; the fire-and-forget session state hook passes
/// this same cap so a hung plugin script cannot leak its background
/// `tokio::spawn` task permanently.
pub(crate) const PLUGIN_HOOK_COMMAND_TIMEOUT_MS: u64 = 30_000;

/// Shared subprocess driver for plugin hookers: spawn `sh -c <command>`,
/// feed `payload` to stdin, await stdout, kill the child on timeout, and
/// parse stdout as JSON. `make_err` lifts a human-readable message into the
/// adaptor's own error type so the failure path is shared verbatim across
/// the chat / session / llm / tool plugin hookers; `make_timeout_err`
/// receives the preformatted timeout message plus the cap in milliseconds
/// so domains with a dedicated timeout variant can map it faithfully.
/// `timeout_ms` is `Some(ms)` for all callers — a `None` value would wait
/// indefinitely, leaking the driving task (see
/// [`PLUGIN_HOOK_COMMAND_TIMEOUT_MS`] for the session-hook case). This is
/// `async` so it never blocks the async executor thread driving the agent
/// loop regardless of the timeout choice.
pub(crate) async fn run_plugin_subprocess<E>(
    id: &HookerId,
    command: &str,
    payload: &Value,
    make_err: &impl Fn(String) -> E,
    make_timeout_err: &impl Fn(String, u64) -> E,
    timeout_ms: Option<u64>,
) -> Result<Value, E> {
    let payload_bytes = serde_json::to_vec(payload).map_err(|error| {
        make_err(format!(
            "failed to serialize plugin command payload for hooker '{}': {}",
            id.0, error
        ))
    })?;

    let mut process_command = Command::new("sh");
    process_command
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Bound permit admission, spawn, stdin write and stdout wait together.
    // The permit itself is released before the child's I/O. The stdin
    // write alone (when the plugin ignores stdin and the payload exceeds the
    // pipe buffer, ~64KB) would otherwise block the async task forever:
    // `write_all` cannot complete until the child drains the pipe, and only
    // the stdout wait would remain to be covered by a timeout if the write
    // were driven outside it. `kill_on_drop(true)` on the `Child` ensures the
    // subprocess is reaped when this future is dropped on timeout, whether
    // the timeout fires during the stdin write or while awaiting stdout.
    let deadline = timeout_ms.map(|ms| tokio::time::Instant::now() + Duration::from_millis(ms));
    let run = async {
        let mut child =
            process_coordination::spawn_tokio_with_deadline(&mut process_command, deadline)
                .await
                .map_err(|error| {
                    if error.kind() == std::io::ErrorKind::TimedOut {
                        return make_timeout_err(
                            format!(
                                "plugin hooker '{}' command '{}' timed out before spawn",
                                id.0, command
                            ),
                            timeout_ms.unwrap_or(0),
                        );
                    }
                    make_err(format!(
                        "failed to spawn plugin command for hooker '{}' (command='{}'): {}",
                        id.0, command, error
                    ))
                })?;

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
                return Err(make_timeout_err(
                    format!(
                        "plugin hooker '{}' command '{}' timed out after {}ms",
                        id.0, command, ms
                    ),
                    ms,
                ));
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

#[cfg(test)]
#[path = "../../../../../tests/unit/hook/hookers/plugin/core_test.rs"]
mod tests;
