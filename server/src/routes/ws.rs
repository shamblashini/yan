use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::time;

use crate::{db, ServerState};
use yan_shared::sync::WsServerMessage;

pub fn router() -> Router<Arc<ServerState>> {
    Router::new().route("/api/ws", get(ws_handler))
}

#[derive(Deserialize)]
struct WsParams {
    /// Bearer token for auth (passed as query param since browser WebSocket API
    /// doesn't support custom headers).
    token: String,
    /// received_at cursor — server will push all ops since this timestamp first.
    #[serde(default)]
    cursor: Option<DateTime<Utc>>,
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<WsParams>,
    State(state): State<Arc<ServerState>>,
) -> impl IntoResponse {
    // Auth check (constant-time)
    if !constant_time_eq(&params.token, &state.auth_token) {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(socket, state, params.cursor))
        .into_response()
}

async fn handle_socket(socket: WebSocket, state: Arc<ServerState>, cursor: Option<DateTime<Utc>>) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.hub.subscribe();

    // Send catch-up ops since the client's cursor
    let (catchup_ops, _) = db::ops_since(&state.pool, cursor).await;
    if !catchup_ops.is_empty() {
        let msg = WsServerMessage::Ops { ops: catchup_ops };
        if let Ok(text) = serde_json::to_string(&msg) {
            let _ = sender.send(Message::Text(text.into())).await;
        }
    }

    let mut ping_interval = time::interval(Duration::from_secs(30));

    loop {
        tokio::select! {
            // Push new ops from broadcast channel to this client
            result = rx.recv() => {
                match result {
                    Ok(ops) => {
                        let msg = WsServerMessage::Ops { ops };
                        if let Ok(text) = serde_json::to_string(&msg) {
                            if sender.send(Message::Text(text.into())).await.is_err() {
                                break; // Client disconnected
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("WebSocket client lagged by {n} messages");
                        // Re-sync: client should reconnect or we could send a full snapshot.
                        // For now, just continue — next poll will catch up via POST /sync.
                    }
                }
            }

            // Keepalive ping
            _ = ping_interval.tick() => {
                if sender.send(Message::Ping(vec![].into())).await.is_err() {
                    break;
                }
            }

            // Drain incoming frames (pong responses, client messages we don't act on)
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {} // Ignore other client messages
                }
            }
        }
    }
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
