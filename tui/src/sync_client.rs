/// Background sync task: pushes local ops to the server and pulls remote ops.
///
/// Architecture:
/// - Runs as a tokio task, separate from the main Ratatui event loop.
/// - Communicates with AppState via two channels:
///     local_op_rx:  receives Operation values emitted by the TUI mutations.
///     remote_op_tx: sends Vec<Operation> (server ops) back to the TUI.
///     status_tx:    sends SyncStatus updates for the status bar.
/// - HTTP POST /sync for pushing/pulling ops (500ms debounce).
/// - WebSocket /api/ws for receiving live pushes from the server.
/// - Exponential backoff on network errors (1s → 2s → 4s … capped at 60s).
use std::path::PathBuf;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::{mpsc, watch};
use tokio::time::{self, Instant};
use tokio_tungstenite::tungstenite::Message;

use rusqlite::Connection;
use yan_shared::ops::Operation;
use yan_shared::sync::{SnapshotResponse, SyncRequest, SyncResponse, WsServerMessage};

use crate::config::Config;
use crate::storage;

#[derive(Debug, Clone)]
pub enum SyncStatus {
    /// No server configured.
    Disabled,
    /// Connected and up to date.
    Connected,
    /// Actively pushing or pulling.
    Syncing,
    /// Network unreachable; n ops are pending.
    Offline { pending_ops: usize },
}

pub async fn run(
    config: Config,
    db_path: PathBuf,
    mut local_op_rx: mpsc::Receiver<Operation>,
    remote_op_tx: mpsc::Sender<Vec<Operation>>,
    status_tx: watch::Sender<SyncStatus>,
) {
    if !config.is_sync_configured() {
        let _ = status_tx.send(SyncStatus::Disabled);
        return;
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to build HTTP client");

    let base_url = config.server_url.trim_end_matches('/').to_string();
    let auth_header = format!("Bearer {}", config.auth_token);

    // Open our own DB connection for reading/writing sync state.
    let db = Connection::open(&db_path).expect("sync task: failed to open DB");

    // Bootstrap: if local snapshot is empty, fetch a full snapshot from server.
    let has_data = {
        db.query_row("SELECT COUNT(*) FROM snapshot", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0)
            > 0
    };
    if !has_data {
        let _ = status_tx.send(SyncStatus::Syncing);
        match fetch_snapshot(&client, &base_url, &auth_header).await {
            Ok(snap) => {
                import_snapshot(&db, snap);
                // Notify TUI to reload — send an empty vec as a signal.
                // (In practice, the TUI will pick up data from the DB on next startup;
                //  for a live reload, the TUI would need to handle this. Omitted for now.)
            }
            Err(e) => {
                tracing_simple(format!("Bootstrap snapshot failed: {e}"));
            }
        }
    }

    // Spawn WebSocket listener
    let remote_op_tx_ws = remote_op_tx.clone();
    let status_tx_ws = status_tx.clone();
    let ws_url = format!(
        "{}/api/ws?token={}&cursor=",
        base_url.replace("http://", "ws://").replace("https://", "wss://"),
        config.auth_token,
    );
    tokio::spawn(ws_listener(ws_url, remote_op_tx_ws, status_tx_ws));

    // Main sync loop: collect local ops, debounce, POST /sync
    let debounce = Duration::from_millis(500);
    let mut pending_local: Vec<Operation> = Vec::new();
    let mut backoff_secs: u64 = 1;
    let mut last_sync_attempt: Option<Instant> = None;

    loop {
        // Drain all available local ops (non-blocking)
        loop {
            match local_op_rx.try_recv() {
                Ok(op) => pending_local.push(op),
                Err(_) => break,
            }
        }

        let should_sync = if pending_local.is_empty() {
            // Nothing pending — wait for either a new op or a 30s heartbeat
            match time::timeout(Duration::from_secs(30), local_op_rx.recv()).await {
                Ok(Some(op)) => {
                    pending_local.push(op);
                    // Debounce: wait a bit for more ops to arrive
                    time::sleep(debounce).await;
                    true
                }
                Ok(None) => break, // channel closed, TUI exited
                Err(_) => true, // 30s heartbeat — sync even if no local ops (pull-only)
            }
        } else {
            // We already have ops — respect backoff before retrying
            if let Some(last) = last_sync_attempt {
                let elapsed = last.elapsed();
                let wait = Duration::from_secs(backoff_secs);
                if elapsed < wait {
                    time::sleep(wait - elapsed).await;
                }
            }
            true
        };

        if !should_sync {
            continue;
        }

        // Drain any more ops that arrived during the debounce/wait
        loop {
            match local_op_rx.try_recv() {
                Ok(op) => pending_local.push(op),
                Err(_) => break,
            }
        }

        let cursor = storage::get_sync_state(&db, "server_cursor")
            .and_then(|s| s.parse().ok());

        let pending_count = count_unsynced(&db);
        let _ = status_tx.send(SyncStatus::Syncing);
        last_sync_attempt = Some(Instant::now());

        let req = SyncRequest {
            device_id: config.device_id,
            cursor,
            ops: pending_local.clone(),
        };

        match post_sync(&client, &base_url, &auth_header, req).await {
            Ok(resp) => {
                backoff_secs = 1; // reset on success

                // Mark local ops as synced
                if resp.accepted_through_seq > 0 {
                    storage::mark_ops_synced(&db, resp.accepted_through_seq);
                }
                pending_local.clear();

                // Update cursor
                storage::set_sync_state(
                    &db,
                    "server_cursor",
                    &resp.new_cursor.to_rfc3339(),
                );

                // Apply remote ops to local DB and forward to TUI
                if !resp.new_ops.is_empty() {
                    for op in &resp.new_ops {
                        storage::apply_remote_op(&db, op);
                    }
                    let _ = remote_op_tx.send(resp.new_ops).await;
                }

                let _ = status_tx.send(SyncStatus::Connected);
            }
            Err(e) => {
                tracing_simple(format!("Sync failed: {e}"));
                let _ = status_tx.send(SyncStatus::Offline { pending_ops: pending_count });
                backoff_secs = (backoff_secs * 2).min(60);
            }
        }
    }
}

async fn post_sync(
    client: &reqwest::Client,
    base_url: &str,
    auth_header: &str,
    req: SyncRequest,
) -> Result<SyncResponse, String> {
    client
        .post(format!("{base_url}/api/sync"))
        .header("Authorization", auth_header)
        .json(&req)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json::<SyncResponse>()
        .await
        .map_err(|e| e.to_string())
}

async fn fetch_snapshot(
    client: &reqwest::Client,
    base_url: &str,
    auth_header: &str,
) -> Result<SnapshotResponse, String> {
    client
        .get(format!("{base_url}/api/snapshot"))
        .header("Authorization", auth_header)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json::<SnapshotResponse>()
        .await
        .map_err(|e| e.to_string())
}

fn import_snapshot(db: &Connection, snap: SnapshotResponse) {
    // Clear existing snapshot and import from server
    db.execute("DELETE FROM snapshot", []).ok();
    db.execute("DELETE FROM statuses", []).ok();

    for s in &snap.statuses {
        db.execute(
            "INSERT OR IGNORE INTO statuses (name, color) VALUES (?1, ?2)",
            rusqlite::params![s.name, s.color],
        )
        .ok();
    }

    // Flatten the tree and insert all items
    fn insert_items(db: &Connection, items: &[yan_shared::models::TodoItem], parent_id: Option<uuid::Uuid>, pos: i64) {
        for (i, item) in items.iter().enumerate() {
            let position = pos + i as i64;
            db.execute(
                "INSERT OR REPLACE INTO snapshot
                 (item_id, parent_id, position, title, description, status,
                  accumulated_secs, timer_running_since, created_at, updated_at, is_deleted)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0)",
                rusqlite::params![
                    item.id.to_string(),
                    parent_id.map(|u| u.to_string()),
                    position,
                    item.title,
                    item.description,
                    item.status,
                    item.timer.accumulated_secs,
                    item.timer.running_since.map(|dt| dt.to_rfc3339()),
                    item.created_at.to_rfc3339(),
                    item.updated_at.to_rfc3339(),
                ],
            ).ok();
            insert_items(db, &item.children, Some(item.id), 0);
        }
    }
    insert_items(db, &snap.items, None, 0);

    storage::set_sync_state(db, "server_cursor", &snap.cursor.to_rfc3339());
}

