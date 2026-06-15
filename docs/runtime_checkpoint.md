# Runtime Checkpoint Control

This document records the current control-plane model for runtime checkpoint and
checkout.

## Layering

The external API is runtime-oriented. Callers control a runtime object by using
`runtime_id` and `checkpoint_id`; they do not need to know which session or
backend instance is carrying that runtime.

Internally, the current v1 implementation uses this layering:

| Layer | Current role | External exposure |
| --- | --- | --- |
| Runtime | Control-plane object used for checkpoint and checkout | Exposed as `runtime_id` and `RuntimeRecord` |
| Session | Gateway execution state, conversation identity, lifecycle state, handle state, and resolved agent runtime snapshot | Mostly hidden behind runtime APIs; v1 uses `runtime_id == session_id` |
| Backend | Operation substrate for file, exec, search, export, and provider snapshots | Internal; backend ids are not returned by `RuntimeRecord` |

`RuntimeRecord` is derived from `SessionRecord`:

```rust
pub struct RuntimeRecord {
    pub runtime_id: String,
    pub conversation_id: String,
    pub sender_id: String,
    pub status: SessionLifecycleStatus,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}
```

In v1, `runtime_id` is the same value as `SessionRecord.session_id`. The
backend binding is kept inside `SessionRecord.backend_instance` and the
`BackendManager` session index, which maps a session/runtime id to a backend id.
This keeps backend placement and provider-native ids out of the public runtime
record.

## Public Runtime API

The daemon exposes runtime checkpoint control through the protected HTTP API.
Bearer authentication applies when `[http]` auth is configured.

| Endpoint | Request | Response | Meaning |
| --- | --- | --- | --- |
| `POST /api/v1/runtimes/checkpoint` | `RuntimeCheckpointRequest` | `RuntimeCheckpointResult` | Capture the current idle runtime as a checkpoint |
| `POST /api/v1/runtimes/checkpoint/delete-snapshot` | `RuntimeCheckpointSnapshotDeleteRequest` | `RuntimeCheckpointSnapshotDeleteResult` | Delete the provider snapshot/template referenced by a checkpoint |
| `POST /api/v1/runtimes/checkout` | `RuntimeCheckoutRequest` | `RuntimeCheckoutResult` | Create a new runtime from an existing checkpoint |

`RuntimeCheckpointRequest` contains `runtime_id`, optional `name`, and optional
`metadata`. The result returns a generated `checkpoint_id`, the public
`RuntimeRecord`, and the previous checkpoint head for the same runtime when one
exists.

`RuntimeCheckoutRequest` contains `checkpoint_id`, optional `conversation_id`,
optional `sender_id`, and optional `metadata`. Checkout always creates a new
runtime branch in v1. The caller cannot provide the child runtime id; the service
generates it from the source runtime id plus a UUID suffix.

`RuntimeCheckpointSnapshotDeleteRequest` contains `checkpoint_id`. The service
looks up the checkpoint's internal `BackendCheckpointRef` and deletes only the
provider snapshot recorded there; callers cannot provide an arbitrary provider
template id. The checkpoint record remains in the process-local store, but its
provider snapshot id is cleared after successful or already-deleted provider
cleanup.

## Runtime Checkpoint Shape

The in-memory checkpoint store records:

- `checkpoint_id`
- `runtime_id`
- `parent_checkpoint_id`
- a `SessionRecord` snapshot
- an optional `BackendCheckpointRef`
- creation time, name, and metadata

The store is process-local in v1. Restarting the daemon loses runtime
checkpoints unless a later persistence layer is added.

Checkpoint and checkout require the source runtime to be idle. A running or busy
session returns `SessionBusy` because the session snapshot and backend snapshot
must describe a stable point in time.

## Backend Checkpoint Behavior

Backend checkpointing is internal to runtime checkpointing.

`BackendManager::checkpoint_backend` accepts either a `backend_id` or a
`session_id`. Runtime checkpoint uses the session/runtime id path so it can stay
above the backend layer.

For E2B backends, checkpoint creates a provider snapshot by calling the E2B
snapshot API and stores the returned provider snapshot id inside
`BackendCheckpointRef.provider_snapshot_id`. Checkout uses that snapshot id to
create a new E2B-backed backend for the child runtime.

