use std::any::Any;

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::Hooker;
use agent_types::hook::{HookInvokeError, HookInvokeInput, HookInvokeMetadata, HookInvokeOutput};
use agent_types::session::{
    SessionClosedHookInput, SessionCreatedHookInput, SessionHookError, SessionHookResult,
    SessionStateHookInput,
};
use async_trait::async_trait;
use serde_json::{json, Value};

use super::super::core::PluginHookerCore;
use super::super::PLUGIN_HOOK_COMMAND_TIMEOUT_MS;
use crate::{resolve_hook_point_category, HookPointCategory};

/// Plugin adaptor for the `*.Session.lifecycle.*` family: `created`,
/// `closed`, and `state`.
///
/// Unlike the (input, output) chat/llm/tool adaptors, these are event-style
/// observer hooks: the only accepted result is `{"result":"ack"}`, mapped to
/// [`SessionHookResult::Acknowledged`]. There is no `transform`/`deny` path
/// because the events carry no mutable output. The three stages share one
/// adaptor; the payload's `stage` field (`session_created` /
/// `session_closed` / `session_state`) tells the plugin which event fired,
/// and the state event additionally carries `state` / `outcome` so plugins
/// switch on `payload.state` rather than on the hook point. The caller in
/// the gateway layer chooses the dispatch strategy per state: `idle` is
/// awaited so plugin-requested `actions` can be collected into
/// `AppTurnResult.hook_actions`; `failed` is fire-and-forget and its
/// `actions` are discarded. Created/closed are best-effort observers whose
/// output is not consumed by the gateway (`actions` are not collected).
/// This adaptor itself runs the plugin command once and returns.
pub(crate) struct PluginSessionHookerAdaptor {
    core: PluginHookerCore,
}

impl PluginSessionHookerAdaptor {
    pub fn new(
        id: agent_types::common::HookerId,
        hook_point: agent_types::hook::HookPointId,
        command: String,
        definition: Value,
    ) -> Self {
        Self {
            core: PluginHookerCore::new(id, hook_point, command, definition),
        }
    }

    /// Lift a message into the session-domain plugin error. Passed to
    /// [`PluginHookerCore`] helpers so the shared subprocess/JSON code paths
    /// construct `SessionHookError` rather than a foreign error type.
    fn err(message: String) -> SessionHookError {
        SessionHookError::Plugin { message }
    }

    /// Session hook timeouts surface as the same plugin error, carrying the
    /// driver's preformatted timeout message.
    fn timeout_err(message: String, _timeout_ms: u64) -> SessionHookError {
        Self::err(message)
    }

