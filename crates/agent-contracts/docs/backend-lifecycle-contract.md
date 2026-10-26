# Backend Lifecycle Contract

Status: partially implemented. Shared lifecycle interfaces and the lifecycle
state machine live in `src/backend/lifecycle.rs`; the current manager-side
runtime/backend control implementation lives in `apps/shared/src/backend`.

This document defines the unified lifecycle control contract for xiaoO
operation backends. It intentionally lives next to `agent-contracts` because it
describes the shared contract that every backend provider should satisfy before
runtime tools can use the backend's operation plane.

## Goals

- Make backend lifecycle management provider-neutral.
- Move provider-specific actions such as E2B sandbox create and delete
  behind a shared backend lifecycle contract.
- Keep lifecycle control separate from active operation execution.
- Provide enough state, identity, capability, and error information for session
  resume, pause, cleanup, and diagnostics.
- Allow local, E2B, Docker, SSH, Kubernetes, Firecracker, or future providers
  to implement the same control surface.

## Non-Goals

- This document does not replace the existing `OperationBackend` execution
  contract.
- Not every provider must implement the control plane immediately.
- This document does not require all providers to support snapshots, but it
  does require them to report whether they support pause/load semantics.
- This document does not prescribe a storage backend for persisted lifecycle
  records.

## Contract Split

Backend behavior is split into two planes.

| Plane | Responsibility | Existing or Proposed |
| --- | --- | --- |
| Lifecycle control plane | Create, load, pause, delete, inspect backend instances | Proposed by this document |
| Operation plane | Resolve paths, read/write files, search, execute commands, export files | Existing `OperationBackend` |

The lifecycle control plane owns instance state transitions. The operation plane
is only valid when the instance is active.

## Required Lifecycle Interface

Every backend provider must expose the following lifecycle control interface.

```rust
#[async_trait]
pub trait BackendLifecycle: Send + Sync {
    async fn create_sandbox(
        &self,
        request: BackendCreateRequest,
    ) -> Result<BackendInstance, BackendControlError>;

    async fn load(
        &self,
        request: BackendLoadRequest,
    ) -> Result<BackendInstance, BackendControlError>;

    async fn pause(
        &self,
        request: BackendPauseRequest,
    ) -> Result<BackendSnapshot, BackendControlError>;

    async fn delete(
        &self,
        request: BackendDeleteRequest,
    ) -> Result<BackendDeleteOutcome, BackendControlError>;

    async fn inspect(
        &self,
        request: BackendInspectRequest,
    ) -> Result<BackendInstanceStatus, BackendControlError>;
}
```

`inspect` is required because manager-side cached state can drift from provider
truth after process crashes, network failures, or partial lifecycle operations.

## Naming

The low-level `agent-contracts` lifecycle trait still exposes the historical
method name `create_sandbox`. At the manager-facing layer, the current xiaoO API
uses backend names such as `build_backend`, `checkpoint_backend`,
`checkout_backend`, `delete_backend_instance`, `BackendInfo`, and
`BackendCheckpointRef`.

Treat `create_sandbox` here as a provider-lifecycle boundary name, not as the
runtime control-plane vocabulary. Provider adapters may also use sandbox
terminology internally when the provider API itself is sandbox-shaped, such as
E2B snapshots. That provider-native vocabulary should
not leak into runtime checkpoint request/response shapes.

## Lifecycle State

```rust
pub enum BackendLifecycleState {
    Unknown,
    Creating,
    Active,
    Pausing,
    Paused,
    Loading,
    Deleting,
    Deleted,
    Failed,
}
```

| State | Meaning | Operation plane usable | Expected next states |
| --- | --- | --- | --- |
| `Unknown` | Manager does not have reliable provider state | No | `inspect` -> any concrete state |
| `Creating` | A new instance is being provisioned | No | `Active`, `Failed` |
| `Active` | Instance is alive and can serve operation calls | Yes | `Pausing`, `Deleting`, `Failed` |
| `Pausing` | Instance is being frozen or snapshotted | No | `Paused`, `Failed` |
| `Paused` | Instance is stopped, frozen, or represented by a snapshot | No | `Loading`, `Deleting` |
| `Loading` | Instance is being restored from a snapshot or handle | No | `Active`, `Failed` |
| `Deleting` | Instance resources are being destroyed | No | `Deleted`, `Failed` |
| `Deleted` | Instance resources are gone; terminal state | No | None |
| `Failed` | Last lifecycle operation failed or provider reports failure | No by default | `inspect`, retry operation, `delete` |