Providers without snapshot checkout support return `UnsupportedBackend` for
checkout. The current v1 implementation does not implement local restore.

## E2B Sandbox Timing

E2B runtime checkpoint and checkout include provider-side sandbox work:

| Operation | Provider work | Notes |
| --- | --- | --- |
| `POST /api/v1/runtimes/checkpoint` | Calls the E2B snapshot API for the source sandbox when the backend is dirty or has no reusable checkpoint | Clean backends with an existing checkpoint can reuse the prior `BackendCheckpointRef` and avoid a new provider snapshot |
| `POST /api/v1/runtimes/checkout` | Starts a new E2B sandbox from the provider snapshot and binds it to the child runtime id | The child runtime receives a generated id; callers cannot provide it in v1 |
| `POST /api/v1/runtimes/checkpoint/delete-snapshot` | Deletes the E2B snapshot/template by calling the E2B delete-template API | This is explicit cleanup for snapshots the caller no longer needs for future checkout |
| `POST /api/v1/runtimes/close` | Releases the runtime backend and deletes the E2B sandbox when no runtimes remain bound to that backend | Close time is separate from checkpoint/checkout timing |

Use `curl -w '%{time_total}'` around the checkpoint and checkout requests when
measuring from a client. That value is end-to-end HTTP latency as observed by the
caller; it includes daemon bookkeeping and E2B provider calls, but not the LLM
turns needed to prepare or validate the runtime state.

One local smoke run against an E2B backend on 2026-06-13 measured:

| Operation | Measured `time_total` |
| --- | ---: |
| Runtime checkpoint | `1.516061s` |
| Runtime checkout | `2.335461s` |

These numbers are operational examples rather than SLA values. They can vary
with E2B service latency, network path, snapshot size, template cold/warm state,
and daemon host load.

## Dirty Tracking

Backend dirty tracking lives in `apps/shared/src/backend/dirty_write.rs`. It is
a conservative operation-level invalidation wrapper, not a file diff engine.

The backend is marked dirty after these successful operations:

- `write_bytes`
- `create_dir_all`
- `temp_path`
- `exec`, once it returns `Ok(ExecResult)`, even if the process exit code is
  non-zero

Read-only operations do not mark the backend dirty:

- `stat`
- `read_bytes`
- `search`
- `export`

If a backend is clean and already has a `BackendCheckpointRef`, a new backend
checkpoint reuses that ref. If it is dirty or has no previous checkpoint ref,
the manager creates a new backend checkpoint and clears the dirty flag.

## Checkout Versus Resume

`checkout_runtime` and `resume_session` are different operations:

| Operation | Input | Creates new id | Backend action | Use case |
| --- | --- | --- | --- | --- |
| `checkout_runtime` | `checkpoint_id` | Yes, a generated child `runtime_id` | Optional backend checkout from `BackendCheckpointRef` | Branch from a stable checkpoint |
| `resume_session` | existing `session_id` | No | No checkpoint or backend checkout | Reattach to an existing live session handle |

Checkout is non-destructive in v1. It copies the checkpointed session context,
optionally overrides `conversation_id` and `sender_id`, binds a checked-out
backend when the checkpoint has one, saves the child session, and registers the
checkpoint as the child runtime head.

Resume only returns the current snapshot of an existing session handle. It does
not create a checkpoint, does not create a branch, and does not change backend
placement.

## Compatibility Notes

`SessionForkRequest`, `SessionForkResult`, and backend `fork_backend` shapes
still exist for compatibility and tests. Conceptually, fork is now modeled as:

```text
checkpoint runtime -> checkout runtime
```

The HTTP router currently exposes the runtime checkpoint/checkout routes. New
callers should use runtime checkpoint APIs directly instead of relying on a
session-level fork concept.

## Module Layout

- Runtime checkpoint API types live at `apps/shared/src/runtime_checkpoint.rs`
  and are re-exported from `xiaoo_shared` crate root.
- Session-scoped runtime assembly and resolution live under
  `apps/shared/src/gateway/session_runtime/`.
- Backend manager types live under `apps/shared/src/backend/`; shared backend
  request/result shapes are grouped in `apps/shared/src/backend/base.rs`.
- Provider-specific code may still use provider-native words such as sandbox
  when talking to E2B or Conch. The xiaoO manager-facing layer uses backend and
  runtime terminology.
