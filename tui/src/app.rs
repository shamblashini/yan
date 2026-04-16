use std::collections::{HashMap, HashSet};
use std::time::Instant;

use chrono::Utc;
use ratatui_textarea::TextArea;
use rusqlite::Connection;
use uuid::Uuid;

use crate::input::{new_textarea, Mode};
use crate::storage;
use crate::sync_client::SyncStatus;
use crate::todo::{
    check_parent_completion, flatten_node, item_at, item_at_mut, parent_vec_mut,
    set_status_recursive, CursorPath, Status, TodoItem,
};
use tokio::sync::{mpsc, watch};
use yan_shared::ops::{Operation, OpPayload};

pub struct TagView {
    pub name: String,
    pub tag_filter: String,
}

pub enum PopupKind {
    EditTitle { textarea: TextArea<'static> },
    EditDescription { textarea: TextArea<'static> },
    SetStatus { options: Vec<String>, selected: usize },
    AddStatus { textarea: TextArea<'static>, color_buf: String },
    ConfirmDelete,
    EditTags { textarea: TextArea<'static>, existing: Vec<String>, selected: usize },
    CreateTabName { textarea: TextArea<'static> },
    RenameTab { textarea: TextArea<'static> },
    TabPicker { options: Vec<(String, usize)>, selected: usize },
    ConfirmDeleteTab,
    ViewPicker { options: Vec<String>, selected: usize },
    CreateView { textarea: TextArea<'static> },
    Help,
}

pub struct AppState {
    // ── Tabs ────────────────────────────────────────────────────────────────
    pub tabs: Vec<crate::todo::Tab>,
    pub tab_roots: HashMap<Uuid, Vec<TodoItem>>,
    pub active_tab_idx: usize,
    // ── Views ───────────────────────────────────────────────────────────────
    pub views: Vec<TagView>,
    pub active_view: Option<usize>,
    /// When in view mode: maps item_id to the tab_id that owns it.
    view_item_tab: HashMap<Uuid, Uuid>,
    // ── Active tab data (swapped on tab switch) ─────────────────────────────
    pub roots: Vec<TodoItem>,
    pub status_map: HashMap<String, Status>,
    pub mode: Mode,
    /// Index into visible_flat
    pub cursor_idx: usize,
    pub visible_flat: Vec<(usize, CursorPath)>,
    pub collapsed: HashSet<Uuid>,
    pub tree_scroll: usize,
    pub search_query: Option<String>,
    pub pending_key: Option<char>,
    pub popup: Option<PopupKind>,
    pub status_message: Option<String>,
    pub should_quit: bool,
    pub show_detail_panel: bool,
    /// When false, Done/Cancelled tasks are filtered out of `visible_flat`.
    /// Toggled by `o` and persisted to `config.toml`.
    pub show_completed: bool,
    // ── Sync / persistence ──────────────────────────────────────────────────
    db: Connection,
    device_id: Uuid,
    /// Next client_seq to assign to the next emitted operation.
    next_seq: u64,
    /// When add_sibling_below / add_child creates a new item, we track its id here
    /// so apply_edit_title can emit CreateItem (not UpdateTitle) for the commit.
    pending_new_item: Option<NewItemCtx>,
    /// Channel to send local ops to the background sync task.
    local_op_tx: Option<mpsc::Sender<Operation>>,
    /// Channel to receive remote ops from the background sync task.
    remote_op_rx: Option<mpsc::Receiver<Vec<Operation>>>,
    /// Watch channel for sync status updates.
    sync_status_rx: Option<watch::Receiver<SyncStatus>>,
    /// Most recent sync status, cached for rendering.
    pub sync_status: SyncStatus,
    /// Channel for error strings from the sync task.
    sync_err_rx: Option<mpsc::Receiver<String>>,
    /// Current toast notification: (message, time it was set). Auto-dismisses after 5s.
    pub sync_toast: Option<(String, Instant)>,
}

struct NewItemCtx {
    item_id: Uuid,
    parent_id: Option<Uuid>,
    position: u32,
    status: String,
}

impl AppState {
    pub fn new(
        tabs: Vec<crate::todo::Tab>,
        mut tab_roots: HashMap<Uuid, Vec<TodoItem>>,
        statuses: Vec<Status>,
        db: Connection,
        device_id: Uuid,
        initial_seq: u64,
        initial_collapsed: HashSet<Uuid>,
        show_completed: bool,
        local_op_tx: Option<mpsc::Sender<Operation>>,
        remote_op_rx: Option<mpsc::Receiver<Vec<Operation>>>,
        sync_status_rx: Option<watch::Receiver<SyncStatus>>,
        sync_err_rx: Option<mpsc::Receiver<String>>,
    ) -> Self {
        let status_map: HashMap<String, Status> = statuses
            .into_iter()
            .map(|s| (s.name.clone(), s))
            .collect();
        // Load the first tab's roots as the active roots
        let active_tab_id = tabs.first().map(|t| t.id).unwrap_or(crate::todo::DEFAULT_TAB_ID);
        let roots = tab_roots.remove(&active_tab_id).unwrap_or_default();
        // Load views from DB
        let view_rows = storage::load_views(&db);
        let views: Vec<TagView> = view_rows
            .into_iter()
            .map(|(name, tag_filter)| TagView { name, tag_filter })
            .collect();

        let mut app = Self {
            tabs,
            tab_roots,
            active_tab_idx: 0,
            views,
            active_view: None,
            view_item_tab: HashMap::new(),
            roots,
            status_map,
            mode: Mode::Normal,
            cursor_idx: 0,
            visible_flat: Vec::new(),
            collapsed: initial_collapsed,
            tree_scroll: 0,
            search_query: None,
            pending_key: None,
            popup: None,
            status_message: None,
            should_quit: false,
            show_detail_panel: false,
            show_completed,
            db,
            device_id,
            next_seq: initial_seq,
            pending_new_item: None,
            local_op_tx,
            remote_op_rx,
            sync_status_rx,
            sync_status: SyncStatus::Disabled,
            sync_err_rx,
            sync_toast: None,
        };
        app.rebuild_visible();
        app
    }

    /// Poll channels from the background sync task. Call once per event-loop tick.
    pub fn poll_sync(&mut self) {
        // Update sync status
        if let Some(ref rx) = self.sync_status_rx {
            if rx.has_changed().unwrap_or(false) {
                self.sync_status = rx.borrow().clone();
            }
        }

        // Drain error messages — keep only the latest one, reset its timer
        if let Some(ref mut rx) = self.sync_err_rx {
            while let Ok(msg) = rx.try_recv() {
                self.sync_toast = Some((msg, Instant::now()));
            }
        }

        // Collect remote ops first (avoids re-borrowing self while iterating the channel)
        let mut all_remote: Vec<Vec<Operation>> = Vec::new();
        if let Some(ref mut rx) = self.remote_op_rx {
            while let Ok(ops) = rx.try_recv() {
                all_remote.push(ops);
            }
        }
        let had_remote = !all_remote.is_empty();
        for ops in all_remote {
            for op in &ops {
                // Skip our own ops (already applied locally)
                if op.device_id == self.device_id {
                    continue;
                }
                storage::apply_remote_op(&self.db, op);
                self.apply_remote_op_in_memory(op);
            }
        }
        if had_remote {
            self.rebuild_visible();
        }
    }

    /// Apply a remote op to the in-memory tree without going through local storage.
    fn apply_remote_op_in_memory(&mut self, op: &Operation) {
        use crate::todo::set_status_recursive;
        match &op.payload {
            OpPayload::CreateItem { item_id, parent_id, position, title, status, tags, .. } => {
                let new_item = TodoItem {
                    id: *item_id,
                    title: title.clone(),
                    description: None,
                    status: status.clone(),
                    tags: tags.clone(),
                    children: Vec::new(),
                    timer: yan_shared::models::TimerState::default(),
                    created_at: op.happened_at,
                    updated_at: op.happened_at,
                };
                if let Some(pid) = parent_id {
                    // Find parent and insert
                    if let Some(parent) = find_item_by_id_mut(&mut self.roots, pid) {
                        let pos = (*position as usize).min(parent.children.len());
                        parent.children.insert(pos, new_item);
                    }
                } else {
                    let pos = (*position as usize).min(self.roots.len());
                    self.roots.insert(pos, new_item);
                }
            }
            OpPayload::UpdateTitle { item_id, title } => {
                if let Some(item) = find_item_by_id_mut(&mut self.roots, item_id) {
                    item.title = title.clone();
                    item.updated_at = op.happened_at;
                }
            }
            OpPayload::UpdateDescription { item_id, description } => {
                if let Some(item) = find_item_by_id_mut(&mut self.roots, item_id) {
                    item.description = description.clone();
                    item.updated_at = op.happened_at;
                }
            }
            OpPayload::UpdateStatus { item_id, status, recursive } => {
                // Find the path for this item to use set_status_recursive
                if *recursive {
                    if let Some(item) = find_item_by_id_mut(&mut self.roots, item_id) {
                        set_status_recursive(item, status);
                    }
                } else if let Some(item) = find_item_by_id_mut(&mut self.roots, item_id) {
                    item.status = status.clone();
                    item.updated_at = op.happened_at;
                }
            }
            OpPayload::DeleteItem { item_id } => {
                remove_item_by_id(&mut self.roots, item_id);
            }
            OpPayload::MoveItem { item_id, new_parent_id, new_position } => {
                if let Some(item) = remove_item_by_id(&mut self.roots, item_id) {
                    let pos = *new_position as usize;
                    if let Some(pid) = new_parent_id {
                        if let Some(parent) = find_item_by_id_mut(&mut self.roots, pid) {
                            let pos = pos.min(parent.children.len());
                            parent.children.insert(pos, item);
                        }
                    } else {
                        let pos = pos.min(self.roots.len());
                        self.roots.insert(pos, item);
                    }
                }
            }
            OpPayload::TimerStart { item_id, started_at } => {
                if let Some(item) = find_item_by_id_mut(&mut self.roots, item_id) {
                    if !item.timer.is_running() {
                        item.timer.running_since = Some(*started_at);
                    }
                }
            }
            OpPayload::TimerStop { item_id, session_secs, .. } => {
                if let Some(item) = find_item_by_id_mut(&mut self.roots, item_id) {
                    item.timer.accumulated_secs += session_secs;
                    item.timer.running_since = None;
                }
            }
            OpPayload::UpdateTags { item_id, tags } => {
                if let Some(item) = find_item_by_id_mut(&mut self.roots, item_id) {
                    item.tags = tags.clone();
                    item.updated_at = op.happened_at;
                }
            }
            OpPayload::CreateTab { tab_id, name, color, position } => {
                if !self.tabs.iter().any(|t| t.id == *tab_id) {
                    self.tabs.push(crate::todo::Tab {
                        id: *tab_id,
                        name: name.clone(),
                        color: color.clone(),
                        position: *position,
                    });
                    self.tab_roots.insert(*tab_id, Vec::new());
                }
            }
            OpPayload::RenameTab { tab_id, name } => {
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == *tab_id) {
                    tab.name = name.clone();
                }
            }
            OpPayload::DeleteTab { tab_id } => {
                self.tabs.retain(|t| t.id != *tab_id);
                self.tab_roots.remove(tab_id);
            }
            OpPayload::UpsertStatus { name, color } => {
                self.status_map.insert(
                    name.clone(),
                    Status { name: name.clone(), color: color.clone() },
                );
            }
        }
    }

    // ── Op emission ───────────────────────────────────────────────────────────

    fn emit(&mut self, payload: OpPayload) {
        let seq = self.next_seq;
        self.next_seq += 1;
        let op = Operation::new(self.device_id, seq, payload);
        storage::write_op(&self.db, &op);
        // Forward to sync task (best-effort; fails silently if sync is disabled)
        if let Some(ref tx) = self.local_op_tx {
            let _ = tx.try_send(op);
        }
    }

    // ── Navigation ────────────────────────────────────────────────────────────

    pub fn rebuild_visible(&mut self) {
        self.visible_flat.clear();
        let search = self.search_query.as_deref();
        let hide_done = !self.show_completed;

        if self.active_view.is_some() {
            // View mode: self.roots contains the filtered view items
            for (i, root) in self.roots.iter().enumerate() {
                flatten_node(root, &[i], 0, &self.collapsed, &mut self.visible_flat, search, hide_done);
            }
        } else {
            for (i, root) in self.roots.iter().enumerate() {
                flatten_node(root, &[i], 0, &self.collapsed, &mut self.visible_flat, search, hide_done);
            }
        }

        if !self.visible_flat.is_empty() && self.cursor_idx >= self.visible_flat.len() {
            self.cursor_idx = self.visible_flat.len() - 1;
        }
    }

    /// Toggle the show/hide-completed flag, persist to config, rebuild the view,
    /// and clamp the cursor in case it now points past the last visible row.
    pub fn toggle_show_completed(&mut self) {
        self.show_completed = !self.show_completed;
        // Persist: load current config from disk, mutate, save. We do this rather
        // than threading a Config handle into AppState because no other state needs it.
        let mut cfg = crate::config::load();
        cfg.show_completed = self.show_completed;
        let _ = crate::config::save(&cfg);
        self.rebuild_visible();
        if self.visible_flat.is_empty() {
            self.cursor_idx = 0;
        } else if self.cursor_idx >= self.visible_flat.len() {
            self.cursor_idx = self.visible_flat.len() - 1;
        }
        self.update_scroll();
        self.status_message = Some(if self.show_completed {
            "Showing completed tasks".into()
        } else {
            "Hiding completed tasks".into()
        });
    }

    pub fn current_path(&self) -> Option<&CursorPath> {
        self.visible_flat.get(self.cursor_idx).map(|(_, p)| p)
    }

    pub fn current_item(&self) -> Option<&TodoItem> {
        let path = self.current_path()?;
        item_at(&self.roots, path)
    }

    pub fn move_cursor(&mut self, delta: isize) {
        if self.visible_flat.is_empty() {
            return;
        }
        let len = self.visible_flat.len() as isize;
        let new = (self.cursor_idx as isize + delta).clamp(0, len - 1);
        self.cursor_idx = new as usize;
        self.update_scroll();
    }

    pub fn move_to_top(&mut self) {
        self.cursor_idx = 0;
        self.tree_scroll = 0;
    }

    pub fn move_to_bottom(&mut self) {
        if !self.visible_flat.is_empty() {
            self.cursor_idx = self.visible_flat.len() - 1;
            self.update_scroll();
        }
    }

    fn update_scroll(&mut self) {
        let height = 20usize;
        if self.cursor_idx < self.tree_scroll {
            self.tree_scroll = self.cursor_idx;
        } else if self.cursor_idx >= self.tree_scroll + height {
            self.tree_scroll = self.cursor_idx + 1 - height;
        }
    }

    // ── Tree mutations ────────────────────────────────────────────────────────

    pub fn toggle_collapse(&mut self) {
        if let Some(path) = self.current_path().cloned() {
            if let Some(item) = item_at(&self.roots, &path) {
                if item.children.is_empty() {
                    return;
                }
                let id = item.id;
                if self.collapsed.contains(&id) {
                    self.collapsed.remove(&id);
                } else {
                    self.collapsed.insert(id);
                }
                self.rebuild_visible();
            }
        }
    }

    pub fn collapse_current(&mut self) {
        if let Some(path) = self.current_path().cloned() {
            if let Some(item) = item_at(&self.roots, &path) {
                if !item.children.is_empty() {
                    self.collapsed.insert(item.id);
                    self.rebuild_visible();
                    return;
                }
            }
            if path.len() > 1 {
                let parent_path = path[..path.len() - 1].to_vec();
                if let Some(idx) = self.visible_flat.iter().position(|(_, p)| p == &parent_path) {
                    self.cursor_idx = idx;
                    self.update_scroll();
                }
            }
        }
    }

    pub fn expand_current(&mut self) {
        if let Some(path) = self.current_path().cloned() {
            if let Some(item) = item_at(&self.roots, &path) {
                let id = item.id;
                if !item.children.is_empty() && self.collapsed.contains(&id) {
                    self.collapsed.remove(&id);
                    self.rebuild_visible();
                    return;
                }
                if !item.children.is_empty() {
                    if let Some(idx) = self
                        .visible_flat
                        .iter()
                        .position(|(_, p)| p.len() == path.len() + 1 && p.starts_with(&path))
                    {
                        self.cursor_idx = idx;
                        self.update_scroll();
                    }
                }
            }
        }
    }

    pub fn add_sibling_below(&mut self) {
        let new_item = TodoItem::new("", "Todo");
        let new_id = new_item.id;

        if self.visible_flat.is_empty() {
            let position = self.roots.len() as u32;
            self.roots.push(new_item);
            self.rebuild_visible();
            self.cursor_idx = self.visible_flat.len().saturating_sub(1);
            self.pending_new_item = Some(NewItemCtx {
                item_id: new_id,
                parent_id: None,
                position,
                status: "Todo".into(),
            });
        } else {
            let path = self.current_path().cloned().unwrap_or_default();
            let insert_idx = *path.last().unwrap_or(&0) + 1;

            let parent_id = if path.len() <= 1 {
                None
            } else {
                let parent_path = &path[..path.len() - 1];
                item_at(&self.roots, parent_path).map(|p| p.id)
            };

            if let Some((vec, _)) = parent_vec_mut(&mut self.roots, &path) {
                vec.insert(insert_idx, new_item);
            } else {
                self.roots.insert(insert_idx.min(self.roots.len()), new_item);
            }
            self.rebuild_visible();
            let new_path = if path.len() <= 1 {
                vec![insert_idx]
            } else {
                let mut p = path[..path.len() - 1].to_vec();
                p.push(insert_idx);
                p
            };
            if let Some(idx) = self.visible_flat.iter().position(|(_, p)| p == &new_path) {
                self.cursor_idx = idx;
            }
            self.pending_new_item = Some(NewItemCtx {
                item_id: new_id,
                parent_id,
                position: insert_idx as u32,
                status: "Todo".into(),
            });
        }
        self.rebuild_visible();
        self.open_edit_title_for_new();
    }

    pub fn add_child(&mut self) {
        let new_item = TodoItem::new("", "Todo");
        let new_id = new_item.id;

        if self.visible_flat.is_empty() {
            self.roots.push(new_item);
            self.rebuild_visible();
            self.cursor_idx = 0;
            self.pending_new_item = Some(NewItemCtx {
                item_id: new_id,
                parent_id: None,
                position: 0,
                status: "Todo".into(),
            });
        } else {
            let path = self.current_path().cloned().unwrap_or_default();
            if let Some(item) = item_at(&self.roots, &path) {
                self.collapsed.remove(&item.id);
            }
            let parent_id = item_at(&self.roots, &path).map(|p| p.id);
            if let Some(parent) = item_at_mut(&mut self.roots, &path) {
                let child_idx = parent.children.len();
                parent.children.push(new_item);
                let mut new_path = path.clone();
                new_path.push(child_idx);
                self.rebuild_visible();
                if let Some(idx) = self.visible_flat.iter().position(|(_, p)| p == &new_path) {
                    self.cursor_idx = idx;
                }
                self.pending_new_item = Some(NewItemCtx {
                    item_id: new_id,
                    parent_id,
                    position: child_idx as u32,
                    status: "Todo".into(),
                });
            }
        }
        self.rebuild_visible();
        self.open_edit_title_for_new();
    }

    pub fn indent_item(&mut self) {
        let path = match self.current_path().cloned() {
            Some(p) => p,
            None => return,
        };
        let idx = match path.last() {
            Some(&i) => i,
            None => return,
        };
        if idx == 0 {
            return;
        }

        let item_id = item_at(&self.roots, &path).map(|i| i.id);

        let item = {
            let (vec, i) = match parent_vec_mut(&mut self.roots, &path) {
                Some(x) => x,
                None => return,
            };
            vec.remove(i)
        };

        let new_parent_path = if path.len() == 1 {
            vec![idx - 1]
        } else {
            let mut p = path[..path.len() - 1].to_vec();
            p.push(idx - 1);
            p
        };
        if let Some(parent) = item_at_mut(&mut self.roots, &new_parent_path) {
            let child_count = parent.children.len();
            let new_parent_id = parent.id;
            parent.children.push(item);
            let mut new_path = new_parent_path.clone();
            new_path.push(child_count);
            self.rebuild_visible();
            if let Some(i) = self.visible_flat.iter().position(|(_, p)| p == &new_path) {
                self.cursor_idx = i;
            }
            if let Some(iid) = item_id {
                self.emit(OpPayload::MoveItem {
                    item_id: iid,
                    new_parent_id: Some(new_parent_id),
                    new_position: child_count as u32,
                });
            }
        }
    }

    pub fn dedent_item(&mut self) {
        let path = match self.current_path().cloned() {
            Some(p) => p,
            None => return,
        };
        if path.len() <= 1 {
            return;
        }

        let item_id = item_at(&self.roots, &path).map(|i| i.id);

        let item = {
            let (vec, i) = match parent_vec_mut(&mut self.roots, &path) {
                Some(x) => x,
                None => return,
            };
            vec.remove(i)
        };

        let parent_path = &path[..path.len() - 1];
        let parent_idx = *parent_path.last().unwrap_or(&0);
        let insert_idx = parent_idx + 1;

        if parent_path.len() == 1 {
            self.roots.insert(insert_idx.min(self.roots.len()), item);
            let new_path = vec![insert_idx];
            self.rebuild_visible();
            if let Some(i) = self.visible_flat.iter().position(|(_, p)| p == &new_path) {
                self.cursor_idx = i;
            }
            if let Some(iid) = item_id {
                self.emit(OpPayload::MoveItem {
                    item_id: iid,
                    new_parent_id: None,
                    new_position: insert_idx as u32,
                });
            }
        } else {
            let grandparent_path = &parent_path[..parent_path.len() - 1];
            let grandparent_id = item_at(&self.roots, grandparent_path).map(|g| g.id);
            if let Some(gp) = item_at_mut(&mut self.roots, grandparent_path) {
                gp.children.insert(insert_idx.min(gp.children.len()), item);
                let mut new_path = grandparent_path.to_vec();
                new_path.push(insert_idx);
                self.rebuild_visible();
                if let Some(i) = self.visible_flat.iter().position(|(_, p)| p == &new_path) {
                    self.cursor_idx = i;
                }
                if let Some(iid) = item_id {
                    self.emit(OpPayload::MoveItem {
                        item_id: iid,
                        new_parent_id: grandparent_id,
                        new_position: insert_idx as u32,
                    });
                }
            }
        }
    }

    pub fn move_item_down(&mut self) {
        let path = match self.current_path().cloned() {
            Some(p) => p,
            None => return,
        };
        let idx = *path.last().unwrap_or(&0);
        let parent_id = if path.len() <= 1 {
            None
        } else {
            item_at(&self.roots, &path[..path.len() - 1]).map(|p| p.id)
        };
        let (id_a, id_b) = match parent_vec_mut(&mut self.roots, &path) {
            Some((vec, i)) => {
                if i + 1 >= vec.len() {
                    return;
                }
                let (a, b) = (vec[i].id, vec[i + 1].id);
                vec.swap(i, i + 1);
                (a, b)
            }
            None => return,
        };
        self.emit(OpPayload::MoveItem { item_id: id_a, new_parent_id: parent_id, new_position: (idx + 1) as u32 });
        self.emit(OpPayload::MoveItem { item_id: id_b, new_parent_id: parent_id, new_position: idx as u32 });
        let mut new_path = path;
        *new_path.last_mut().unwrap() = idx + 1;
        self.rebuild_visible();
        if let Some(i) = self.visible_flat.iter().position(|(_, p)| p == &new_path) {
            self.cursor_idx = i;
            self.update_scroll();
        }
    }

    pub fn move_item_up(&mut self) {
        let path = match self.current_path().cloned() {
            Some(p) => p,
            None => return,
        };
        let idx = *path.last().unwrap_or(&0);
        if idx == 0 {
            return;
        }
        let parent_id = if path.len() <= 1 {
            None
        } else {
            item_at(&self.roots, &path[..path.len() - 1]).map(|p| p.id)
        };
        let (id_a, id_b) = match parent_vec_mut(&mut self.roots, &path) {
            Some((vec, i)) => {
                if i == 0 {
                    return;
                }
                let (a, b) = (vec[i].id, vec[i - 1].id);
                vec.swap(i - 1, i);
                (a, b)
            }
            None => return,
        };
        self.emit(OpPayload::MoveItem { item_id: id_a, new_parent_id: parent_id, new_position: (idx - 1) as u32 });
        self.emit(OpPayload::MoveItem { item_id: id_b, new_parent_id: parent_id, new_position: idx as u32 });
        let mut new_path = path;
        *new_path.last_mut().unwrap() = idx - 1;
        self.rebuild_visible();
        if let Some(i) = self.visible_flat.iter().position(|(_, p)| p == &new_path) {
            self.cursor_idx = i;
            self.update_scroll();
        }
    }

    pub fn toggle_done(&mut self) {
        let path = match self.current_path().cloned() {
            Some(p) => p,
            None => return,
        };
        let current_status = item_at(&self.roots, &path)
            .map(|i| i.status.clone())
            .unwrap_or_default();
        let next = if current_status == "Done" {
            "Todo".to_string()
        } else {
            "Done".to_string()
        };
        self.apply_set_status(next);
    }

    pub fn apply_set_status(&mut self, status: String) {
        let path = match self.current_path().cloned() {
            Some(p) => p,
            None => return,
        };
        let item_id = match item_at(&self.roots, &path) {
            Some(i) => i.id,
            None => return,
        };
        if let Some(item) = item_at_mut(&mut self.roots, &path) {
            set_status_recursive(item, &status);
        }
        check_parent_completion(&mut self.roots, &path);
        self.rebuild_visible();
        self.emit(OpPayload::UpdateStatus {
            item_id,
            status,
            recursive: true,
        });
    }

    pub fn open_status_picker(&mut self) {
        let mut options: Vec<String> = self.status_map.keys().cloned().collect();
        options.sort();
        options.push("+ Add new status".to_string());
        self.popup = Some(PopupKind::SetStatus { options, selected: 0 });
    }

    pub fn open_add_status(&mut self) {
        self.popup = Some(PopupKind::AddStatus {
            textarea: TextArea::default(),
            color_buf: "white".to_string(),
        });
    }

    pub fn add_custom_status(&mut self, name: String, color: String) {
        let status = Status { name: name.clone(), color: color.clone() };
        self.status_map.insert(name.clone(), status);
        self.emit(OpPayload::UpsertStatus { name, color });
    }

    /// Remove a custom status. Built-in statuses cannot be removed.
    /// Returns true if the status was removed.
    pub fn remove_status(&mut self, name: &str) -> bool {
        const BUILTINS: &[&str] = &["Todo", "In Progress", "Done", "Blocked", "Cancelled"];
        if BUILTINS.contains(&name) {
            return false;
        }
        self.status_map.remove(name);
        storage::delete_status(&self.db, name);
        true
    }

    // ── Tabs ─────────────────────────────────────────────────────────────

    pub fn active_tab_id(&self) -> Uuid {
        self.tabs.get(self.active_tab_idx).map(|t| t.id).unwrap_or(crate::todo::DEFAULT_TAB_ID)
    }

    pub fn active_tab_name(&self) -> &str {
        self.tabs.get(self.active_tab_idx).map(|t| t.name.as_str()).unwrap_or("Default")
    }

    pub fn switch_to_tab(&mut self, idx: usize) {
        if idx == self.active_tab_idx || idx >= self.tabs.len() {
            return;
        }
        // Save current tab's roots
        let old_tab_id = self.active_tab_id();
        let old_roots = std::mem::take(&mut self.roots);
        self.tab_roots.insert(old_tab_id, old_roots);
        // Load new tab's roots
        self.active_tab_idx = idx;
        let new_tab_id = self.active_tab_id();
        self.roots = self.tab_roots.remove(&new_tab_id).unwrap_or_default();
        self.cursor_idx = 0;
        self.tree_scroll = 0;
        self.rebuild_visible();
    }

    pub fn next_tab(&mut self) {
        if self.tabs.len() > 1 {
            let next = (self.active_tab_idx + 1) % self.tabs.len();
            self.switch_to_tab(next);
        }
    }

    pub fn prev_tab(&mut self) {
        if self.tabs.len() > 1 {
            let prev = if self.active_tab_idx == 0 {
                self.tabs.len() - 1
            } else {
                self.active_tab_idx - 1
            };
            self.switch_to_tab(prev);
        }
    }

    pub fn create_tab(&mut self, name: String) {
        let position = self.tabs.len() as u32;
        let tab = crate::todo::Tab::new(name.clone(), position);
        let tab_id = tab.id;
        let color = tab.color.clone();
        self.tabs.push(tab);
        self.tab_roots.insert(tab_id, Vec::new());
        self.emit(OpPayload::CreateTab {
            tab_id,
            name,
            color,
            position,
        });
        // Switch to the new tab
        self.switch_to_tab(self.tabs.len() - 1);
    }

    pub fn open_create_tab(&mut self) {
        let ta = new_textarea("");
        self.popup = Some(PopupKind::CreateTabName { textarea: ta });
        self.mode = Mode::Insert;
    }

    pub fn apply_create_tab(&mut self, name: String) {
        if !name.is_empty() {
            self.create_tab(name);
        }
        self.status_message = None;
    }

    pub fn open_rename_tab(&mut self) {
        let current_name = self.active_tab_name().to_string();
        let mut ta = new_textarea(&current_name);
        ta.move_cursor(ratatui_textarea::CursorMove::End);
        self.popup = Some(PopupKind::RenameTab { textarea: ta });
        self.mode = Mode::Insert;
    }

    pub fn apply_rename_tab(&mut self, name: String) {
        if name.is_empty() {
            return;
        }
        let tab_id = self.active_tab_id();
        if let Some(tab) = self.tabs.get_mut(self.active_tab_idx) {
            tab.name = name.clone();
        }
        self.emit(OpPayload::RenameTab { tab_id, name });
    }

    pub fn open_confirm_delete_tab(&mut self) {
        if self.tabs.len() <= 1 {
            self.status_message = Some("Cannot delete the last tab".into());
            return;
        }
        self.popup = Some(PopupKind::ConfirmDeleteTab);
    }

    pub fn delete_current_tab(&mut self) {
        if self.tabs.len() <= 1 {
            return;
        }
        let tab_id = self.active_tab_id();
        self.tabs.remove(self.active_tab_idx);
        // Don't need to remove from tab_roots since active tab roots are in self.roots
        self.emit(OpPayload::DeleteTab { tab_id });
        // Switch to previous tab or first
        let new_idx = if self.active_tab_idx >= self.tabs.len() {
            self.tabs.len() - 1
        } else {
            self.active_tab_idx
        };
        // Load the new active tab's roots
        self.active_tab_idx = new_idx;
        let new_tab_id = self.active_tab_id();
        self.roots = self.tab_roots.remove(&new_tab_id).unwrap_or_default();
        self.cursor_idx = 0;
        self.tree_scroll = 0;
        self.rebuild_visible();
    }

    pub fn open_move_to_tab(&mut self) {
        if self.tabs.len() <= 1 || self.visible_flat.is_empty() {
            return;
        }
        let options: Vec<(String, usize)> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != self.active_tab_idx)
            .map(|(i, t)| (t.name.clone(), i))
            .collect();
        if options.is_empty() {
            return;
        }
        self.popup = Some(PopupKind::TabPicker { options, selected: 0 });
    }

    pub fn move_item_to_tab(&mut self, target_tab_idx: usize) {
        let path = match self.current_path().cloned() {
            Some(p) => p,
            None => return,
        };
        // Remove item from current tree
        let (vec, idx) = match parent_vec_mut(&mut self.roots, &path) {
            Some(v) => v,
            None => return,
        };
        let item = vec.remove(idx);
        let target_tab_id = self.tabs[target_tab_idx].id;
        // Insert into target tab's roots
        let target_roots = self.tab_roots.entry(target_tab_id).or_default();
        let _position = target_roots.len() as u32;
        target_roots.push(item);
        // Fix cursor
        if self.cursor_idx >= self.visible_flat.len().saturating_sub(1) && self.cursor_idx > 0 {
            self.cursor_idx -= 1;
        }
        self.rebuild_visible();
    }

    // ── Tags ─────────────────────────────────────────────────────────────

    pub fn open_tag_editor(&mut self) {
        let existing = self
            .current_item()
            .map(|i| i.tags.clone())
            .unwrap_or_default();
        let ta = new_textarea("");
        self.popup = Some(PopupKind::EditTags {
            textarea: ta,
            existing,
            selected: 0,
        });
        self.mode = Mode::Insert;
    }

    pub fn apply_tags(&mut self, tags: Vec<String>) {
        let path = match self.current_path().cloned() {
            Some(p) => p,
            None => return,
        };
        let item_id = match item_at(&self.roots, &path) {
            Some(i) => i.id,
            None => return,
        };
        if let Some(item) = item_at_mut(&mut self.roots, &path) {
            item.tags = tags.clone();
            item.updated_at = Utc::now();
        }
        self.emit(OpPayload::UpdateTags { item_id, tags });
        self.rebuild_visible();
    }

    // ── Views ────────────────────────────────────────────────────────────

    pub fn open_view_picker(&mut self) {
        let mut options: Vec<String> = self.views.iter().map(|v| v.name.clone()).collect();
        options.push("+ New view...".to_string());
        self.popup = Some(PopupKind::ViewPicker { options, selected: 0 });
    }

    pub fn activate_view(&mut self, idx: usize) {
        if idx >= self.views.len() {
            return;
        }
        // If already in a view, deactivate first to restore tab roots
        if self.active_view.is_some() {
            self.deactivate_view();
        }
        // Save current tab's roots
        let tab_id = self.active_tab_id();
        let tab_roots = std::mem::take(&mut self.roots);
        self.tab_roots.insert(tab_id, tab_roots);

        self.active_view = Some(idx);
        self.rebuild_view_roots();
        self.cursor_idx = 0;
        self.tree_scroll = 0;
        self.rebuild_visible();
    }

    pub fn deactivate_view(&mut self) {
        if self.active_view.is_some() {
            self.active_view = None;
            self.view_item_tab.clear();
            // Restore active tab's roots
            let tab_id = self.active_tab_id();
            self.roots = self.tab_roots.remove(&tab_id).unwrap_or_default();
            self.cursor_idx = 0;
            self.tree_scroll = 0;
            self.rebuild_visible();
        }
    }

    /// Build self.roots from all tabs' items matching the active view's tag filter.
    fn rebuild_view_roots(&mut self) {
        let tag_filter = match self.active_view.and_then(|i| self.views.get(i)) {
            Some(v) => v.tag_filter.clone(),
            None => return,
        };
        self.view_item_tab.clear();
        let mut items = Vec::new();
        for tab in &self.tabs {
            let roots = self.tab_roots.get(&tab.id).map(|r| r.as_slice()).unwrap_or(&[]);
            collect_tagged_items(roots, &tag_filter, &mut items, tab.id, &mut self.view_item_tab);
        }
        self.roots = items;
    }

    pub fn open_create_view(&mut self) {
        let ta = new_textarea("");
        self.popup = Some(PopupKind::CreateView { textarea: ta });
        self.mode = Mode::Insert;
    }

    pub fn apply_create_view(&mut self, tag: String) {
        if tag.is_empty() {
            return;
        }
        // Don't create duplicate
        if self.views.iter().any(|v| v.tag_filter == tag) {
            return;
        }
        let view = TagView {
            name: tag.clone(),
            tag_filter: tag.clone(),
        };
        storage::save_view(&self.db, &view.name, &view.tag_filter);
        self.views.push(view);
        // Activate the newly created view
        self.activate_view(self.views.len() - 1);
    }

    pub fn delete_view(&mut self, idx: usize) {
        if idx >= self.views.len() {
            return;
        }
        let was_active = self.active_view == Some(idx);
        // Deactivate first if this view is active (restores real roots)
        if was_active {
            self.deactivate_view();
        }
        let name = self.views[idx].name.clone();
        storage::delete_view(&self.db, &name);
        self.views.remove(idx);
    }

    pub fn toggle_timer(&mut self) {
        let path = match self.current_path().cloned() {
            Some(p) => p,
            None => return,
        };
        let item_id = match item_at(&self.roots, &path) {
            Some(i) => i.id,
            None => return,
        };
        if let Some(item) = item_at_mut(&mut self.roots, &path) {
            if item.timer.is_running() {
                let session_secs = item.timer.stop_and_session_secs();
                let stopped_at = Utc::now();
                self.emit(OpPayload::TimerStop { item_id, stopped_at, session_secs });
            } else {
                let started_at = Utc::now();
                item.timer.start();
                self.emit(OpPayload::TimerStart { item_id, started_at });
            }
        }
    }

    pub fn stop_all_timers(&mut self) {
        stop_timers_recursive_emit(&mut self.roots, &self.db, self.device_id, &mut self.next_seq);
    }

    pub fn open_edit_description(&mut self) {
        let current_desc = self
            .current_item()
            .and_then(|i| i.description.clone())
            .unwrap_or_default();
        let mut ta = new_textarea(&current_desc);
        ta.move_cursor(ratatui_textarea::CursorMove::End);
        self.popup = Some(PopupKind::EditDescription { textarea: ta });
        self.mode = Mode::Insert;
    }

    pub fn open_edit_title(&mut self) {
        let current_title = self
            .current_item()
            .map(|i| i.title.clone())
            .unwrap_or_default();
        let mut ta = new_textarea(&current_title);
        ta.move_cursor(ratatui_textarea::CursorMove::End);
        self.popup = Some(PopupKind::EditTitle { textarea: ta });
        self.mode = Mode::Insert;
    }

    fn open_edit_title_for_new(&mut self) {
        let mut ta = TextArea::default();
        ta.move_cursor(ratatui_textarea::CursorMove::End);
        self.popup = Some(PopupKind::EditTitle { textarea: ta });
        self.mode = Mode::Insert;
    }

    /// Cancel an in-progress title edit. If a new item placeholder exists, remove it.
    pub fn cancel_edit_title(&mut self) {
        if let Some(_ctx) = self.pending_new_item.take() {
            // Remove the placeholder item from the tree
            if let Some(path) = self.current_path().cloned() {
                self.delete_at_path_no_op(&path);
            }
        }
    }

    pub fn apply_edit_title(&mut self, title: String) {
        let path = match self.current_path().cloned() {
            Some(p) => p,
            None => return,
        };

        if let Some(ctx) = self.pending_new_item.take() {
            // This is a brand-new item being committed for the first time.
            if title.is_empty() {
                // Cancelled — remove the placeholder from the tree (no op emitted).
                self.delete_at_path_no_op(&path);
            } else {
                // Commit: set the title in memory and emit CreateItem.
                if let Some(item) = item_at_mut(&mut self.roots, &path) {
                    item.title = title.clone();
                    item.updated_at = Utc::now();
                }
                self.emit(OpPayload::CreateItem {
                    item_id: ctx.item_id,
                    parent_id: ctx.parent_id,
                    position: ctx.position,
                    title,
                    status: ctx.status,
                    tags: Vec::new(),
                    tab_id: Some(self.active_tab_id()),
                });
            }
        } else {
            // Editing an existing item.
            let item_id = match item_at(&self.roots, &path) {
                Some(i) => i.id,
                None => return,
            };
            if title.is_empty() {
                self.delete_at_path(&path);
            } else {
                if let Some(item) = item_at_mut(&mut self.roots, &path) {
                    item.title = title.clone();
                    item.updated_at = Utc::now();
                }
                self.emit(OpPayload::UpdateTitle { item_id, title });
            }
        }

        self.rebuild_visible();
    }

    pub fn apply_edit_description(&mut self, desc: Option<String>) {
        let path = match self.current_path().cloned() {
            Some(p) => p,
            None => return,
        };
        let item_id = match item_at(&self.roots, &path) {
            Some(i) => i.id,
            None => return,
        };
        if let Some(item) = item_at_mut(&mut self.roots, &path) {
            item.description = desc.clone();
            item.updated_at = Utc::now();
        }
        self.emit(OpPayload::UpdateDescription { item_id, description: desc });
    }

    pub fn open_confirm_delete(&mut self) {
        self.popup = Some(PopupKind::ConfirmDelete);
    }

    pub fn delete_current(&mut self) {
        let path = match self.current_path().cloned() {
            Some(p) => p,
            None => return,
        };
        self.delete_at_path(&path);
    }

    fn delete_at_path(&mut self, path: &[usize]) {
        let item_id = item_at(&self.roots, path).map(|i| i.id);
        if let Some((vec, idx)) = parent_vec_mut(&mut self.roots, path) {
            if idx < vec.len() {
                vec.remove(idx);
            }
        }
        self.rebuild_visible();
        if self.cursor_idx > 0 && self.cursor_idx >= self.visible_flat.len() {
            self.cursor_idx = self.visible_flat.len().saturating_sub(1);
        }
        if let Some(iid) = item_id {
            self.emit(OpPayload::DeleteItem { item_id: iid });
        }
    }

    /// Delete from tree without emitting an op (used for cancelled new items).
    fn delete_at_path_no_op(&mut self, path: &[usize]) {
        if let Some((vec, idx)) = parent_vec_mut(&mut self.roots, path) {
            if idx < vec.len() {
                vec.remove(idx);
            }
        }
        self.rebuild_visible();
        if self.cursor_idx > 0 && self.cursor_idx >= self.visible_flat.len() {
            self.cursor_idx = self.visible_flat.len().saturating_sub(1);
        }
    }

    pub fn next_match(&mut self) {
        let query = match &self.search_query {
            Some(q) if !q.is_empty() => q.clone(),
            _ => return,
        };
        let start = self.cursor_idx + 1;
        let len = self.visible_flat.len();
        for i in 0..len {
            let idx = (start + i) % len;
            let path = &self.visible_flat[idx].1;
            if let Some(item) = item_at(&self.roots, path) {
                if item.title.to_lowercase().contains(&query.to_lowercase()) {
                    self.cursor_idx = idx;
                    self.update_scroll();
                    return;
                }
            }
        }
    }

    pub fn prev_match(&mut self) {
        let query = match &self.search_query {
            Some(q) if !q.is_empty() => q.clone(),
            _ => return,
        };
        let len = self.visible_flat.len();
        if len == 0 {
            return;
        }
        for i in 1..=len {
            let idx = (self.cursor_idx + len - i) % len;
            let path = &self.visible_flat[idx].1;
            if let Some(item) = item_at(&self.roots, path) {
                if item.title.to_lowercase().contains(&query.to_lowercase()) {
                    self.cursor_idx = idx;
                    self.update_scroll();
                    return;
                }
            }
        }
    }

    /// Persist timer state, statuses, and collapsed state to DB on exit.
    pub fn save_to_db(&mut self) {
        // If in a view, deactivate it first so self.roots has real tab data
        if self.active_view.is_some() {
            self.deactivate_view();
        }
        storage::save_tree(
            &self.db,
            &self.tabs,
            &self.tab_roots,
            self.active_tab_id(),
            &self.roots,
            &self.status_map,
        );
        storage::save_collapse_state(&self.db, &self.collapsed);
    }

    pub fn toggle_detail_panel(&mut self) {
        self.show_detail_panel = !self.show_detail_panel;
    }

    pub fn save_and_quit(&mut self) {
        self.save_to_db();
        self.should_quit = true;
    }
}

fn find_item_by_id_mut<'a>(items: &'a mut Vec<TodoItem>, id: &Uuid) -> Option<&'a mut TodoItem> {
    for item in items.iter_mut() {
        if item.id == *id {
            return Some(item);
        }
        if let Some(found) = find_item_by_id_mut(&mut item.children, id) {
            return Some(found);
        }
    }
    None
}

/// Remove and return an item by UUID from anywhere in the tree.
fn remove_item_by_id(items: &mut Vec<TodoItem>, id: &Uuid) -> Option<TodoItem> {
    for i in 0..items.len() {
        if items[i].id == *id {
            return Some(items.remove(i));
        }
        if let Some(found) = remove_item_by_id(&mut items[i].children, id) {
            return Some(found);
        }
    }
    None
}

/// Collect items (and their subtrees) that have the given tag.
fn collect_tagged_items(
    items: &[TodoItem],
    tag: &str,
    out: &mut Vec<TodoItem>,
    tab_id: Uuid,
    item_tab_map: &mut HashMap<Uuid, Uuid>,
) {
    for item in items {
        if item.tags.iter().any(|t| t == tag) {
            item_tab_map.insert(item.id, tab_id);
            // Clone the item without children (flatten for view)
            let mut view_item = item.clone();
            view_item.children.clear();
            out.push(view_item);
        }
        // Also check children
        collect_tagged_items(&item.children, tag, out, tab_id, item_tab_map);
    }
}

/// Stop all running timers, emitting a TimerStop op for each one.
/// Extracted as a free function to avoid borrowing issues in `stop_all_timers`.
fn stop_timers_recursive_emit(
    items: &mut Vec<TodoItem>,
    db: &Connection,
    device_id: Uuid,
    next_seq: &mut u64,
) {
    for item in items.iter_mut() {
        if item.timer.is_running() {
            let session_secs = item.timer.stop_and_session_secs();
            let stopped_at = Utc::now();
            let seq = *next_seq;
            *next_seq += 1;
            let op = Operation::new(device_id, seq, OpPayload::TimerStop {
                item_id: item.id,
                stopped_at,
                session_secs,
            });
            storage::write_op(db, &op);
        }
        stop_timers_recursive_emit(&mut item.children, db, device_id, next_seq);
    }
}