/// WebSocket listener: receives live op pushes from the server.
async fn ws_listener(
    ws_url: String,
    remote_op_tx: mpsc::Sender<Vec<Operation>>,
    status_tx: watch::Sender<SyncStatus>,
) {
    let mut backoff_secs: u64 = 1;
    loop {
        match tokio_tungstenite::connect_async(&ws_url).await {
            Ok((ws_stream, _)) => {
                backoff_secs = 1;
                let (_, mut read) = ws_stream.split();
                loop {
                    match read.next().await {
                        Some(Ok(Message::Text(text))) => {
                            if let Ok(msg) = serde_json::from_str::<WsServerMessage>(&text) {
                                match msg {
                                    WsServerMessage::Ops { ops } if !ops.is_empty() => {
                                        let _ = remote_op_tx.send(ops).await;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Some(Ok(Message::Close(_))) | None => break,
                        _ => {} // Ping/Pong/Binary ignored
                    }
                }
                let _ = status_tx.send(SyncStatus::Offline { pending_ops: 0 });
            }
            Err(_) => {}
        }
        time::sleep(Duration::from_secs(backoff_secs)).await;
        backoff_secs = (backoff_secs * 2).min(60);
    }
}

fn count_unsynced(db: &Connection) -> usize {
    db.query_row("SELECT COUNT(*) FROM local_ops WHERE synced = 0", [], |r| {
        r.get::<_, i64>(0)
    })
    .unwrap_or(0) as usize
}

fn tracing_simple(msg: String) {
    eprintln!("[yan-sync] {msg}");
}