    async fn invoke_for_category(
        &self,
        category: HookPointCategory,
        input: HookInvokeInput,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, SessionHookError> {
        match (category, input) {
            (
                HookPointCategory::SessionCreated,
                HookInvokeInput::SessionCreated { input, metadata },
            ) => {
                self.invoke_session_created(&input, &metadata, runtime)
                    .await
            }
            (
                HookPointCategory::SessionClosed,
                HookInvokeInput::SessionClosed { input, metadata },
            ) => self.invoke_session_closed(&input, &metadata, runtime).await,
            (
                HookPointCategory::SessionState,
                HookInvokeInput::SessionState { input, metadata },
            ) => self.invoke_session_state(&input, &metadata, runtime).await,
            (category, _) => Err(Self::err(format!(
                "session hooker '{}' received mismatched invoke input for category {:?}",
                self.core.id().0,
                category
            ))),
        }
    }

    async fn invoke_session_created(
        &self,
        input: &SessionCreatedHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, SessionHookError> {
        let payload = self.build_session_created_payload(input, metadata, runtime);
        let (_, primary) = self.run_session_event(&payload, "created").await?;
        Ok(HookInvokeOutput::SessionCreated(primary))
    }

    async fn invoke_session_closed(
        &self,
        input: &SessionClosedHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, SessionHookError> {
        let payload = self.build_session_closed_payload(input, metadata, runtime);
        let (_, primary) = self.run_session_event(&payload, "closed").await?;
        Ok(HookInvokeOutput::SessionClosed(primary))
    }

    async fn invoke_session_state(
        &self,
        input: &SessionStateHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, SessionHookError> {
        let payload = self.build_session_state_payload(input, metadata, runtime);
        let (output, primary) = self.run_session_event(&payload, "state").await?;
        let actions = agent_types::hook::parse_actions(&output);
        Ok(HookInvokeOutput::SessionState(primary).with_actions(actions))
    }

    /// Run the plugin command for one session lifecycle event and parse its
    /// ack-only result. Returns the raw plugin output alongside the parsed
    /// result so the state stage can additionally collect plugin-requested
    /// `actions` from it.
    async fn run_session_event(
        &self,
        payload: &Value,
        stage: &str,
    ) -> Result<(Value, SessionHookResult), SessionHookError> {
        let output = self
            .core
            .run_plugin_command(
                payload,
                &Self::err,
                &Self::timeout_err,
                Some(PLUGIN_HOOK_COMMAND_TIMEOUT_MS),
            )
            .await?;
        let primary = self.parse_session_event_result(&output, stage)?;
        Ok((output, primary))
    }

    /// Build the payload shared by every session lifecycle event: the
    /// common plugin payload skeleton plus the session identity fields
    /// (`session_id` / `sender_id` / `workspace`) that all three session
    /// hook inputs carry. Stage-specific fields are layered on by the
    /// per-stage builders below.
    fn build_session_payload(
        &self,
        stage: &str,
        session_id: &str,
        sender_id: &str,
        workspace: &Option<String>,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Value {
        self.core.build_stage_payload(
            stage,
            metadata,
            runtime,
            vec![
                ("session_id", json!(session_id)),
                ("sender_id", json!(sender_id)),
                ("workspace", json!(workspace)),
            ],
        )
    }

    fn build_session_created_payload(
        &self,
        input: &SessionCreatedHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Value {
        self.build_session_payload(
            "session_created",
            &input.session_id,
            &input.sender_id,
            &input.workspace,
            metadata,
            runtime,
        )
    }

    fn build_session_closed_payload(
        &self,
        input: &SessionClosedHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Value {
        self.build_session_payload(
            "session_closed",
            &input.session_id,
            &input.sender_id,
            &input.workspace,
            metadata,
            runtime,
        )
    }

    fn build_session_state_payload(
        &self,
        input: &SessionStateHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Value {
        let mut payload = self.build_session_payload(
            "session_state",
            &input.session_id,
            &input.sender_id,
            &input.workspace,
            metadata,
            runtime,
        );
        payload["state"] = json!(input.state);
        payload["outcome"] = json!(input.outcome);
        payload["agent_id"] = json!(input.agent_id);
        payload
    }

    /// Result parsing for the ack-only session lifecycle hooks: the only
    /// accepted result is `ack` / `acknowledged`, mapped to
    /// [`SessionHookResult::Acknowledged`]. There is no `transform`/`deny`
    /// path because the events carry no mutable output; created/closed are
    /// best-effort observers whose output is not consumed by the gateway
    /// (`actions` are not collected; only `state` collects them).
    fn parse_session_event_result(
        &self,
        output: &Value,
        stage: &str,
    ) -> Result<SessionHookResult, SessionHookError> {
        match self
            .core
            .read_required_result_tag(output, Self::err)?
            .as_str()
        {
            "ack" | "acknowledged" => Ok(SessionHookResult::Acknowledged),
            result => Err(Self::err(format!(
                "plugin session hooker '{}' returned unsupported result '{}'; only 'ack' is valid for event-style {} hook",
                self.core.id().0,
                result,
                stage
            ))),
        }
    }
}

#[async_trait]
impl Hooker for PluginSessionHookerAdaptor {
    fn id(&self) -> &agent_types::common::HookerId {
        self.core.id()
    }

    fn hook_point(&self) -> &agent_types::hook::HookPointId {
        self.core.hook_point()
    }

    async fn invoke(
        &self,
        input: HookInvokeInput,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, HookInvokeError> {
        let category = resolve_hook_point_category(self.core.hook_point()).map_err(|error| {
            Self::err(format!(
                "failed to resolve hook point category for hooker '{}': {}",
                self.core.id().0,
                error
            ))
        })?;
        self.invoke_for_category(category, input, runtime)
            .await
            .map_err(HookInvokeError::from)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/hook/hookers/plugin/session/adaptor_test.rs"]
mod tests;