## State Transition Rules

| From | Operation | To | Notes |
| --- | --- | --- | --- |
| None | `create_sandbox` | `Creating` -> `Active` | Creates a new backend instance |
| `Paused` | `load` | `Loading` -> `Active` | Restores from snapshot or resumable handle |
| `Active` | `pause` | `Pausing` -> `Paused` | Returns a `BackendSnapshot` |
| `Active` | `delete` | `Deleting` -> `Deleted` | Destroys live resources |
| `Paused` | `delete` | `Deleting` -> `Deleted` | Deletes paused/snapshot resources where supported |
| `Failed` | `inspect` | concrete state | Reconciles manager cache with provider truth |
| `Unknown` | `inspect` | concrete state | Used after restart or lost manager state |

Invalid transitions should return `BackendControlError::InvalidState`.

## Identity Model

```rust
pub struct BackendId(pub String);

pub struct BackendProviderKind(pub String);

pub struct BackendInstanceId(pub String);

pub struct BackendSnapshotId(pub String);
```

| Field | Meaning | Stability |
| --- | --- | --- |
| `backend_id` | Stable manager-facing id for one backend instance record | Stable for the record |
| `provider` | Provider kind, e.g. `local`, `e2b`, `docker` | Stable |
| `instance_id` | Provider-native live instance id, e.g. E2B sandbox id | May change after load |
| `snapshot_id` | Provider-native paused/snapshot id | Stable for a snapshot |
| `session_id` | xiaoO session that owns the backend | Stable for session-scoped backends |

## Core Data Types

### BackendInstance

```rust
pub struct BackendInstance {
    pub backend_id: BackendId,
    pub provider: BackendProviderKind,
    pub instance_id: BackendInstanceId,
    pub session_id: String,
    pub state: BackendLifecycleState,
    pub workspace_root: BackendPath,
    pub endpoint: Option<BackendEndpoint>,
    pub snapshot: Option<BackendSnapshotRef>,
    pub capabilities: BackendRuntimeCapabilities,
    pub resources: BackendResourceAllocation,
    pub metadata: serde_json::Value,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}
```

### BackendInstanceStatus

```rust
pub struct BackendInstanceStatus {
    pub backend_id: BackendId,
    pub provider: BackendProviderKind,
    pub instance_id: Option<BackendInstanceId>,
    pub state: BackendLifecycleState,
    pub endpoint: Option<BackendEndpoint>,
    pub snapshot: Option<BackendSnapshotRef>,
    pub last_error: Option<String>,
    pub metadata: serde_json::Value,
    pub updated_at_ms: u64,
}
```

### BackendSnapshot

```rust
pub struct BackendSnapshot {
    pub snapshot_id: BackendSnapshotId,
    pub provider: BackendProviderKind,
    pub source_backend_id: BackendId,
    pub source_instance_id: BackendInstanceId,
    pub state: BackendLifecycleState,
    pub metadata: serde_json::Value,
    pub created_at_ms: u64,
}
```

### BackendSnapshotRef

```rust
pub struct BackendSnapshotRef {
    pub snapshot_id: BackendSnapshotId,
    pub provider: BackendProviderKind,
    pub metadata: serde_json::Value,
}
```

### BackendEndpoint

```rust
pub enum BackendEndpoint {
    Local,
    Tcp {
        host: String,
        port: u16,
    },
    UnixSocket {
        path: String,
    },
    ProviderHandle {
        value: serde_json::Value,
    },
}
```

### BackendRuntimeCapabilities

```rust
pub struct BackendRuntimeCapabilities {
    pub supports_exec: bool,
    pub supports_file_read: bool,
    pub supports_file_write: bool,
    pub supports_search: bool,
    pub supports_export_file: bool,
    pub supports_lsp: bool,
    pub supports_pause: bool,
    pub supports_snapshot: bool,
    pub supports_delete: bool,
}
```

### BackendResourceLimits

```rust
pub struct BackendResourceLimits {
    pub vcpu_count: Option<u32>,
    pub memory_mb: Option<u64>,
    pub disk_mb: Option<u64>,
    pub timeout_ms: Option<u64>,
}
```

### BackendResourceAllocation

