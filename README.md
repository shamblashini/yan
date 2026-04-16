# yan ~~lungmen~~

## Why?
Because, of course you need ANOTHER todo TUI with lazy[insertutilname] syle and vim controlls. to be honest I do not care if people use it or not, I needed my infinite nesting and I WILL GET IT, I like my tasks properly nested, also was kind of needing the sync.
*I will now proceed to force everyone to used it via advanced comoputer hypnosis*

## preview
<img width="513" height="1030" alt="image" src="https://github.com/user-attachments/assets/c703e7f7-3fd5-4352-9083-461f5dcfdc81" />
<img width="438" height="642" alt="image" src="https://github.com/user-attachments/assets/5ad1f2e4-ccb1-40ac-9136-2c0c1993957c" />

## Quickstart
the TUI works without a server (in loacl only mode, no sync), to use, please clone the repo, build and install the /tui
```
git clone https://github.com/shamblashini/yan.git
cd yan/tui
cargo install --path .
```
> make sure your shell resolves rust install dir

if you want to install the sync server, you can use the docker image
```
docker pull ghcr.io/shamblashini/yan/server:latest
```

## plans

- [X] tabs (spaces of tasks)
- [X] tags & views (ability to tag tasks and view tasks with specific tags)
- [ ] notes (add note tasks without a status but with the ability to wrap text)
- [ ] clear/hide compleated tasks (maybe with animation)

---

below is a more detailed description of the archetecture and requirements, written by AI, enter at your own risk

## Table of Contents

