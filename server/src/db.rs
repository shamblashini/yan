use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use yan_shared::models::{Status, TimerState, TodoItem};
use yan_shared::ops::{Operation, OpPayload};

// ── Migrations ────────────────────────────────────────────────────────────────

pub async fn run_migrations(pool: &PgPool) {
    let stmts = [
        "CREATE TABLE IF NOT EXISTS operations (
            op_id        UUID PRIMARY KEY,
            device_id    UUID NOT NULL,
            client_seq   BIGINT NOT NULL,
            happened_at  TIMESTAMPTZ NOT NULL,
            received_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            payload      JSONB NOT NULL,
            UNIQUE (device_id, client_seq)
        )",
        "CREATE INDEX IF NOT EXISTS ops_received_at_idx ON operations (received_at)",
        "CREATE INDEX IF NOT EXISTS ops_device_seq_idx  ON operations (device_id, client_seq)",
        "CREATE TABLE IF NOT EXISTS snapshot (
            item_id              UUID PRIMARY KEY,
            parent_id            UUID REFERENCES snapshot(item_id) ON DELETE CASCADE,
            position             INTEGER NOT NULL DEFAULT 0,
            title                TEXT NOT NULL DEFAULT '',
            description          TEXT,
            status               TEXT NOT NULL DEFAULT 'Todo',
            accumulated_secs     BIGINT NOT NULL DEFAULT 0,
            timer_running_since  TIMESTAMPTZ,
            created_at           TIMESTAMPTZ NOT NULL,
            updated_at           TIMESTAMPTZ NOT NULL,
            is_deleted           BOOLEAN NOT NULL DEFAULT FALSE
        )",
        "CREATE TABLE IF NOT EXISTS statuses (
            name       TEXT PRIMARY KEY,
            color      TEXT NOT NULL,
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )",
    ];

    for stmt in &stmts {
        sqlx::query(stmt)
            .execute(pool)
            .await
            .expect("Migration failed");
    }

    // Seed default statuses if table is empty
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM statuses")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    if count == 0 {
        for s in Status::defaults() {
            sqlx::query("INSERT INTO statuses (name, color) VALUES ($1, $2) ON CONFLICT DO NOTHING")
                .bind(&s.name)
                .bind(&s.color)
                .execute(pool)
                .await
                .ok();
        }
    }
}

// ── Store operations ──────────────────────────────────────────────────────────

/// Persist a batch of incoming ops. Returns the highest client_seq stored for the sender's device.
pub async fn store_ops(pool: &PgPool, ops: &[Operation]) -> u64 {
    let mut max_seq: u64 = 0;
    for op in ops {
        let payload = serde_json::to_value(&op.payload).unwrap_or_default();
        sqlx::query(
            "INSERT INTO operations (op_id, device_id, client_seq, happened_at, payload)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (op_id) DO NOTHING",
        )
        .bind(op.op_id)
        .bind(op.device_id)
        .bind(op.client_seq as i64)
        .bind(op.happened_at)
        .bind(payload)
        .execute(pool)
        .await
        .ok();

        // Update materialized snapshot
        apply_op_to_snapshot(pool, op).await;

        if op.client_seq > max_seq {
            max_seq = op.client_seq;
        }
    }
    max_seq
}

/// Fetch all ops received after the given cursor (by received_at), up to a limit.
pub async fn ops_since(pool: &PgPool, cursor: Option<DateTime<Utc>>) -> (Vec<Operation>, DateTime<Utc>) {
    let since = cursor.unwrap_or_else(|| DateTime::<Utc>::UNIX_EPOCH);

    let rows = sqlx::query_as::<_, OpRow>(
        "SELECT op_id, device_id, client_seq, happened_at, payload
         FROM operations
         WHERE received_at > $1
         ORDER BY received_at, op_id
         LIMIT 1000",
    )
    .bind(since)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let new_cursor = rows
        .iter()
        .map(|r| r.happened_at)
        .max()
        .unwrap_or_else(Utc::now);

    let ops: Vec<Operation> = rows
        .into_iter()
        .filter_map(|r| {
            let payload: OpPayload = serde_json::from_value(r.payload).ok()?;
            Some(Operation {
                op_id: r.op_id,
                device_id: r.device_id,
                client_seq: r.client_seq as u64,
                happened_at: r.happened_at,
                payload,
            })
        })
        .collect();

    (ops, new_cursor)
}

#[derive(sqlx::FromRow)]
struct OpRow {
    op_id: Uuid,
    device_id: Uuid,
    client_seq: i64,
    happened_at: DateTime<Utc>,
    payload: serde_json::Value,
}

// ── Snapshot materialization ──────────────────────────────────────────────────