```rust
pub struct BackendResourceAllocation {
    pub vcpu_count: Option<u32>,
    pub memory_mb: Option<u64>,
    pub disk_mb: Option<u64>,
}
```

## Request Types

### BackendCreateRequest

```rust
pub struct BackendCreateRequest {
    pub requested_backend_id: Option<BackendId>,
    pub session_id: String,
    pub conversation_id: Option<String>,
    pub workspace_root: BackendPath,
    pub provider_options: serde_json::Value,
    pub resource_limits: BackendResourceLimits,
    pub metadata: serde_json::Value,
}
```

Creates a new backend instance. `requested_backend_id` optionally specifies the
desired backend id; when omitted, the provider generates one.

### BackendLoadRequest

```rust
pub struct BackendLoadRequest {
    pub session_id: String,
    pub workspace_root: BackendPath,
    pub source: BackendLoadSource,
    pub provider_options: serde_json::Value,
    pub resource_limits: BackendResourceLimits,
    pub metadata: serde_json::Value,
}

pub enum BackendLoadSource {
    SnapshotId(BackendSnapshotId),
    InstanceId(BackendInstanceId),
    SerializedHandle(serde_json::Value),
}
```

Loads or restores an instance from a snapshot, a known provider instance id, or
a provider-specific serialized handle.

### BackendPauseRequest

```rust
pub struct BackendPauseRequest {
    pub backend_id: BackendId,
    pub instance_id: BackendInstanceId,
    pub mode: BackendPauseMode,
    pub reason: BackendLifecycleReason,
    pub metadata: serde_json::Value,
}

pub enum BackendPauseMode {
    SnapshotAndStop,
    FreezeInPlace,
    BestEffort,
}
```

Pauses an active instance. Providers that cannot create a durable snapshot must
return `UnsupportedCapability` unless `BestEffort` is explicitly allowed.

### BackendDeleteRequest

```rust
pub struct BackendDeleteRequest {
    pub backend_id: BackendId,
    pub instance_id: Option<BackendInstanceId>,
    pub snapshot_id: Option<BackendSnapshotId>,
    pub force: bool,
    pub reason: BackendLifecycleReason,
    pub metadata: serde_json::Value,
}
```

Deletes live instance resources, paused resources, or both. Providers should be
idempotent: deleting an already-deleted instance should return a successful
`BackendDeleteOutcome` with `already_deleted = true`.

### BackendInspectRequest

```rust
pub struct BackendInspectRequest {
    pub backend_id: Option<BackendId>,
    pub instance_id: Option<BackendInstanceId>,
    pub snapshot_id: Option<BackendSnapshotId>,
    pub metadata: serde_json::Value,
}
```

At least one id field must be present.

### BackendLifecycleReason

```rust
pub enum BackendLifecycleReason {
    SessionOpen,
    SessionResume,
    SessionIdle,
    SessionClose,
    DaemonShutdown,
    UserRequested,
    ErrorCleanup,
    ManagerReconcile,
}
```

## Return Types

### BackendDeleteOutcome

```rust
pub struct BackendDeleteOutcome {
    pub backend_id: BackendId,
    pub instance_id: Option<BackendInstanceId>,
    pub snapshot_id: Option<BackendSnapshotId>,
    pub state: BackendLifecycleState,
    pub already_deleted: bool,
    pub metadata: serde_json::Value,
}
```

## Error Model

```rust
pub enum BackendLifecycleOperation {
    CreateSandbox,
    Load,
    Pause,
    Delete,
    Inspect,
}

pub enum BackendControlError {
    InvalidRequest { message: String },
    InvalidState {
        current: Option<BackendLifecycleState>,
        requested_operation: BackendLifecycleOperation,
        message: String,
    },
    UnsupportedCapability {
        provider: BackendProviderKind,
        capability: String,
    },
    NotFound {
        provider: BackendProviderKind,
        id: String,
    },
    ProviderError {
        provider: BackendProviderKind,
        message: String,
    },
    Timeout {
        operation: BackendLifecycleOperation,
        timeout_ms: u64,
    },
    Transport {
        message: String,
    },
}
```

`current` is `None` when the manager has no recorded state for the target.
`requested_operation` and `operation` are typed lifecycle operations, not
strings.

Errors should preserve provider-native details in `message` or `metadata`, but
callers should make control-flow decisions using the structured variant.

## Capability Requirements