- [Features](#features)
- [Architecture](#architecture)
- [Requirements](#requirements)
- [Quick Start — Local Only](#quick-start--local-only)
- [Quick Start — With Sync](#quick-start--with-sync)
- [TUI Reference](#tui-reference)
- [Server Reference](#server-reference)
- [Configuration](#configuration)
- [Data Storage](#data-storage)
- [How Sync Works](#how-sync-works)
- [Conflict Resolution](#conflict-resolution)
- [Building from Source](#building-from-source)
- [Project Structure](#project-structure)

---

## Features

- **Hierarchical tasks** — nest todos to any depth; collapse and expand branches
- **Tabs** — separate todo lists switchable as tabs; each tab has its own tree of tasks
- **Tags** — coloured keyword labels on tasks, rendered as `[tag]` badges; deterministic per-tag colours
- **Views** — filter tasks by tag across all tabs; interactable virtual views shown alongside tabs
- **Custom statuses** — ship with Todo / In Progress / Done / Blocked / Cancelled; add your own with any colour
- **Time tracking** — per-task timer with aggregate time rolled up through parent tasks
- **Detail panel** — sidebar or bottom strip showing status, timestamps, children progress, tags, description
- **Search** — live filter across all task titles and tag names with next/previous match navigation
- **Offline-first sync** — changes apply instantly to local storage; background sync to server with no perceptible delay
- **Live sync** — connected devices see each other's changes within ~1 second over WebSocket
- **Conflict resolution** — divergent offline edits are merged automatically when devices reconnect
- **Single-user** — one server instance per person; no accounts, no multi-tenancy

---

## Architecture

```
yan/
├── shared/      shared Rust types (models, operations, sync protocol)
├── tui/         terminal UI — the client you run day-to-day
└── server/      Axum HTTP server — optional, enables sync across devices
```

The three components communicate through a well-defined sync protocol:

```
┌──────────────┐    POST /api/sync    ┌─────────────────┐
│   TUI        │ ──────────────────►  │                 │
│  (device A)  │ ◄──────────────────  │  yan-server     │
└──────────────┘    new remote ops    │  (PostgreSQL)   │
                                      │                 │
┌──────────────┐    WS  /api/ws       │                 │
│   TUI        │ ◄──────────────────  │                 │
│  (device B)  │   live push          └─────────────────┘
└──────────────┘
```

Every mutation produces an **Operation** (an append-only event). Operations are:
- written to local SQLite immediately (no delay to the user)
- sent to the server in the background over HTTP
- broadcast to other connected clients over WebSocket

---

## Requirements

**To run the TUI (local only):**
- Rust toolchain — install from [rustup.rs](https://rustup.rs)

**To run the server (for sync):**
- Rust toolchain
- PostgreSQL 14 or later

---

## Quick Start — Local Only

Clone the repository, build, and run:

```bash
git clone <repo-url>
cd yan
cargo run -p todo
```

Your data is stored in `~/.local/share/todo/todo.db` (Linux/macOS) or
`%APPDATA%\todo\todo.db` (Windows).

**Migrating from a previous version:** if you have a `todos.toml` file from the original
app it will be imported automatically on first run and renamed to `todos.toml.migrated`.

---

## Quick Start — With Sync

### 1. Set up PostgreSQL

Create a database for yan:

```bash
createdb yan
```

### 2. Start the server

```bash
cd yan

export DATABASE_URL="postgres://localhost/yan"
export AUTH_TOKEN="choose-a-strong-secret-here"
# optional — defaults to 0.0.0.0:3000
export LISTEN_ADDR="0.0.0.0:3000"

cargo run -p yan-server
```

The server runs migrations automatically on startup; no separate migration step is needed.

### 3. Configure the TUI on each device

Edit `~/.config/yan/config.toml` (created automatically on first run):

```toml
device_id    = "..."              # auto-generated — do not change this
server_url   = "http://your-server:3000"
auth_token   = "choose-a-strong-secret-here"
sync_enabled = true
```

The file is created the first time you run the TUI. Open it, fill in `server_url` and
`auth_token`, and set `sync_enabled = true`.

### 4. Run the TUI

```bash
cargo run -p todo
```

The sync status is shown in the bottom-right corner of the screen.

---

## TUI Reference

### Modes

The TUI has three modes shown in the bottom-left badge:

| Badge | Mode | Description |
|---|---|---|
| `NORMAL` | Normal | Navigation and commands |
| `INSERT` | Insert | Typing text in a popup |
| `SEARCH` | Search | Live title search |

### Normal Mode — Navigation

| Key | Action |
|---|---|
| `j` / `↓` | Move cursor down |
| `k` / `↑` | Move cursor up |
| `h` / `←` | Collapse current item |
| `l` / `→` | Expand current item |
| `Enter` | Toggle collapse/expand |
| `gg` | Jump to first item |
| `G` | Jump to last item |

### Normal Mode — Editing

| Key | Action |
|---|---|
| `a` | Add a sibling task below the current one |
| `A` | Add a child task under the current one |
| `i` / `e` | Edit the current task's title |
| `E` | Edit the current task's description |
| `dd` | Delete current task and all its children (with confirmation) |
| `L` / `>` | Indent — make current item a child of the item above |
| `H` / `<` | Dedent — promote current item one level up |
| `J` / `K` | Move task down/up among siblings |

When you add or edit a title, a popup appears:
- `Enter` — confirm and save
- `Esc` — cancel (new items are discarded; existing items keep their old title)

When you edit a description, a multi-line popup appears:
- `Esc` — confirm and save
- `Ctrl-C` — cancel

### Normal Mode — Tags

| Key | Action |
|---|---|
| `#` | Open tag editor on the current task |

Inside the tag editor popup:
- Type a tag name and press `Enter` to add it
- `↑` / `↓` — navigate existing tags
- `Ctrl+d` — remove the selected tag
- `Esc` — confirm and close

Tags appear as coloured `[tag]` badges after the task title. Each tag gets a
deterministic colour from a palette so the same tag always looks the same. Tags
are also matched by the `/` search.

### Normal Mode — Tabs

| Key | Action |
|---|---|
| `Tab` | Switch to the next tab |
| `Shift+Tab` | Switch to the previous tab |
| `c` | Create a new tab |
| `r` | Rename the current tab |
| `m` | Move the current task to another tab (picker) |
| `X` | Delete the current tab and all its items (with confirmation) |

The tab bar appears at the top of the screen when you have more than one tab (or
any views). The active tab is highlighted. Every new installation starts with a
single "Default" tab; existing data is migrated into it automatically.

### Normal Mode — Views

| Key | Action |
|---|---|
| `v` | Open the view picker |
| `Esc` | Exit the active view and return to normal tab mode |

Views let you see all tasks with a given tag across every tab. Inside the view
picker:
- `j` / `k` — navigate
- `Enter` — activate the selected view, or create a new one
- `d` — delete the selected view
- `Esc` — cancel

Views are shown in the tab bar after the real tabs, styled in cyan. While a view
is active the tree title shows `View: <tag>` and only matching items are listed.
Views are stored locally and are not synced between devices.

### Normal Mode — Status and Timers

| Key | Action |
|---|---|
| `Space` | Toggle between Done and Todo |
| `s` | Open status picker (all statuses) |
| `t` | Start/stop the timer on the current task |
| `T` | Stop all running timers |

Changing a parent's status applies recursively to all children.
When all children of a task are Done, the parent auto-completes.
When any child becomes not Done, the parent reverts to In Progress.

### Normal Mode — Search and UI

| Key | Action |
|---|---|
| `/` | Start search — type to filter titles and tags live |
| `n` | Next search match |
| `N` | Previous search match |
| `Esc` | Clear search (or exit view if one is active) |
| `p` | Toggle detail panel (sidebar or bottom strip) |
| `?` | Show help popup |
| `q` | Save and quit |
| `Ctrl-C` | Quit immediately |

### Status Picker

When the status picker is open (`s`):

| Key | Action |
|---|---|
| `j` / `↓` | Move down |
| `k` / `↑` | Move up |
| `Enter` | Apply selected status |
| `a` | Add a new custom status |
| `Esc` | Cancel |

Custom statuses can use any named colour (`red`, `green`, `yellow`, `blue`, `magenta`,
`cyan`, `white`, `dark_gray`, `light_red`, `light_green`, `light_yellow`, `light_blue`,
`light_magenta`, `light_cyan`) or a hex colour (`#ff8800`).

### Detail Panel

Toggle with `p`. Shows:
- Current status with colour
- Created and updated timestamps
- Children progress (`n/m done`)
- Timer — own time, total time (including children), running indicator
- Tags (coloured badges)
- Full description

In sidebar mode (wide terminals): tree on the left, detail on the right.
In strip mode (narrow terminals): compact strip below the tree.

### Sync Status Indicator

Bottom-right corner shows the current sync state:

| Indicator | Meaning |
|---|---|
| *(nothing)* | Sync is disabled (no server configured) |
| `[Synced]` green | Connected and fully up to date |
| `[Syncing…]` yellow | Actively pushing or pulling |
| `[Offline]` red | Cannot reach server; all changes are saved locally |
| `[Offline · N pending]` red | Offline with N unsynced operations queued |

Offline changes are never lost. They are stored in the local database and synced
automatically when the connection is restored.

---

## Server Reference

### Environment Variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `DATABASE_URL` | Yes | — | PostgreSQL connection string |
| `AUTH_TOKEN` | Yes | — | Bearer token that clients must send |
| `LISTEN_ADDR` | No | `0.0.0.0:3000` | Address and port to listen on |
| `RUST_LOG` | No | `yan_server=debug` | Log level filter |

### API Endpoints

#### `GET /health`

Returns `200 ok`. Use for load balancer or uptime checks. No authentication required.

---

#### `POST /api/sync`

Push local operations to the server and pull remote operations since your last sync.
All endpoints except `/health` require `Authorization: Bearer <token>`.

**Request:**
```json
{
  "device_id": "550e8400-e29b-41d4-a716-446655440000",
  "cursor": "2026-04-07T10:00:00Z",
  "ops": [
    {
      "op_id": "...",
      "device_id": "...",
      "client_seq": 42,
      "happened_at": "2026-04-07T10:01:00Z",
      "payload": {
        "type": "update_title",
        "item_id": "...",
        "title": "Buy oat milk"
      }
    }
  ]
}
```

- `cursor` — the `new_cursor` value from the previous response, or omit on first sync
- `ops` — local operations not yet confirmed by the server (may be empty for pull-only)

**Response:**
```json
{
  "accepted_through_seq": 42,
  "new_ops": [ ... ],
  "new_cursor": "2026-04-07T10:01:05Z"
}
```

- `accepted_through_seq` — highest `client_seq` stored for this device
- `new_ops` — operations from other devices since your cursor
- `new_cursor` — store this and send it in the next request

---

#### `GET /api/snapshot`

Retrieve the full current state. Use this to bootstrap a fresh device instead of
replaying the full operation history.

**Response:**
```json
{
  "items": [ ... ],
  "statuses": [
    { "name": "Todo",        "color": "white"    },
    { "name": "In Progress", "color": "yellow"   },
    { "name": "Done",        "color": "green"    },
    { "name": "Blocked",     "color": "red"      },
    { "name": "Cancelled",   "color": "dark_gray" }
  ],
  "cursor": "2026-04-07T10:01:05Z"
}
```

After bootstrapping, start syncing from the returned `cursor`.

---

#### `WS /api/ws`

WebSocket endpoint for receiving live operation pushes from other devices.

**Query parameters:**
- `token` — auth token (since WebSocket clients cannot send custom headers)
- `cursor` — (optional) `received_at` of the last known operation; catch-up ops are sent immediately on connect

**Server-to-client messages:**
```json
{ "type": "ops", "ops": [ { ... } ] }
```

The client never sends operations over WebSocket; use `POST /api/sync` for that.
The server sends keepalive pings every 30 seconds.

### Operation Payload Types

All operations follow the same envelope:

```json
{
  "op_id":       "uuid",
  "device_id":   "uuid",
  "client_seq":  42,
  "happened_at": "2026-04-07T10:00:00Z",
  "payload": { "type": "...", ... }
}
```

| `type` | Fields | Description |
|---|---|---|
| `create_item` | `item_id`, `parent_id` (nullable), `position`, `title`, `status`, `tags`, `tab_id` | Create a new task |
| `update_title` | `item_id`, `title` | Rename a task |
| `update_description` | `item_id`, `description` (nullable) | Set or clear description |
| `update_status` | `item_id`, `status`, `recursive` (bool) | Change status |
| `update_tags` | `item_id`, `tags` (array) | Replace all tags on a task |
| `delete_item` | `item_id` | Delete task and all children |
| `move_item` | `item_id`, `new_parent_id` (nullable), `new_position` | Re-parent or re-order |
| `timer_start` | `item_id`, `started_at` | Start timer |
| `timer_stop` | `item_id`, `stopped_at`, `session_secs` | Stop timer; records session duration |
| `create_tab` | `tab_id`, `name`, `color`, `position` | Create a new tab |
| `rename_tab` | `tab_id`, `name` | Rename a tab |
| `delete_tab` | `tab_id` | Delete a tab and all its items |
| `upsert_status` | `name`, `color` | Create or update a custom status |

---

## Configuration

The TUI reads `~/.config/yan/config.toml` (Linux/macOS) or
`%APPDATA%\yan\config.toml` (Windows). The file is created automatically on first run.

```toml
# Unique identifier for this device.
# Generated once on first run. Do not change this — it is used to
# de-duplicate your operations on the server.
device_id = "550e8400-e29b-41d4-a716-446655440000"

# URL of your yan server. Leave empty to run in local-only mode.
server_url = "https://yan.example.com"

# Bearer token matching AUTH_TOKEN on the server.
auth_token = "your-secret-token"

# Set to true to enable background sync.
sync_enabled = true
```

If `sync_enabled` is `false`, or `server_url` / `auth_token` are empty, the TUI runs
in fully local mode. All data is still saved to SQLite; you can enable sync later
without losing anything.

---

## Data Storage

### Client — SQLite

`~/.local/share/todo/todo.db` (Linux) · `~/Library/Application Support/todo/todo.db` (macOS) · `%APPDATA%\todo\todo.db` (Windows)

The database has these tables:

| Table | Contents |
|---|---|
| `snapshot` | Current state of all tasks with `tab_id` and `tags` columns |
| `tabs` | Tab definitions (id, name, colour, position) |
| `statuses` | Custom and default status definitions |
| `local_ops` | Append-only log of every local mutation; `synced=0` means pending |
| `sync_state` | Key/value pairs: `device_id`, `server_cursor`, etc. |
| `collapse_state` | Set of collapsed item UUIDs |
| `tag_views` | Locally-stored tag view definitions (not synced) |

The `snapshot` table is a cache — it can be fully rebuilt by replaying `local_ops`.
You can back up the whole database with a simple file copy while the TUI is not running.

### Server — PostgreSQL

| Table | Contents |
|---|---|
| `operations` | All operations from all devices, append-only |
| `snapshot` | Materialised current state (rebuilt from operations) |
| `statuses` | Current status definitions |

The `operations` table is the canonical source of truth. The `snapshot` table is a
performance cache and can be rebuilt from `operations` at any time.

---

## How Sync Works

### Write path (local mutation)

1. User presses a key → `AppState` applies the change to the in-memory tree immediately
2. An `Operation` is written to `local_ops` in SQLite with `synced = 0`
3. The operation is sent to the background sync task via a channel
4. The TUI renders the updated state — no waiting for the network

### Sync task (background)

1. Collects local operations (debounced to 500ms to batch rapid edits)
2. Sends them to `POST /api/sync` with the last known server cursor
3. Server stores the operations and returns any new operations from other devices
4. Sync task marks local operations as `synced = 1` and updates the cursor
5. Remote operations are forwarded to the TUI via a channel

### Receive path (remote operations)

1. TUI receives remote operations from the sync task channel each tick (~200ms)
2. Operations are applied to the in-memory tree
3. Operations are also written to the local SQLite snapshot
4. Visible tree is rebuilt

### Live push (WebSocket)

In addition to polling via `POST /api/sync`, the sync task opens a WebSocket connection
to `WS /api/ws`. When another device syncs, the server immediately broadcasts its
operations to all connected clients. This reduces the typical latency from ~200ms (one
poll interval) to ~50ms (network round-trip only).

### Fresh device bootstrap

When a device syncs for the first time and the local database is empty, it calls
`GET /api/snapshot` to download the full current state rather than replaying the entire
operation history. After bootstrapping, it switches to the normal sync loop.

---

## Conflict Resolution

yan is a single-user application — one server, one person's data. Conflicts arise when
you edit on an offline device (e.g. a laptop without internet) and separately make changes
through another device, then reconnect.

### Rules

| Scenario | Resolution |
|---|---|
| Same field edited on two devices | The edit with the later `happened_at` timestamp wins |
| Item deleted on one device, modified on another | **Deletion wins** — the item is gone |
| Two items inserted at the same position | Both are kept; `op_id` (UUID) is used as a stable tiebreaker for ordering |
| Item moved on two devices | The later `MoveItem` operation by `happened_at` wins |
| Timer running on two devices for the same task | Both sessions accumulate — `TimerStop` records the session's elapsed seconds, so the final total is the correct sum of all sessions |

### Why last-write-wins is sufficient

Multi-user applications need complex conflict resolution (CRDTs, operational transforms)
because two users might have genuinely conflicting intentions. With a single user,
last-write-wins is always semantically correct — you know which edit you made most recently
and that is the one you want.

The only exception is deletions, which win regardless of timestamp. A deleted task is
gone definitively; it would be confusing if a remote edit resurrected it.

---

## Building from Source

### Debug build

```bash
cargo build
```

### Release build (smaller binary, optimised)

```bash
cargo build --release
```

Binaries are at `target/release/todo` (TUI) and `target/release/yan-server` (server).

### Run tests

```bash
cargo test                    # all workspace tests
cargo test -p yan-shared      # shared crate only
```

### Format and lint

```bash
cargo fmt       # auto-format all code
cargo clippy    # lint — treat warnings as advice
```

### Environment for development

For local development you can run the server against a local Postgres instance and point
a TUI at it:

```bash
# Terminal 1 — server
export DATABASE_URL="postgres://localhost/yan_dev"
export AUTH_TOKEN="dev-token"
cargo run -p yan-server

# Terminal 2 — TUI (after setting config.toml)
cargo run -p todo
```

To test offline sync, start two TUI instances with the same `config.toml` pointed at the
server, make changes in one, stop the server, make changes in the other, restart the server,
and watch them merge.

---

## Project Structure

```
yan/
├── Cargo.toml                  workspace root
├── Cargo.lock
├── README.md                   this file
├── RUST_LEARNING.md            Rust concept guide and exercises
│
├── shared/                     yan-shared library crate
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── models.rs           TodoItem, Status, TimerState, tree utilities
│       ├── ops.rs              Operation and OpPayload — the sync event log
│       └── sync.rs             SyncRequest, SyncResponse, SnapshotResponse, WsServerMessage
│
├── tui/                        todo binary crate — the terminal app
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs             entry point, tokio runtime, channel wiring
│       ├── app.rs              AppState — all runtime state and mutation methods
│       ├── input.rs            keyboard event handling
│       ├── ui.rs               Ratatui rendering
│       ├── storage.rs          SQLite persistence and operation log
│       ├── sync_client.rs      background sync task (HTTP + WebSocket)
│       ├── config.rs           ~/.config/yan/config.toml
│       ├── todo.rs             re-exports from yan-shared
│       └── time_tracker.rs     re-exports from yan-shared
│
└── server/                     yan-server binary crate — the sync backend
    ├── Cargo.toml
    └── src/
        ├── main.rs             Axum setup, state construction
        ├── db.rs               PostgreSQL queries and snapshot materialisation
        ├── broadcast.rs        in-memory broadcast channel for WebSocket push
        └── routes/
            ├── mod.rs          router assembly
            ├── sync.rs         POST /api/sync, GET /api/snapshot, auth middleware
            └── ws.rs           WS /api/ws — live push handler
```

### Key dependency choices

| Dependency | Used in | Purpose |
|---|---|---|
| `ratatui` | tui | Terminal UI rendering |
| `crossterm` | tui | Terminal backend (keyboard, cursor) |
| `ratatui-textarea` | tui | Multi-line text input widget |
| `rusqlite` (bundled) | tui | Local SQLite database |
| `tokio` | tui, server | Async runtime |
| `reqwest` | tui | HTTP client for sync |
| `tokio-tungstenite` | tui, server | WebSocket client/server |
| `axum` | server | HTTP and WebSocket server framework |
| `sqlx` | server | Async PostgreSQL driver |
| `tower-http` | server | Middleware (CORS, tracing) |
| `serde` + `serde_json` | all | Serialization |
| `uuid` | all | Unique IDs for items and devices |
| `chrono` | all | Timestamps and durations |
| `dirs` | tui | Platform-appropriate config/data paths |

---

## Deploying the Server

The server is a single statically-linked binary (after a release build). A minimal
production setup:

**systemd service** (`/etc/systemd/system/yan.service`):

```ini
[Unit]
Description=yan sync server
After=network.target postgresql.service

[Service]
Type=simple
User=yan
WorkingDirectory=/opt/yan
ExecStart=/opt/yan/yan-server
Restart=on-failure
RestartSec=5

Environment=DATABASE_URL=postgres://yan:password@localhost/yan
Environment=AUTH_TOKEN=your-strong-secret-here
Environment=LISTEN_ADDR=127.0.0.1:3000
Environment=RUST_LOG=yan_server=info

[Install]
WantedBy=multi-user.target
```

**Nginx reverse proxy** (for HTTPS — strongly recommended when not on a local network):

```nginx
server {
    listen 443 ssl;
    server_name yan.example.com;

    ssl_certificate     /etc/letsencrypt/live/yan.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/yan.example.com/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_http_version 1.1;

        # Required for WebSocket upgrade
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_read_timeout 86400;   # keep WebSocket connections alive
    }
}
```

With HTTPS, update the TUI config to use `wss://` automatically (the sync client
converts `https://` to `wss://` for the WebSocket URL):

```toml
server_url = "https://yan.example.com"
```
