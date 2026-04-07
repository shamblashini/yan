use std::collections::HashSet;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Status ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Status {
    pub name: String,
    /// Color: named ("green", "red") or hex ("#ff8800")
    pub color: String,
}

impl Status {
    pub fn defaults() -> Vec<Status> {
        vec![
            Status { name: "Todo".into(),        color: "white".into()     },
            Status { name: "In Progress".into(), color: "yellow".into()    },
            Status { name: "Done".into(),        color: "green".into()     },
            Status { name: "Blocked".into(),     color: "red".into()       },
            Status { name: "Cancelled".into(),   color: "dark_gray".into() },
        ]
    }
}

// ── TimerState ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TimerState {
    pub accumulated_secs: i64,
    pub running_since: Option<DateTime<Utc>>,
}

impl TimerState {
    pub fn start(&mut self) {
        if self.running_since.is_none() {
            self.running_since = Some(Utc::now());
        }
    }

    pub fn stop(&mut self) {
        if let Some(since) = self.running_since.take() {
            let delta = (Utc::now() - since).num_seconds().max(0);
            self.accumulated_secs += delta;
        }
    }

    /// Returns the session seconds that just elapsed (only meaningful right after stop).
    pub fn stop_and_session_secs(&mut self) -> i64 {
        if let Some(since) = self.running_since.take() {
            let delta = (Utc::now() - since).num_seconds().max(0);
            self.accumulated_secs += delta;
            delta
        } else {
            0
        }
    }

    pub fn elapsed(&self) -> Duration {
        let extra = self
            .running_since
            .map(|s| (Utc::now() - s).num_seconds().max(0))
            .unwrap_or(0);
        Duration::seconds(self.accumulated_secs + extra)
    }

    pub fn is_running(&self) -> bool {
        self.running_since.is_some()
    }
}

// ── TodoItem ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub children: Vec<TodoItem>,
    pub timer: TimerState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TodoItem {
    pub fn new(title: impl Into<String>, default_status: &str) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            title: title.into(),
            description: None,
            status: default_status.to_string(),
            children: Vec::new(),
            timer: TimerState::default(),
            created_at: now,
            updated_at: now,
        }
    }
}

pub type CursorPath = Vec<usize>;

// ── Tree traversal ────────────────────────────────────────────────────────────

pub fn item_at<'a>(roots: &'a [TodoItem], path: &[usize]) -> Option<&'a TodoItem> {
    let (&head, tail) = path.split_first()?;
    let node = roots.get(head)?;
    if tail.is_empty() { Some(node) } else { item_at(&node.children, tail) }
}

pub fn item_at_mut<'a>(roots: &'a mut Vec<TodoItem>, path: &[usize]) -> Option<&'a mut TodoItem> {
    let (&head, tail) = path.split_first()?;
    let node = roots.get_mut(head)?;
    if tail.is_empty() { Some(node) } else { item_at_mut(&mut node.children, tail) }
}

/// Returns (&mut parent_vec, index_of_item_within_that_vec)
pub fn parent_vec_mut<'a>(
    roots: &'a mut Vec<TodoItem>,
    path: &[usize],
) -> Option<(&'a mut Vec<TodoItem>, usize)> {
    match path {
        [] => None,
        [idx] => Some((roots, *idx)),
        [head, rest @ ..] => {
            let node = roots.get_mut(*head)?;
            parent_vec_mut(&mut node.children, rest)
        }
    }
}

pub fn set_status_recursive(item: &mut TodoItem, status: &str) {
    item.status = status.to_string();
    item.updated_at = Utc::now();
    for child in &mut item.children {
        set_status_recursive(child, status);
    }
}

/// After a status change at `path`, walk up and auto-complete parents if all children are Done.
pub fn check_parent_completion(roots: &mut Vec<TodoItem>, path: &[usize]) {
    if path.len() < 2 {
        return;
    }
    let parent_path = &path[..path.len() - 1];
    if let Some(parent) = item_at_mut(roots, parent_path) {
        let all_done = !parent.children.is_empty()
            && parent.children.iter().all(|c| c.status == "Done");
        if all_done && parent.status != "Done" {
            parent.status = "Done".to_string();
            parent.updated_at = Utc::now();
        }
        let any_not_done = parent
            .children
            .iter()
            .any(|c| c.status != "Done" && c.status != "Cancelled");
        if any_not_done && parent.status == "Done" {
            parent.status = "In Progress".to_string();
            parent.updated_at = Utc::now();
        }
    }
    check_parent_completion(roots, parent_path);
}

pub fn flatten_node(
    item: &TodoItem,
    path: &[usize],
    depth: usize,
    collapsed: &HashSet<Uuid>,
    out: &mut Vec<(usize, CursorPath)>,
    search: Option<&str>,
) {
    let matches_search = search.map_or(true, |q| subtree_matches(item, q));
    if !matches_search {
        return;
    }
    out.push((depth, path.to_vec()));
    if !collapsed.contains(&item.id) {
        for (i, child) in item.children.iter().enumerate() {
            let mut child_path = path.to_vec();
            child_path.push(i);
            flatten_node(child, &child_path, depth + 1, collapsed, out, search);
        }
    }
}

fn subtree_matches(item: &TodoItem, query: &str) -> bool {
    let q = query.to_lowercase();
    if item.title.to_lowercase().contains(&q) {
        return true;
    }
    item.children.iter().any(|c| subtree_matches(c, &q))
}

// ── Timer utilities ───────────────────────────────────────────────────────────

pub fn total_elapsed(item: &TodoItem) -> Duration {
    let own = item.timer.elapsed();
    item.children
        .iter()
        .fold(own, |acc, child| acc + total_elapsed(child))
}

pub fn format_duration(d: Duration) -> String {
    let total = d.num_seconds().max(0);
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{}h {}m", h, m)
    } else if m > 0 {
        format!("{}m {}s", m, s)
    } else {
        format!("{}s", s)
    }
}
