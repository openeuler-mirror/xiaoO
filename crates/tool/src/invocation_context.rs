use std::future::Future;
use std::sync::{Arc, Mutex};

use agent_contracts::backend::{
    OperationBackend, SandboxPermissionGrantId, SandboxPermissionScope,
};

#[derive(Clone)]
struct ToolInvocationContext {
    tool_name: String,
    sandbox_permission_scope: Arc<Mutex<Option<SandboxPermissionScope>>>,
    once_grants: Arc<Mutex<Vec<OnceSandboxGrant>>>,
}

impl ToolInvocationContext {
    fn new(tool_name: String) -> Self {
        Self {
            tool_name,
            sandbox_permission_scope: Arc::new(Mutex::new(None)),
            once_grants: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[derive(Clone)]
struct OnceSandboxGrant {
    backend: Arc<dyn OperationBackend>,
    id: SandboxPermissionGrantId,
}

tokio::task_local! {
    static CURRENT_TOOL_INVOCATION: ToolInvocationContext;
}

pub async fn scope_tool_invocation<F, T>(tool_name: String, future: F) -> T
where
    F: Future<Output = T>,
{
    let context = ToolInvocationContext::new(tool_name);
    CURRENT_TOOL_INVOCATION
        .scope(context, async {
            let output = future.await;
            revoke_current_once_sandbox_grants();
            output
        })
        .await
}

pub fn current_tool_name() -> Option<String> {
    CURRENT_TOOL_INVOCATION
        .try_with(|context| context.tool_name.clone())
        .ok()
}

pub fn current_sandbox_permission_scope() -> Option<SandboxPermissionScope> {
    CURRENT_TOOL_INVOCATION
        .try_with(|context| {
            context
                .sandbox_permission_scope
                .lock()
                .ok()
                .and_then(|scope| *scope)
        })
        .ok()
        .flatten()
}

pub fn approve_current_sandbox_permission(scope: SandboxPermissionScope) -> bool {
    CURRENT_TOOL_INVOCATION
        .try_with(|context| {
            if let Ok(mut approved) = context.sandbox_permission_scope.lock() {
                *approved = Some(scope);
                true
            } else {
                false
            }
        })
        .unwrap_or(false)
}

pub fn register_once_sandbox_grant(
    backend: Arc<dyn OperationBackend>,
    id: SandboxPermissionGrantId,
) -> bool {
    CURRENT_TOOL_INVOCATION
        .try_with(|context| {
            if let Ok(mut grants) = context.once_grants.lock() {
                grants.push(OnceSandboxGrant { backend, id });
                true
            } else {
                false
            }
        })
        .unwrap_or(false)
}

fn revoke_current_once_sandbox_grants() {
    let _ = CURRENT_TOOL_INVOCATION.try_with(|context| {
        let grants = context
            .once_grants
            .lock()
            .map(|mut grants| std::mem::take(&mut *grants))
            .unwrap_or_default();
        for grant in grants {
            if let Some(control) = grant.backend.permission_control() {
                let _ = control.revoke(grant.id);
            }
        }
    });
}