All providers must implement all lifecycle methods. Providers that cannot
perform an operation must return `UnsupportedCapability`; they should not omit
the method.

| Method | Required method | Capability may be unsupported |
| --- | --- | --- |
| `create_sandbox` | Yes | No for any provider used by session runtime |
| `load` | Yes | Yes, if provider does not support resume |
| `pause` | Yes | Yes, if provider does not support pause/snapshot |
| `delete` | Yes | No for any provider that allocates resources |
| `inspect` | Yes | No |

## Lifecycle Policy

The session/backend manager decides which lifecycle operation to call. The
provider executes the requested operation and reports the result.

| Situation | Manager policy |
| --- | --- |
| New session with external backend | `create_sandbox` |
| Resume session with stored snapshot | `load` |
| TUI temporary session exit | `delete` |
| Long-running remote session idle | `pause` |
| Explicit user close | `delete` |
| Daemon graceful shutdown | Configurable: `pause` or `delete` |
| Create health check failure | `delete` cleanup |
| Pause failure | `inspect`, then retry or `delete` |
| Lost manager cache | `inspect` |

## Manager Record

The manager should persist enough information to reconcile and resume backend
instances across process restarts.

```rust
pub struct BackendManagerRecord {
    pub backend_id: BackendId,
    pub provider: BackendProviderKind,
    pub session_id: String,
    pub instance_id: Option<BackendInstanceId>,
    pub snapshot_id: Option<BackendSnapshotId>,
    pub state: BackendLifecycleState,
    pub workspace_root: BackendPath,
    pub endpoint: Option<BackendEndpoint>,
    pub capabilities: BackendRuntimeCapabilities,
    pub metadata: serde_json::Value,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub last_error: Option<String>,
}
```

## Provider Mapping Table

| Provider | `create_sandbox` | `load` | `pause` | `delete` | `inspect` |
| --- | --- | --- | --- | --- | --- |
| `local` | Return an active local instance record | Return active local instance if workspace is valid | Return `UnsupportedCapability` | No-op successful delete | Check workspace path and report active/failed |
| `e2b` | Create or connect an E2B sandbox-backed backend | Checkout from a provider snapshot id where supported | Create provider snapshot via the E2B snapshot API | Delete provider sandbox resources where supported | Query provider status where supported |
| `docker` future | Create container | Start container or restore named container | Stop or commit container | Remove container | Inspect container |
| `ssh` future | Establish session record | Reattach to host/session | Usually unsupported | No-op or close session lease | Probe host/session |

## Operation Plane Attachment

After lifecycle `create_sandbox` or `load` returns an active instance, the
manager attaches the operation plane.

```rust
#[async_trait]
pub trait BackendProvider: Send + Sync {
    fn kind(&self) -> BackendProviderKind;

    fn lifecycle(&self) -> Arc<dyn BackendLifecycle>;

    async fn attach(
        &self,
        instance: BackendInstance,
    ) -> Result<Arc<dyn OperationBackend>, BackendControlError>;
}
```

Attachment must fail unless the instance state is `Active`.

## Implementation Status

All contract types in this document are implemented in
`crates/agent-contracts/src/backend/lifecycle.rs`: the `BackendLifecycle` and
`BackendProvider` traits, the `BackendLifecycleState` and
`BackendLifecycleOperation` enums, the `BackendLifecycleStateMachine`, the
request/response/record types (`BackendCreateRequest`, `BackendLoadRequest`,
`BackendPauseRequest`, `BackendDeleteRequest`, `BackendInspectRequest`,
`BackendInstance`, `BackendInstanceStatus`, `BackendSnapshot`,
`BackendSnapshotRef`, `BackendManagerRecord`), and the `BackendControlError`
error model. The manager-side runtime lives in `apps/shared/src/backend`,
with `local` and `e2b` providers.

## Open Decisions

These are intentionally left for implementation planning.

| Decision | Options |
| --- | --- |
| Method name | Keep `create_sandbox` at the low-level provider lifecycle boundary, or rename it to provider-neutral `create` in a later code cleanup |
| Snapshot storage | Provider-owned, manager-owned metadata, or mixed |
| Manager persistence | In-memory only, session store extension, or dedicated backend store |
| Delete idempotency window | Provider-specific or manager-enforced |
| OperationBackend shutdown | Keep as best-effort cleanup, or route all release through lifecycle delete/pause |
