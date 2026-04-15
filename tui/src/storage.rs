use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use rusqlite::{params, Connection, Result as SqlResult};
use uuid::Uuid;
use chrono::{DateTime, Utc};

use yan_shared::models::{Status, TodoItem, TimerState};
use yan_shared::ops::{Operation, OpPayload};

// ── Paths ──────────────────────────────────────────────────────────────────────

pub fn db_path() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".local")
            .join("share")
    });
    base.join("todo").join("todo.db")
}

/// Legacy TOML path — used for one-time migration only.
pub fn legacy_toml_path() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".local")
            .join("share")
    });
    base.join("todo").join("todos.toml")
}

// ── Open & migrate ────────────────────────────────────────────────────────────

pub fn open_db() -> Connection {
    let path = db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let conn = Connection::open(&path).expect("Failed to open SQLite database");
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
        .expect("PRAGMA setup failed");
    run_migrations(&conn);
    conn
}

fn run_migrations(conn: &Connection) {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS snapshot (
            item_id      TEXT PRIMARY KEY,
            parent_id    TEXT,
            position     INTEGER NOT NULL DEFAULT 0,
            title        TEXT NOT NULL DEFAULT '',
            description  TEXT,
            status       TEXT NOT NULL DEFAULT 'Todo',
            accumulated_secs INTEGER NOT NULL DEFAULT 0,
            timer_running_since TEXT,
            created_at   TEXT NOT NULL,
            updated_at   TEXT NOT NULL,
            is_deleted   INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS statuses (
            name  TEXT PRIMARY KEY,
            color TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS local_ops (
            op_id      TEXT PRIMARY KEY,
            client_seq INTEGER NOT NULL UNIQUE,
            happened_at TEXT NOT NULL,
            payload    TEXT NOT NULL,
            synced     INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS sync_state (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS collapse_state (
            item_id TEXT PRIMARY KEY
        );
        ",
    )
    .expect("Migration failed");

    // Incremental migrations
    // Add tags column to snapshot
    conn.execute_batch("ALTER TABLE snapshot ADD COLUMN tags TEXT NOT NULL DEFAULT '[]'")
        .ok();
    // Add tab_id column to snapshot
    let default_tab_id = yan_shared::models::DEFAULT_TAB_ID.to_string();
    conn.execute(
        &format!("ALTER TABLE snapshot ADD COLUMN tab_id TEXT NOT NULL DEFAULT '{}'", default_tab_id),
        [],
    ).ok();
    // Create tabs table
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tabs (
            tab_id   TEXT PRIMARY KEY,
            name     TEXT NOT NULL,
            color    TEXT NOT NULL DEFAULT 'white',
            position INTEGER NOT NULL DEFAULT 0
        )",
    ).expect("tabs table creation failed");
    // Create tag_views table (local only, not synced)
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tag_views (
            name       TEXT PRIMARY KEY,
            tag_filter TEXT NOT NULL
        )",
    ).expect("tag_views table creation failed");
}

// ── Sync state helpers ────────────────────────────────────────────────────────

pub fn get_sync_state(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM sync_state WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .ok()
}

pub fn set_sync_state(conn: &Connection, key: &str, value: &str) {
    conn.execute(
        "INSERT INTO sync_state (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .ok();
}

pub fn load_collapse_state(conn: &Connection) -> HashSet<Uuid> {
    let mut stmt = match conn.prepare("SELECT item_id FROM collapse_state") {
        Ok(s) => s,
        Err(_) => return HashSet::new(),
    };
    stmt.query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .filter_map(|s| Uuid::parse_str(&s).ok())
        .collect()
}

pub fn save_collapse_state(conn: &Connection, collapsed: &HashSet<Uuid>) {
    conn.execute("DELETE FROM collapse_state", []).ok();
    for id in collapsed {
        conn.execute(
            "INSERT INTO collapse_state (item_id) VALUES (?1)",
            params![id.to_string()],
        )
        .ok();
    }
}

// ── Tab helpers ──────────────────────────────────────────────────────────────

use yan_shared::models::Tab;

pub fn load_tabs(conn: &Connection) -> Vec<Tab> {
    let mut stmt = match conn.prepare("SELECT tab_id, name, color, position FROM tabs ORDER BY position") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map([], |row| {
        Ok(Tab {
            id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap_or_else(|_| Uuid::new_v4()),
            name: row.get(1)?,
            color: row.get(2)?,
            position: row.get::<_, i64>(3)? as u32,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn save_tabs(conn: &Connection, tabs: &[Tab]) {
    conn.execute("DELETE FROM tabs", []).ok();
    for tab in tabs {
        conn.execute(
            "INSERT INTO tabs (tab_id, name, color, position) VALUES (?1, ?2, ?3, ?4)",
            params![tab.id.to_string(), tab.name, tab.color, tab.position as i64],
        )
        .ok();
    }
}

// ── View helpers ─────────────────────────────────────────────────────────────

pub fn load_views(conn: &Connection) -> Vec<(String, String)> {
    let mut stmt = match conn.prepare("SELECT name, tag_filter FROM tag_views ORDER BY name") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
}

pub fn save_view(conn: &Connection, name: &str, tag_filter: &str) {
    conn.execute(
        "INSERT OR REPLACE INTO tag_views (name, tag_filter) VALUES (?1, ?2)",
        params![name, tag_filter],
    )
    .ok();
}

pub fn delete_view(conn: &Connection, name: &str) {
    conn.execute("DELETE FROM tag_views WHERE name = ?1", params![name]).ok();
}

pub fn next_client_seq(conn: &Connection) -> u64 {
    let max: Option<i64> = conn
        .query_row("SELECT MAX(client_seq) FROM local_ops", [], |r| r.get(0))
        .ok()
        .flatten();
    (max.unwrap_or(0) + 1) as u64
}

// ── Load state from DB ────────────────────────────────────────────────────────

/// Load the current state: rebuild the todo tree from the snapshot table plus
/// any pending (unsynced) local ops on top.
pub fn load_state(conn: &Connection) -> (Vec<Tab>, HashMap<Uuid, Vec<TodoItem>>, Vec<Status>) {
    // 1. Load statuses
    let statuses = load_statuses(conn);
    if statuses.is_empty() {
        // First-ever run: try migrating from legacy TOML, else seed defaults.
        let seeded = try_migrate_from_toml(conn);
        if seeded {
            return load_state(conn);
        }
        seed_default_statuses(conn);
    }
    let statuses = load_statuses(conn);

    // 2. Load tabs (seed default if none exist)
    let mut tabs = load_tabs(conn);
    if tabs.is_empty() {
        tabs.push(Tab::default_tab());
        save_tabs(conn, &tabs);
    }

    // 3. Load snapshot
    let mut items_map = load_snapshot_map(conn);

    // 4. Apply pending local ops on top (they may not have round-tripped yet)
    let pending_ops = load_pending_ops(conn);
    for op in &pending_ops {
        apply_op_to_map(&mut items_map, op);
    }

    // 5. Build per-tab trees
    let tab_roots = build_tab_trees(items_map, &tabs);

    (tabs, tab_roots, statuses)
}

fn load_statuses(conn: &Connection) -> Vec<Status> {
    let mut stmt = conn
        .prepare("SELECT name, color FROM statuses ORDER BY rowid")
        .unwrap();
    stmt.query_map([], |row| {
        Ok(Status {
            name: row.get(0)?,
            color: row.get(1)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

fn seed_default_statuses(conn: &Connection) {
    for s in Status::defaults() {
        conn.execute(
            "INSERT OR IGNORE INTO statuses (name, color) VALUES (?1, ?2)",
            params![s.name, s.color],
        )
        .ok();
    }
}

/// Flat representation of a snapshot row.
struct SnapshotRow {
    item_id: String,
    parent_id: Option<String>,
    position: i64,
    title: String,
    description: Option<String>,
    status: String,
    tags: String,
    tab_id: String,
    accumulated_secs: i64,
    timer_running_since: Option<String>,
    created_at: String,
    updated_at: String,
    is_deleted: bool,
}

fn load_snapshot_map(conn: &Connection) -> HashMap<String, SnapshotRow> {
    let mut stmt = conn
        .prepare(
            "SELECT item_id, parent_id, position, title, description, status,
                    accumulated_secs, timer_running_since, created_at, updated_at, is_deleted,
                    tags, tab_id
             FROM snapshot
             WHERE is_deleted = 0
             ORDER BY position",
        )
        .unwrap();
    stmt.query_map([], |row| {
        Ok(SnapshotRow {
            item_id: row.get(0)?,
            parent_id: row.get(1)?,
            position: row.get(2)?,
            title: row.get(3)?,
            description: row.get(4)?,
            status: row.get(5)?,
            accumulated_secs: row.get(6)?,
            timer_running_since: row.get(7)?,
            created_at: row.get(8)?,
            updated_at: row.get(9)?,
            is_deleted: {
                let v: i64 = row.get(10)?;
                v != 0
            },
            tags: row.get::<_, String>(11).unwrap_or_else(|_| "[]".to_string()),
            tab_id: row.get::<_, String>(12).unwrap_or_else(|_| yan_shared::models::DEFAULT_TAB_ID.to_string()),
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect::<Vec<_>>()
    .into_iter()
    .map(|r| (r.item_id.clone(), r))
    .collect()
}

fn load_pending_ops(conn: &Connection) -> Vec<Operation> {
    let mut stmt = conn
        .prepare("SELECT op_id, client_seq, happened_at, payload FROM local_ops WHERE synced = 0 ORDER BY client_seq")
        .unwrap();
    stmt.query_map([], |row| {
        let op_id_str: String = row.get(0)?;
        let client_seq: i64 = row.get(1)?;
        let happened_at_str: String = row.get(2)?;
        let payload_str: String = row.get(3)?;
        Ok((op_id_str, client_seq, happened_at_str, payload_str))
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .filter_map(|(op_id_str, client_seq, happened_at_str, payload_str)| {
        let op_id = Uuid::parse_str(&op_id_str).ok()?;
        let happened_at = happened_at_str.parse::<DateTime<Utc>>().ok()?;
        let payload: OpPayload = serde_json::from_str(&payload_str).ok()?;
        // device_id not critical for local replay, use nil uuid
        Some(Operation {
            op_id,
            device_id: Uuid::nil(),
            client_seq: client_seq as u64,
            happened_at,
            payload,
        })
    })
    .collect()
}

fn apply_op_to_map(map: &mut HashMap<String, SnapshotRow>, op: &Operation) {
    match &op.payload {
        OpPayload::CreateItem { item_id, parent_id, position, title, status, tags, tab_id } => {
            let now = op.happened_at.to_rfc3339();
            map.entry(item_id.to_string()).or_insert_with(|| SnapshotRow {
                item_id: item_id.to_string(),
                parent_id: parent_id.map(|u| u.to_string()),
                position: *position as i64,
                title: title.clone(),
                description: None,
                status: status.clone(),
                tags: serde_json::to_string(tags).unwrap_or_else(|_| "[]".to_string()),
                tab_id: tab_id.unwrap_or(yan_shared::models::DEFAULT_TAB_ID).to_string(),
                accumulated_secs: 0,
                timer_running_since: None,
                created_at: now.clone(),
                updated_at: now,
                is_deleted: false,
            });
        }
        OpPayload::UpdateTitle { item_id, title } => {
            if let Some(row) = map.get_mut(&item_id.to_string()) {
                row.title = title.clone();
                row.updated_at = op.happened_at.to_rfc3339();
            }
        }
        OpPayload::UpdateDescription { item_id, description } => {
            if let Some(row) = map.get_mut(&item_id.to_string()) {
                row.description = description.clone();
                row.updated_at = op.happened_at.to_rfc3339();
            }
        }
        OpPayload::UpdateStatus { item_id, status, recursive } => {
            if *recursive {
                // Collect all ids in subtree
                let ids: Vec<String> = collect_subtree_ids(map, &item_id.to_string());
                for id in ids {
                    if let Some(row) = map.get_mut(&id) {
                        row.status = status.clone();
                        row.updated_at = op.happened_at.to_rfc3339();
                    }
                }
            } else if let Some(row) = map.get_mut(&item_id.to_string()) {
                row.status = status.clone();
                row.updated_at = op.happened_at.to_rfc3339();
            }
        }
        OpPayload::DeleteItem { item_id } => {
            // Mark deleted (cascade to children via tree building)
            let ids = collect_subtree_ids(map, &item_id.to_string());
            for id in ids {
                if let Some(row) = map.get_mut(&id) {
                    row.is_deleted = true;
                }
            }
        }
        OpPayload::MoveItem { item_id, new_parent_id, new_position } => {
            if let Some(row) = map.get_mut(&item_id.to_string()) {
                row.parent_id = new_parent_id.map(|u| u.to_string());
                row.position = *new_position as i64;
                row.updated_at = op.happened_at.to_rfc3339();
            }
        }
        OpPayload::TimerStart { item_id, started_at } => {
            if let Some(row) = map.get_mut(&item_id.to_string()) {
                if row.timer_running_since.is_none() {
                    row.timer_running_since = Some(started_at.to_rfc3339());
                }
            }
        }
        OpPayload::TimerStop { item_id, stopped_at: _, session_secs } => {
            if let Some(row) = map.get_mut(&item_id.to_string()) {
                row.accumulated_secs += session_secs;
                row.timer_running_since = None;
            }
        }
        OpPayload::UpdateTags { item_id, tags } => {
            if let Some(row) = map.get_mut(&item_id.to_string()) {
                row.tags = serde_json::to_string(tags).unwrap_or_else(|_| "[]".to_string());
                row.updated_at = op.happened_at.to_rfc3339();
            }
        }
        OpPayload::UpsertStatus { .. } |
        OpPayload::CreateTab { .. } |
        OpPayload::RenameTab { .. } |
        OpPayload::DeleteTab { .. } => {
            // Handled separately; not in the item map
        }
    }
}

fn collect_subtree_ids(map: &HashMap<String, SnapshotRow>, root_id: &str) -> Vec<String> {
    let mut result = vec![root_id.to_string()];
    let children: Vec<String> = map
        .values()
        .filter(|r| r.parent_id.as_deref() == Some(root_id))
        .map(|r| r.item_id.clone())
        .collect();
    for child_id in children {
        result.extend(collect_subtree_ids(map, &child_id));
    }
    result
}

fn build_tab_trees(map: HashMap<String, SnapshotRow>, tabs: &[Tab]) -> HashMap<Uuid, Vec<TodoItem>> {
    let mut all: Vec<SnapshotRow> = map.into_values().filter(|r| !r.is_deleted).collect();
    all.sort_by_key(|r| r.position);
    let mut result = HashMap::new();
    for tab in tabs {
        let tab_id_str = tab.id.to_string();
        // Root items for this tab: items whose tab_id matches AND have no parent (or parent is in a different tab)
        let roots = build_children_for_tab(&all, None, &tab_id_str);
        result.insert(tab.id, roots);
    }
    result
}

fn build_children_for_tab(all: &[SnapshotRow], parent_id: Option<&str>, tab_id: &str) -> Vec<TodoItem> {
    all.iter()
        .filter(|r| {
            r.parent_id.as_deref() == parent_id &&
            (parent_id.is_some() || r.tab_id == tab_id) // Only filter by tab for root items
        })
        .map(|r| {
            let timer = TimerState {
                accumulated_secs: r.accumulated_secs,
                running_since: r.timer_running_since.as_ref().and_then(|s| s.parse().ok()),
            };
            let children = build_children_for_tab(all, Some(&r.item_id), tab_id);
            let tags: Vec<String> = serde_json::from_str(&r.tags).unwrap_or_default();
            TodoItem {
                id: Uuid::parse_str(&r.item_id).unwrap_or_else(|_| Uuid::new_v4()),
                title: r.title.clone(),
                description: r.description.clone(),
                status: r.status.clone(),
                tags,
                children,
                timer,
                created_at: r.created_at.parse().unwrap_or_else(|_| Utc::now()),
                updated_at: r.updated_at.parse().unwrap_or_else(|_| Utc::now()),
            }
        })
        .collect()
}


// ── Write ops ─────────────────────────────────────────────────────────────────

/// Persist a local operation: write to local_ops and update the snapshot.
pub fn write_op(conn: &Connection, op: &Operation) {
    let payload_json = serde_json::to_string(&op.payload).unwrap_or_default();
    conn.execute(
        "INSERT OR IGNORE INTO local_ops (op_id, client_seq, happened_at, payload, synced)
         VALUES (?1, ?2, ?3, ?4, 0)",
        params![
            op.op_id.to_string(),
            op.client_seq as i64,
            op.happened_at.to_rfc3339(),
            payload_json,
        ],
    )
    .ok();
    update_snapshot(conn, op);
}

/// Mark all local ops with client_seq <= through_seq as confirmed by the server.
pub fn mark_ops_synced(conn: &Connection, through_seq: u64) {
    conn.execute(
        "UPDATE local_ops SET synced = 1 WHERE client_seq <= ?1 AND synced = 0",
        params![through_seq as i64],
    )
    .ok();
}

pub fn delete_status(conn: &Connection, name: &str) {
    conn.execute("DELETE FROM statuses WHERE name = ?1", params![name]).ok();
}

/// Apply a server-returned op to the local snapshot.
/// Skips ops whose op_id is already in local_ops (already applied locally).
pub fn apply_remote_op(conn: &Connection, op: &Operation) {
    // Skip if we produced this op ourselves
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM local_ops WHERE op_id = ?1",
            params![op.op_id.to_string()],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false);
    if exists {
        return;
    }
    update_snapshot(conn, op);
}

fn update_snapshot(conn: &Connection, op: &Operation) {
    match &op.payload {
        OpPayload::CreateItem { item_id, parent_id, position, title, status, tags, tab_id } => {
            let now = op.happened_at.to_rfc3339();
            let tags_json = serde_json::to_string(tags).unwrap_or_else(|_| "[]".to_string());
            let tid = tab_id.unwrap_or(yan_shared::models::DEFAULT_TAB_ID).to_string();
            conn.execute(
                "INSERT OR IGNORE INTO snapshot
                 (item_id, parent_id, position, title, status, tags, tab_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    item_id.to_string(),
                    parent_id.map(|u| u.to_string()),
                    *position as i64,
                    title,
                    status,
                    tags_json,
                    tid,
                    now,
                    now,
                ],
            )
            .ok();
        }
        OpPayload::UpdateTitle { item_id, title } => {
            conn.execute(
                "UPDATE snapshot SET title = ?1, updated_at = ?2 WHERE item_id = ?3",
                params![title, op.happened_at.to_rfc3339(), item_id.to_string()],
            )
            .ok();
        }
        OpPayload::UpdateDescription { item_id, description } => {
            conn.execute(
                "UPDATE snapshot SET description = ?1, updated_at = ?2 WHERE item_id = ?3",
                params![description, op.happened_at.to_rfc3339(), item_id.to_string()],
            )
            .ok();
        }
        OpPayload::UpdateStatus { item_id, status, recursive } => {
            if *recursive {
                update_status_recursive_in_db(conn, &item_id.to_string(), status, &op.happened_at);
            } else {
                conn.execute(
                    "UPDATE snapshot SET status = ?1, updated_at = ?2 WHERE item_id = ?3",
                    params![status, op.happened_at.to_rfc3339(), item_id.to_string()],
                )
                .ok();
            }
        }
        OpPayload::DeleteItem { item_id } => {
            delete_subtree_in_db(conn, &item_id.to_string());
        }
        OpPayload::MoveItem { item_id, new_parent_id, new_position } => {
            conn.execute(
                "UPDATE snapshot SET parent_id = ?1, position = ?2, updated_at = ?3 WHERE item_id = ?4",
                params![
                    new_parent_id.map(|u| u.to_string()),
                    *new_position as i64,
                    op.happened_at.to_rfc3339(),
                    item_id.to_string(),
                ],
            )
            .ok();
        }
        OpPayload::TimerStart { item_id, started_at } => {
            conn.execute(
                "UPDATE snapshot SET timer_running_since = ?1 WHERE item_id = ?2 AND timer_running_since IS NULL",
                params![started_at.to_rfc3339(), item_id.to_string()],
            )
            .ok();
        }
        OpPayload::TimerStop { item_id, stopped_at: _, session_secs } => {
            conn.execute(
                "UPDATE snapshot SET accumulated_secs = accumulated_secs + ?1, timer_running_since = NULL WHERE item_id = ?2",
                params![session_secs, item_id.to_string()],
            )
            .ok();
        }
        OpPayload::UpdateTags { item_id, tags } => {
            let tags_json = serde_json::to_string(tags).unwrap_or_else(|_| "[]".to_string());
            conn.execute(
                "UPDATE snapshot SET tags = ?1, updated_at = ?2 WHERE item_id = ?3",
                params![tags_json, op.happened_at.to_rfc3339(), item_id.to_string()],
            )
            .ok();
        }
        OpPayload::CreateTab { tab_id, name, color, position } => {
            conn.execute(
                "INSERT OR IGNORE INTO tabs (tab_id, name, color, position) VALUES (?1, ?2, ?3, ?4)",
                params![tab_id.to_string(), name, color, *position as i64],
            )
            .ok();
        }
        OpPayload::RenameTab { tab_id, name } => {
            conn.execute(
                "UPDATE tabs SET name = ?1 WHERE tab_id = ?2",
                params![name, tab_id.to_string()],
            )
            .ok();
        }
        OpPayload::DeleteTab { tab_id } => {
            // Delete all items in this tab
            conn.execute(
                "UPDATE snapshot SET is_deleted = 1 WHERE tab_id = ?1",
                params![tab_id.to_string()],
            )
            .ok();
            conn.execute(
                "DELETE FROM tabs WHERE tab_id = ?1",
                params![tab_id.to_string()],
            )
            .ok();
        }
        OpPayload::UpsertStatus { name, color } => {
            conn.execute(
                "INSERT INTO statuses (name, color) VALUES (?1, ?2)
                 ON CONFLICT(name) DO UPDATE SET color = excluded.color",
                params![name, color],
            )
            .ok();
        }
    }
}

fn update_status_recursive_in_db(
    conn: &Connection,
    item_id: &str,
    status: &str,
    updated_at: &DateTime<Utc>,
) {
    conn.execute(
        "UPDATE snapshot SET status = ?1, updated_at = ?2 WHERE item_id = ?3",
        params![status, updated_at.to_rfc3339(), item_id],
    )
    .ok();
    let children: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT item_id FROM snapshot WHERE parent_id = ?1 AND is_deleted = 0")
            .unwrap();
        stmt.query_map(params![item_id], |r| r.get(0))
            .unwrap()
            .filter_map(|r: SqlResult<String>| r.ok())
            .collect()
    };
    for child_id in children {
        update_status_recursive_in_db(conn, &child_id, status, updated_at);
    }
}

fn delete_subtree_in_db(conn: &Connection, item_id: &str) {
    conn.execute(
        "UPDATE snapshot SET is_deleted = 1 WHERE item_id = ?1",
        params![item_id],
    )
    .ok();
    let children: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT item_id FROM snapshot WHERE parent_id = ?1")
            .unwrap();
        stmt.query_map(params![item_id], |r| r.get(0))
            .unwrap()
            .filter_map(|r: SqlResult<String>| r.ok())
            .collect()
    };
    for child_id in children {
        delete_subtree_in_db(conn, &child_id);
    }
}

// ── Snapshot upsert (for saving current in-memory state) ──────────────────────

/// Persist the full in-memory tree back to the snapshot table.
/// Called when we want to ensure the DB reflects current state
/// (e.g. on graceful exit with unsaved timer changes).
pub fn save_tree(
    conn: &Connection,
    tabs: &[Tab],
    tab_roots: &HashMap<Uuid, Vec<TodoItem>>,
    active_tab_id: Uuid,
    active_roots: &[TodoItem],
    statuses: &HashMap<String, yan_shared::models::Status>,
) {
    // Clear and rebuild statuses
    conn.execute("DELETE FROM statuses", []).ok();
    for s in statuses.values() {
        conn.execute(
            "INSERT INTO statuses (name, color) VALUES (?1, ?2)",
            params![s.name, s.color],
        )
        .ok();
    }
    // Save tabs
    save_tabs(conn, tabs);
    // Clear snapshot and rewrite
    conn.execute("DELETE FROM snapshot", []).ok();
    for tab in tabs {
        let roots = if tab.id == active_tab_id {
            active_roots
        } else {
            tab_roots.get(&tab.id).map(|r| r.as_slice()).unwrap_or(&[])
        };
        save_items(conn, roots, None, 0, &tab.id.to_string());
    }
}

fn save_items(conn: &Connection, items: &[TodoItem], parent_id: Option<Uuid>, start_pos: i64, tab_id: &str) {
    for (i, item) in items.iter().enumerate() {
        let position = start_pos + i as i64;
        let timer_running_since: Option<String> = item
            .timer
            .running_since
            .map(|dt| dt.to_rfc3339());
        let tags_json = serde_json::to_string(&item.tags).unwrap_or_else(|_| "[]".to_string());
        conn.execute(
            "INSERT OR REPLACE INTO snapshot
             (item_id, parent_id, position, title, description, status, tags, tab_id,
              accumulated_secs, timer_running_since, created_at, updated_at, is_deleted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0)",
            params![
                item.id.to_string(),
                parent_id.map(|u| u.to_string()),
                position,
                item.title,
                item.description,
                item.status,
                tags_json,
                tab_id,
                item.timer.accumulated_secs,
                timer_running_since,
                item.created_at.to_rfc3339(),
                item.updated_at.to_rfc3339(),
            ],
        )
        .ok();
        save_items(conn, &item.children, Some(item.id), 0, tab_id);
    }
}

// ── Legacy TOML migration ─────────────────────────────────────────────────────

/// If a legacy TOML file exists, import it into the SQLite snapshot and return true.
fn try_migrate_from_toml(conn: &Connection) -> bool {
    let toml_path = legacy_toml_path();
    if !toml_path.exists() {
        return false;
    }
    let contents = match std::fs::read_to_string(&toml_path) {
        Ok(c) => c,
        Err(_) => return false,
    };

    #[derive(serde::Deserialize)]
    struct LegacyFile {
        #[serde(default)]
        statuses: Vec<Status>,
        #[serde(default)]
        todos: Vec<TodoItem>,
    }

    let file: LegacyFile = match toml::from_str(&contents) {
        Ok(f) => f,
        Err(_) => return false,
    };

    // Seed statuses
    let statuses_to_use = if file.statuses.is_empty() {
        Status::defaults()
    } else {
        file.statuses
    };
    for s in &statuses_to_use {
        conn.execute(
            "INSERT OR IGNORE INTO statuses (name, color) VALUES (?1, ?2)",
            params![s.name, s.color],
        )
        .ok();
    }

    // Import todos
    save_items(conn, &file.todos, None, 0, &yan_shared::models::DEFAULT_TAB_ID.to_string());

    // Rename the old file so migration doesn't run again
    let done_path = toml_path.with_extension("toml.migrated");
    std::fs::rename(&toml_path, &done_path).ok();

    true
}
