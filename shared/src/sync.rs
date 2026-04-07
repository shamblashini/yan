use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::{Status, TodoItem};
use crate::ops::Operation;

/// Sent by the client to push local ops and pull server ops since its cursor.
#[derive(Debug, Serialize, Deserialize)]
pub struct SyncRequest {
    pub device_id: Uuid,
    /// `received_at` of the last server op this client has already applied.
    /// None means the client has never synced (use GET /snapshot instead).
    pub cursor: Option<DateTime<Utc>>,
    /// Local ops not yet confirmed by the server.
    pub ops: Vec<Operation>,
}

/// Returned by POST /sync.
#[derive(Debug, Serialize, Deserialize)]
pub struct SyncResponse {
    /// Highest `client_seq` the server stored for this device in this request.
    pub accepted_through_seq: u64,
    /// Ops from other devices (or previously unknown) since the client's cursor.
    pub new_ops: Vec<Operation>,
    /// Updated cursor; store this and send it in the next SyncRequest.
    pub new_cursor: DateTime<Utc>,
}

/// Returned by GET /snapshot — full current state for a fresh device.
#[derive(Debug, Serialize, Deserialize)]
pub struct SnapshotResponse {
    pub items: Vec<TodoItem>,
    pub statuses: Vec<Status>,
    /// Cursor to use in the first SyncRequest after bootstrapping.
    pub cursor: DateTime<Utc>,
}

/// Messages pushed from server to client over WebSocket.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsServerMessage {
    /// New ops from other devices; client should apply them.
    Ops { ops: Vec<Operation> },
}

/// Messages sent from client to server over WebSocket (currently unused; clients push via HTTP).
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsClientMessage {
    Ping,
}