pub async fn apply_op_to_snapshot(pool: &PgPool, op: &Operation) {
    match &op.payload {
        OpPayload::CreateItem { item_id, parent_id, position, title, status } => {
            sqlx::query(
                "INSERT INTO snapshot (item_id, parent_id, position, title, status, created_at, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
                 ON CONFLICT (item_id) DO NOTHING",
            )
            .bind(item_id)
            .bind(parent_id)
            .bind(*position as i32)
            .bind(title)
            .bind(status)
            .bind(op.happened_at)
            .bind(op.happened_at)
            .execute(pool)
            .await
            .ok();
        }
        OpPayload::UpdateTitle { item_id, title } => {
            sqlx::query(
                "UPDATE snapshot SET title = $1, updated_at = $2 WHERE item_id = $3 AND NOT is_deleted",
            )
            .bind(title)
            .bind(op.happened_at)
            .bind(item_id)
            .execute(pool)
            .await
            .ok();
        }
        OpPayload::UpdateDescription { item_id, description } => {
            sqlx::query(
                "UPDATE snapshot SET description = $1, updated_at = $2 WHERE item_id = $3 AND NOT is_deleted",
            )
            .bind(description)
            .bind(op.happened_at)
            .bind(item_id)
            .execute(pool)
            .await
            .ok();
        }
        OpPayload::UpdateStatus { item_id, status, recursive } => {
            if *recursive {
                update_status_recursive(pool, item_id, status, op.happened_at).await;
            } else {
                sqlx::query(
                    "UPDATE snapshot SET status = $1, updated_at = $2 WHERE item_id = $3 AND NOT is_deleted",
                )
                .bind(status)
                .bind(op.happened_at)
                .bind(item_id)
                .execute(pool)
                .await
                .ok();
            }
        }
        OpPayload::DeleteItem { item_id } => {
            // Mark deleted; ON DELETE CASCADE will propagate if we do a hard delete later.
            // For now, soft-delete the subtree.
            sqlx::query(
                "WITH RECURSIVE subtree AS (
                     SELECT item_id FROM snapshot WHERE item_id = $1
                     UNION ALL
                     SELECT s.item_id FROM snapshot s
                     JOIN subtree t ON s.parent_id = t.item_id
                 )
                 UPDATE snapshot SET is_deleted = TRUE
                 WHERE item_id IN (SELECT item_id FROM subtree)",
            )
            .bind(item_id)
            .execute(pool)
            .await
            .ok();
        }
        OpPayload::MoveItem { item_id, new_parent_id, new_position } => {
            sqlx::query(
                "UPDATE snapshot SET parent_id = $1, position = $2, updated_at = $3 WHERE item_id = $4",
            )
            .bind(new_parent_id)
            .bind(*new_position as i32)
            .bind(op.happened_at)
            .bind(item_id)
            .execute(pool)
            .await
            .ok();
        }
        OpPayload::TimerStart { item_id, started_at } => {
            sqlx::query(
                "UPDATE snapshot SET timer_running_since = $1 WHERE item_id = $2 AND timer_running_since IS NULL AND NOT is_deleted",
            )
            .bind(started_at)
            .bind(item_id)
            .execute(pool)
            .await
            .ok();
        }
        OpPayload::TimerStop { item_id, stopped_at: _, session_secs } => {
            sqlx::query(
                "UPDATE snapshot SET accumulated_secs = accumulated_secs + $1, timer_running_since = NULL WHERE item_id = $2 AND NOT is_deleted",
            )
            .bind(session_secs)
            .bind(item_id)
            .execute(pool)
            .await
            .ok();
        }
        OpPayload::UpsertStatus { name, color } => {
            sqlx::query(
                "INSERT INTO statuses (name, color) VALUES ($1, $2)
                 ON CONFLICT (name) DO UPDATE SET color = EXCLUDED.color, updated_at = NOW()",
            )
            .bind(name)
            .bind(color)
            .execute(pool)
            .await
            .ok();
        }
    }
}

async fn update_status_recursive(pool: &PgPool, root_id: &Uuid, status: &str, updated_at: DateTime<Utc>) {
    sqlx::query(
        "WITH RECURSIVE subtree AS (
             SELECT item_id FROM snapshot WHERE item_id = $1
             UNION ALL
             SELECT s.item_id FROM snapshot s
             JOIN subtree t ON s.parent_id = t.item_id
         )
         UPDATE snapshot SET status = $2, updated_at = $3
         WHERE item_id IN (SELECT item_id FROM subtree) AND NOT is_deleted",
    )
    .bind(root_id)
    .bind(status)
    .bind(updated_at)
    .execute(pool)
    .await
    .ok();
}

// ── Snapshot read (for GET /snapshot) ─────────────────────────────────────────

pub async fn get_snapshot(pool: &PgPool) -> (Vec<TodoItem>, Vec<Status>) {
    let statuses: Vec<Status> = sqlx::query_as::<_, (String, String)>(
        "SELECT name, color FROM statuses ORDER BY name",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(name, color)| Status { name, color })
    .collect();

    #[derive(sqlx::FromRow)]
    struct SnapshotRow {
        item_id: Uuid,
        parent_id: Option<Uuid>,
        position: i32,
        title: String,
        description: Option<String>,
        status: String,
        accumulated_secs: i64,
        timer_running_since: Option<DateTime<Utc>>,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    }

    let rows = sqlx::query_as::<_, SnapshotRow>(
        "SELECT item_id, parent_id, position, title, description, status,
                accumulated_secs, timer_running_since, created_at, updated_at
         FROM snapshot
         WHERE NOT is_deleted
         ORDER BY position",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    // Build tree
    let items: Vec<_> = rows
        .iter()
        .map(|r| (
            r.item_id,
            r.parent_id,
            r.position,
            TodoItem {
                id: r.item_id,
                title: r.title.clone(),
                description: r.description.clone(),
                status: r.status.clone(),
                children: Vec::new(),
                timer: TimerState {
                    accumulated_secs: r.accumulated_secs,
                    running_since: r.timer_running_since,
                },
                created_at: r.created_at,
                updated_at: r.updated_at,
            },
        ))
        .collect();

    let roots = build_tree_from_rows(items, None);
    (roots, statuses)
}

fn build_tree_from_rows(
    rows: Vec<(Uuid, Option<Uuid>, i32, TodoItem)>,
    parent_id: Option<Uuid>,
) -> Vec<TodoItem> {
    let mut children: Vec<_> = rows
        .iter()
        .filter(|(_, pid, _, _)| *pid == parent_id)
        .collect();
    children.sort_by_key(|(_, _, pos, _)| *pos);

    children
        .into_iter()
        .map(|(id, _, _, item)| {
            let mut item = item.clone();
            item.children = build_tree_from_rows(rows.clone(), Some(*id));
            item
        })
        .collect()
}
