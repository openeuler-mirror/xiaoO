use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
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
}

fn default_limit() -> usize {
    50
}

/// List all traces
pub async fn list_traces(
    State(state): State<AppState>,
    Query(params): Query<ListTracesQuery>,
) -> Result<Json<Vec<TraceSummary>>, AppError> {
    let traces = if params.alive {
        state.storage.list_alive_traces(params.limit).await?
    } else {
        state.storage.list_traces(params.limit).await?
    };
    Ok(Json(traces))
}

/// Get a specific trace by ID
pub async fn get_trace(
    State(state): State<AppState>,
    Path(trace_id): Path<String>,
) -> Result<Json<Vec<Span>>, AppError> {
    // Try to resolve prefix to full trace_id
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

// Error handling
pub enum AppError {
    Storage(moirai::MoiraiError),
    NotFound(String),
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
        };
        (status, message).into_response()
    }
}
