use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A single mutation recorded by a client device.
/// Operations are append-only and are the source of truth for sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    /// Globally unique ID for this operation.
    pub op_id: Uuid,
    /// Which device produced this operation.
    pub device_id: Uuid,
    /// Monotonic counter per device, starting at 1.
    pub client_seq: u64,
    /// Wall-clock time when the mutation happened on the client.
    pub happened_at: DateTime<Utc>,
    pub payload: OpPayload,
}

impl Operation {
    pub fn new(device_id: Uuid, client_seq: u64, payload: OpPayload) -> Self {
        Self {
            op_id: Uuid::new_v4(),
            device_id,
            client_seq,
            happened_at: Utc::now(),
            payload,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OpPayload {
    /// Create a new todo item.
    CreateItem {
        item_id: Uuid,
        parent_id: Option<Uuid>,
        /// Sibling index at creation time (hint; ordering is resolved by happened_at).
        position: u32,
        title: String,
        status: String,
    },
    /// Rename a todo item.
    UpdateTitle {
        item_id: Uuid,
        title: String,
    },
    /// Set or clear the description.
    UpdateDescription {
        item_id: Uuid,
        description: Option<String>,
    },
    /// Change status, optionally recursively for all children.
    UpdateStatus {
        item_id: Uuid,
        status: String,
        recursive: bool,
    },
    /// Permanently remove an item and its subtree. Delete wins over concurrent edits.
    DeleteItem {
        item_id: Uuid,
    },
    /// Re-parent or re-order an item.
    MoveItem {
        item_id: Uuid,
        new_parent_id: Option<Uuid>,
        new_position: u32,
    },
    /// Start the timer for an item.
    TimerStart {
        item_id: Uuid,
        started_at: DateTime<Utc>,
    },
    /// Stop the timer; records only this session's elapsed seconds so that
    /// parallel offline sessions accumulate additively on the server.
    TimerStop {
        item_id: Uuid,
        stopped_at: DateTime<Utc>,
        session_secs: i64,
    },
    /// Create or update a custom status definition.
    UpsertStatus {
        name: String,
        color: String,
    },
}
