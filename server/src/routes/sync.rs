use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};

use crate::{db, ServerState};
use yan_shared::sync::{SnapshotResponse, SyncRequest, SyncResponse};

pub fn router(state: Arc<ServerState>) -> Router<Arc<ServerState>> {
    Router::new()
        .route("/api/sync", post(post_sync))
        .route("/api/snapshot", get(get_snapshot))
        .layer(middleware::from_fn_with_state(state, auth_middleware))
}

async fn auth_middleware(
    State(state): State<Arc<ServerState>>,
    req: Request,
    next: Next,
) -> Response {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match token {
        Some(t) if constant_time_eq(t, &state.auth_token) => next.run(req).await,
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

/// Constant-time string comparison to prevent timing attacks.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn post_sync(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<SyncRequest>,
) -> Json<SyncResponse> {
    // 1. Store incoming ops
    let accepted_through_seq = db::store_ops(&state.pool, &req.ops).await;

    // 2. Broadcast to WebSocket subscribers
    if !req.ops.is_empty() {
        state.hub.publish(req.ops.clone());
    }

    // 3. Fetch ops since client's cursor
    let (new_ops, new_cursor) = db::ops_since(&state.pool, req.cursor).await;

    Json(SyncResponse {
        accepted_through_seq,
        new_ops,
        new_cursor,
    })
}

async fn get_snapshot(State(state): State<Arc<ServerState>>) -> Json<SnapshotResponse> {
    let (items, statuses) = db::get_snapshot(&state.pool).await;
    let (_, cursor) = db::ops_since(&state.pool, None).await;
    Json(SnapshotResponse {
        items,
        statuses,
        cursor,
    })
}
