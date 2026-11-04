use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use moirai::{Span, SpanStorage, SqliteStorage, TraceSummary};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub storage: Arc<SqliteStorage>,
}

#[derive(Deserialize)]
pub struct ListTracesQuery {
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    alive: bool,
    /// Keyset cursor `"{start_time}.{trace_id}"` from the last row of a
    /// previous page; omitted or empty means "start from the newest".
    #[serde(default)]
    cursor: Option<String>,
}

fn default_limit() -> usize {
    50
}

/// Parse a `"{start_time}.{trace_id}"` cursor. `start_time` is an integer and
/// never contains a dot, so splitting on the *first* dot keeps trace IDs
/// intact even if they contain dots themselves. An empty trace-id part is
/// rejected: no real row produces one, and against the keyset comparison
/// (`trace_id < cursor_id`) it would silently skip every row sharing the
/// cursor's start_time.
fn parse_cursor(raw: &str) -> Option<(i64, String)> {
    let (ts, id) = raw.split_once('.')?;
    if id.is_empty() {
        return None;
    }
    Some((ts.parse().ok()?, id.to_string()))
}

#[derive(Serialize)]
pub struct ListTracesResponse {
    pub items: Vec<TraceSummary>,
    /// Continuation cursor `"{start_time}.{trace_id}"` for the next page;
    /// `None` means the list is exhausted. The handler over-fetches by one
    /// row internally so clients can follow this field without probing.
    pub next_cursor: Option<String>,
}

/// List traces, optionally continuing after a keyset cursor.
pub async fn list_traces(
    State(state): State<AppState>,
    Query(params): Query<ListTracesQuery>,
) -> Result<Json<ListTracesResponse>, AppError> {
    // An empty value is treated as "not sent": clients commonly concatenate
    // an initially-empty cursor variable into the URL, and an empty string
    // carries no continuation intent.
    let before = match params
        .cursor
        .as_deref()
        .filter(|c| !c.is_empty())
        .map(parse_cursor)
    {
        Some(Some(cursor)) => Some(cursor),
        // A non-empty but malformed cursor is a client bug: silently ignoring
        // it would restart from the newest trace and look like "missing" data.
        Some(None) => {
            return Err(AppError::BadRequest(format!(
                "Invalid cursor (expected \"{{start_time}}.{{trace_id}}\"): {:?}",
                params.cursor
            )))
        }
        None => None,
    };

    // saturating_add: a hostile `limit=usize::MAX` must not panic on overflow;
    // saturating merely skips the over-fetch for that degenerate value.
    let fetch_limit = params.limit.saturating_add(1);
    let mut traces = if params.alive {
        state.storage.list_alive_traces(fetch_limit, before).await?
    } else {
        state.storage.list_traces(fetch_limit, before).await?
    };
    let next_cursor = if traces.len() > params.limit {
        traces.pop();
        traces
            .last()
            .map(|t| format!("{}.{}", t.start_time, t.trace_id))
    } else {
        None
    };
    Ok(Json(ListTracesResponse {
        items: traces,
        next_cursor,
    }))
}

/// Get a specific trace by ID
pub async fn get_trace(
    State(state): State<AppState>,
    Path(trace_id): Path<String>,
) -> Result<Json<Vec<Span>>, AppError> {
    let resolved_id = state
        .storage
        .get_trace_by_prefix(&trace_id)
        .await?
        .ok_or(AppError::NotFound(format!("Trace not found: {}", trace_id)))?;

    let spans = state
        .storage
        .get_trace_spans_with_related_segments(&resolved_id)
        .await?;
    Ok(Json(spans))
}

/// Get a specific span by ID
pub async fn get_span(
    State(state): State<AppState>,
    Path(span_id): Path<String>,
) -> Result<Json<Span>, AppError> {
    let span = state
        .storage
        .get_span(&span_id)
        .await?
        .ok_or(AppError::NotFound(format!("Span not found: {}", span_id)))?;
    Ok(Json(span))
}

#[derive(Serialize)]
pub struct DeleteResponse {
    pub deleted_count: usize,
}

/// Delete a trace by ID
pub async fn delete_trace(
    State(state): State<AppState>,
    Path(trace_id): Path<String>,
) -> Result<Json<DeleteResponse>, AppError> {
    let resolved_id = state
        .storage
        .get_trace_by_prefix(&trace_id)
        .await?
        .ok_or(AppError::NotFound(format!("Trace not found: {}", trace_id)))?;

    let deleted_count = state.storage.delete_trace(&resolved_id).await?;
    Ok(Json(DeleteResponse { deleted_count }))
}

pub enum AppError {
    Storage(moirai::MoiraiError),
    NotFound(String),
    BadRequest(String),
}

impl From<moirai::MoiraiError> for AppError {
    fn from(err: moirai::MoiraiError) -> Self {
        AppError::Storage(err)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::Storage(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
        };
        (status, message).into_response()
    }
}
