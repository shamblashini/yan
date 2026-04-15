use std::time::Duration;

use unicode_width::UnicodeWidthStr;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::app::{AppState, PopupKind};
use crate::sync_client::SyncStatus;
use crate::time_tracker::{format_duration, total_elapsed};
use crate::todo::item_at;

pub fn render(frame: &mut Frame, app: &mut AppState) {
    let size = frame.area();

    // Top-level: tab bar + content + status bar
    let has_tabs = app.tabs.len() > 1 || !app.views.is_empty();
    let tab_bar_height = if has_tabs { 1 } else { 0 };

    if app.show_detail_panel {
        // Sidebar mode: tab bar | tree (left) | detail panel (right) | status bar
        let outer_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(tab_bar_height),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(size);

        if has_tabs {
            render_tab_bar(frame, outer_chunks[0], app);
        }

        let main_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(outer_chunks[1]);

        render_tree(frame, main_chunks[0], app);
        render_detail_sidebar(frame, main_chunks[1], app);
        render_status_bar(frame, outer_chunks[2], app);
    } else {
        // Compact mode: tab bar | tree | detail strip | status bar
        let detail_height = app.current_item().map_or(0, |item| {
            let has_desc = item.description.as_ref().map_or(false, |d| !d.trim().is_empty());
            if has_desc { 5 } else { 4 }
        });

        let outer_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(tab_bar_height),
                Constraint::Min(1),
                Constraint::Length(detail_height),
                Constraint::Length(1),
            ])
            .split(size);

        if has_tabs {
            render_tab_bar(frame, outer_chunks[0], app);
        }
        render_tree(frame, outer_chunks[1], app);
        if detail_height > 0 {
            render_detail_strip(frame, outer_chunks[2], app);
        }
        render_status_bar(frame, outer_chunks[3], app);
    }

    if let Some(ref popup) = app.popup {
        match popup {
            PopupKind::EditTitle { .. } => render_edit_title_popup(frame, size, app),
            PopupKind::EditDescription { .. } => render_edit_desc_popup(frame, size, app),
            PopupKind::SetStatus { .. } => render_status_popup(frame, size, app),
            PopupKind::AddStatus { .. } => render_add_status_popup(frame, size, app),
            PopupKind::ConfirmDelete => render_confirm_delete_popup(frame, size),
            PopupKind::EditTags { .. } => render_tag_editor_popup(frame, size, app),
            PopupKind::CreateTabName { .. } => render_create_tab_popup(frame, size, app),
            PopupKind::RenameTab { .. } => render_rename_tab_popup(frame, size, app),
            PopupKind::TabPicker { .. } => render_tab_picker_popup(frame, size, app),
            PopupKind::ConfirmDeleteTab => render_confirm_delete_tab_popup(frame, size),
            PopupKind::ViewPicker { .. } => render_view_picker_popup(frame, size, app),
            PopupKind::CreateView { .. } => render_create_view_popup(frame, size, app),
            PopupKind::Help => render_help_popup(frame, size),
        }
    }

    // Toast notification (sync errors) — rendered last so it floats above everything
    if let Some((ref msg, ts)) = app.sync_toast {
        if ts.elapsed() < Duration::from_secs(5) {
            render_sync_toast(frame, size, msg);
        }
    }
}

fn render_tree(frame: &mut Frame, area: Rect, app: &mut AppState) {
    let title = if let Some(view_idx) = app.active_view {
        let name = app.views.get(view_idx).map(|v| v.name.as_str()).unwrap_or("View");
        format!(" View: {} ", name)
    } else {
        format!(" {} ", app.active_tab_name())
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.visible_flat.is_empty() {
        let hint = Paragraph::new("No todos.  a - new task | A - new child task")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(hint, inner);
        return;
    }

    let height = inner.height as usize;
    // Adjust scroll so cursor is visible
    if app.cursor_idx < app.tree_scroll {
        app.tree_scroll = app.cursor_idx;
    } else if app.cursor_idx >= app.tree_scroll + height {
        app.tree_scroll = app.cursor_idx + 1 - height;
    }

    let search = app.search_query.clone();
    let available_width = inner.width as usize;

    // Collect visible row data for two-pass rendering (needed for tag alignment).
    struct RowData {
        prefix_width: usize,
        tags: Vec<(String, Color)>,
        time_str: String,
        timer_running: bool,
        spans_before_tags: Vec<(String, Style)>,
    }

    let visible: Vec<_> = app
        .visible_flat
        .iter()
        .enumerate()
        .skip(app.tree_scroll)
        .take(height)
        .collect();

    let mut rows: Vec<RowData> = Vec::with_capacity(visible.len());

    for &(i, (depth, path)) in &visible {
        let item = match item_at(&app.roots, path) {
            Some(it) => it,
            None => {
                rows.push(RowData {
                    prefix_width: 0,
                    tags: vec![],
                    time_str: String::new(),
                    timer_running: false,
                    spans_before_tags: vec![("".into(), Style::default())],
                });
                continue;
            }
        };
        let indent = "  ".repeat(*depth);
        let icon = status_icon(&item.status);
        let status_color = parse_color(
            app.status_map
                .get(&item.status)
                .map(|s| s.color.as_str())
                .unwrap_or("white"),
        );
        let timer_icon = if item.timer.is_running() { "●" } else { "" };
        let total = total_elapsed(item);
        let time_str = if total.num_seconds() > 0 || item.timer.is_running() {
            format!(" {}{}", timer_icon, format_duration(total))
        } else {
            String::new()
        };
        let has_children = !item.children.is_empty();
        let collapsed = app.collapsed.contains(&item.id);
        let collapse_icon = if has_children {
            if collapsed { "▶ " } else { "▼ " }
        } else {
            "  "
        };

        let is_selected = i == app.cursor_idx;
        let title_style = if is_selected {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        let title_spans: Vec<(String, Style)> = if let Some(ref q) = search {
            if !q.is_empty() {
                highlight_match(&item.title, q, is_selected)
                    .into_iter()
                    .map(|s| (s.content.to_string(), s.style))
                    .collect()
            } else {
                vec![(item.title.clone(), title_style)]
            }
        } else {
            vec![(item.title.clone(), title_style)]
        };

        let select_indicator = if is_selected { "❯ " } else { "  " };
        let select_style = if is_selected { Style::default().fg(Color::Cyan) } else { Style::default() };

        let mut spans_before_tags: Vec<(String, Style)> = vec![
            (select_indicator.into(), select_style),
            (indent, Style::default()),
            (collapse_icon.into(), Style::default().fg(Color::DarkGray)),
            (format!("{} ", icon), Style::default().fg(status_color)),
        ];
        spans_before_tags.extend(title_spans);

        let prefix_width: usize = spans_before_tags.iter().map(|(s, _)| s.width()).sum();

        let tags: Vec<(String, Color)> = item
            .tags
            .iter()
            .map(|t| (format!("[{}]", t), tag_to_color(t)))
            .collect();

        rows.push(RowData {
            prefix_width,
            tags,
            time_str,
            timer_running: item.timer.is_running(),
            spans_before_tags,
        });
    }

    // Compute the tag alignment column: max prefix width among rows that have tags,
    // but cap it so tags + padding still fit within the available width.
    let max_prefix = rows
        .iter()
        .filter(|r| !r.tags.is_empty())
        .map(|r| r.prefix_width)
        .max()
        .unwrap_or(0);

    let max_tag_width: usize = rows
        .iter()
        .filter(|r| !r.tags.is_empty())
        .map(|r| {
            let tags_w: usize = r.tags.iter().map(|(t, _)| t.width() + 1).sum::<usize>();
            tags_w
        })
        .max()
        .unwrap_or(0);

    // Align column with at least 2 chars gap. If it would push tags off-screen, reduce it.
    let tag_col = if max_tag_width + max_prefix + 2 > available_width {
        available_width.saturating_sub(max_tag_width + 1)
    } else {
        max_prefix + 2
    };

    let items: Vec<ListItem> = rows
        .into_iter()
        .map(|row| {
            let mut spans: Vec<Span> = row
                .spans_before_tags
                .iter()
                .map(|(s, style)| Span::styled(s.clone(), *style))
                .collect();

            if !row.tags.is_empty() {
                let padding = tag_col.saturating_sub(row.prefix_width);
                if padding > 0 {
                    spans.push(Span::raw(" ".repeat(padding)));
                }
                for (j, (tag_text, tag_color)) in row.tags.iter().enumerate() {
                    let sep = if j > 0 { " " } else { "" };
                    spans.push(Span::styled(
                        format!("{}{}", sep, tag_text),
                        Style::default().fg(*tag_color),
                    ));
                }
            }

            if !row.time_str.is_empty() {
                let time_style = if row.timer_running {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                spans.push(Span::styled(row.time_str.clone(), time_style));
            }

            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, inner);
}

fn highlight_match<'a>(title: &str, query: &str, is_selected: bool) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    let lower_title = title.to_lowercase();
    let lower_query = query.to_lowercase();
    let mut last = 0;
    let base_style = if is_selected {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let match_style = if is_selected {
        Style::default().fg(Color::Black).bg(Color::Yellow)
    } else {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    };

    for (idx, _) in lower_title.match_indices(lower_query.as_str()) {
        if idx > last {
            spans.push(Span::styled(title[last..idx].to_string(), base_style));
        }
        spans.push(Span::styled(
            title[idx..idx + lower_query.len()].to_string(),
            match_style,
        ));
        last = idx + lower_query.len();
    }
    if last < title.len() {
        spans.push(Span::styled(title[last..].to_string(), base_style));
    }
    if spans.is_empty() {
        spans.push(Span::styled(title.to_string(), base_style));
    }
    spans
}

fn render_detail_sidebar(frame: &mut Frame, area: Rect, app: &AppState) {
    let item = match app.current_item() {
        Some(i) => i,
        None => {
            let block = Block::default()
                .title(" Detail ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            frame.render_widget(
                Paragraph::new("No task selected.").style(Style::default().fg(Color::DarkGray)),
                inner,
            );
            return;
        }
    };

    let status_color = parse_color(
        app.status_map.get(&item.status).map(|s| s.color.as_str()).unwrap_or("white"),
    );
    let own_time = item.timer.elapsed();
    let total_time = total_elapsed(item);
    let timer_running = item.timer.is_running();

    let block = Block::default()
        .title(Span::styled(
            format!(" {} ", item.title),
            Style::default().add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let dim = Style::default().fg(Color::DarkGray);

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(format!("{} ", status_icon(&item.status)), Style::default().fg(status_color)),
            Span::styled(item.status.clone(), Style::default().fg(status_color)),
        ]),
        Line::from(vec![
            Span::styled("created  ", dim),
            Span::raw(item.created_at.format("%Y-%m-%d %H:%M").to_string()),
        ]),
        Line::from(vec![
            Span::styled("updated  ", dim),
            Span::raw(item.updated_at.format("%Y-%m-%d %H:%M").to_string()),
        ]),
        Line::from(""),
    ];

    // Children progress
    if !item.children.is_empty() {
        let done = item.children.iter().filter(|c| c.status == "Done").count();
        lines.push(Line::from(vec![
            Span::styled("children ", dim),
            Span::raw(format!("{}/{} done", done, item.children.len())),
        ]));
        lines.push(Line::from(""));
    }

    // Timer
    let mut timer_spans = vec![
        Span::styled("time     ", dim),
        Span::raw(format_duration(own_time)),
    ];
    if total_time != own_time {
        timer_spans.push(Span::styled("  total ", dim));
        timer_spans.push(Span::raw(format_duration(total_time)));
    }
    lines.push(Line::from(timer_spans));
    if timer_running {
        lines.push(Line::from(Span::styled(
            "● RUNNING",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )));
    }

    // Tags
    if !item.tags.is_empty() {
        lines.push(Line::from(""));
        let mut tag_spans: Vec<Span> = vec![Span::styled("tags     ", dim)];
        for (i, tag) in item.tags.iter().enumerate() {
            if i > 0 {
                tag_spans.push(Span::raw(" "));
            }
            tag_spans.push(Span::styled(
                format!("[{}]", tag),
                Style::default().fg(tag_to_color(tag)),
            ));
        }
        lines.push(Line::from(tag_spans));
    }

    // Description
    if let Some(ref desc) = item.description {
        let trimmed = desc.trim();
        if !trimmed.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("description", dim)));
            for dline in trimmed.lines() {
                lines.push(Line::from(format!("  {}", dline)));
            }
        }
    }

    let para = Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false });
    frame.render_widget(para, inner);
}

fn render_detail_strip(frame: &mut Frame, area: Rect, app: &AppState) {
    let item = match app.current_item() {
        Some(i) => i,
        None => return,
    };

    let status_color = parse_color(
        app.status_map
            .get(&item.status)
            .map(|s| s.color.as_str())
            .unwrap_or("white"),
    );
    let own_time = item.timer.elapsed();
    let total_time = total_elapsed(item);
    let timer_running = item.timer.is_running();

    // Title in block header, truncated to fit
    let title_str = format!(" {} ", item.title);
    let block = Block::default()
        .title(Span::styled(title_str, Style::default().add_modifier(Modifier::BOLD)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Line 1: status | dates | children progress
    let children_info = if item.children.is_empty() {
        String::new()
    } else {
        let done = item.children.iter().filter(|c| c.status == "Done").count();
        format!("  │  children: {}/{} done", done, item.children.len())
    };
    let line1 = Line::from(vec![
        Span::styled(format!("{} ", status_icon(&item.status)), Style::default().fg(status_color)),
        Span::styled(item.status.clone(), Style::default().fg(status_color)),
        Span::styled("  │  ", Style::default().fg(Color::DarkGray)),
        Span::styled("created ", Style::default().fg(Color::DarkGray)),
        Span::raw(item.created_at.format("%Y-%m-%d").to_string()),
        Span::styled("  updated ", Style::default().fg(Color::DarkGray)),
        Span::raw(item.updated_at.format("%Y-%m-%d").to_string()),
        Span::styled(&children_info, Style::default().fg(Color::DarkGray)),
    ]);

    // Line 2: timer info
    let mut timer_spans = vec![
        Span::styled("time ", Style::default().fg(Color::DarkGray)),
        Span::raw(format_duration(own_time)),
        Span::styled("  total ", Style::default().fg(Color::DarkGray)),
        Span::raw(format_duration(total_time)),
    ];
    if timer_running {
        timer_spans.push(Span::styled(
            "  ● RUNNING",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
    }
    let line2 = Line::from(timer_spans);

    let mut lines = vec![line1, line2];

    // Line 3 (optional): description, truncated to one line
    if let Some(ref desc) = item.description {
        let first_line = desc.lines().next().unwrap_or("").trim();
        if !first_line.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("desc ", Style::default().fg(Color::DarkGray)),
                Span::raw(first_line.to_string()),
            ]));
        }
    }

    let para = Paragraph::new(lines);
    frame.render_widget(para, inner);
}

fn render_status_bar(frame: &mut Frame, area: Rect, app: &AppState) {
    let (mode_str, mode_color) = match app.mode {
        crate::input::Mode::Normal => ("NORMAL", Color::Green),
        crate::input::Mode::Insert => ("INSERT", Color::Yellow),
        crate::input::Mode::Search => ("SEARCH", Color::Cyan),
    };

    let hint = if let Some(ref m) = app.status_message {
        m.clone()
    } else if let Some(ref q) = app.search_query {
        format!("/{q}  (n/N: next/prev  Esc: clear)")
    } else {
        "a:add  dd:del  e:edit  #:tags  Tab:tabs  c:new tab  m:move  v:views  spc:done  s:status  t:timer  /:search  ?:help  q:quit".to_string()
    };

    let (sync_str, sync_color) = match &app.sync_status {
        SyncStatus::Disabled => (String::new(), Color::DarkGray),
        SyncStatus::Connected => ("[Synced]".into(), Color::Green),
        SyncStatus::Syncing => ("[Syncing…]".into(), Color::Yellow),
        SyncStatus::Offline { pending_ops: 0 } => ("[Offline]".into(), Color::Red),
        SyncStatus::Offline { pending_ops: n } => (format!("[Offline · {n} pending]"), Color::Red),
    };

    let mode_width = (mode_str.len() + 2) as u16;
    // Fixed width — wide enough for "[Offline · 999 pending]" + padding.
    // Keeping this constant prevents layout thrash as the pending count changes.
    const SYNC_COL_WIDTH: u16 = 28;

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(mode_width),
            Constraint::Min(1),
            Constraint::Length(SYNC_COL_WIDTH),
        ])
        .split(area);

    let mode_widget = Paragraph::new(format!(" {mode_str} "))
        .style(Style::default().fg(Color::Black).bg(mode_color).add_modifier(Modifier::BOLD));
    frame.render_widget(mode_widget, chunks[0]);

    let hint_widget = Paragraph::new(format!("  {hint}"))
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint_widget, chunks[1]);

    if !sync_str.is_empty() {
        let sync_widget = Paragraph::new(format!(" {sync_str} "))
            .style(Style::default().fg(sync_color));
        frame.render_widget(sync_widget, chunks[2]);
    }
}

fn render_sync_toast(frame: &mut Frame, area: Rect, msg: &str) {
    const POPUP_WIDTH: u16 = 38;
    const POPUP_HEIGHT: u16 = 3;
    if area.width < POPUP_WIDTH + 2 {
        return;
    }
    let x = area.x + area.width.saturating_sub(POPUP_WIDTH + 1);
    let y = area.y;
    let popup_area = Rect::new(x, y, POPUP_WIDTH, POPUP_HEIGHT.min(area.height));

    // Truncate message to fit inside the box (width - 2 borders - 2 padding)
    let max_len = (POPUP_WIDTH as usize).saturating_sub(4);
    let display = if msg.len() > max_len {
        format!("{}…", &msg[..max_len.saturating_sub(1)])
    } else {
        msg.to_string()
    };

    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .title(" Sync error ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);
    frame.render_widget(
        Paragraph::new(display).style(Style::default().fg(Color::Red)),
        inner,
    );
}

fn centered_rect(percent_x: u16, height: u16, r: Rect) -> Rect {
    let popup_width = r.width * percent_x / 100;
    let x = r.x + (r.width - popup_width) / 2;
    let y = r.y + r.height.saturating_sub(height) / 2;
    Rect::new(x, y, popup_width, height.min(r.height))
}

fn render_edit_title_popup(frame: &mut Frame, size: Rect, app: &AppState) {
    let area = centered_rect(60, 5, size);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(" Edit Title  [Enter] confirm  [Esc] cancel ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    if let Some(PopupKind::EditTitle { ref textarea }) = app.popup {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(textarea, inner);
    }
}

fn render_edit_desc_popup(frame: &mut Frame, size: Rect, app: &AppState) {
    let area = centered_rect(70, 12, size);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(" Edit Description  [Esc] confirm  [Ctrl-C] cancel ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    if let Some(PopupKind::EditDescription { ref textarea }) = app.popup {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(textarea, inner);
    }
}

fn render_status_popup(frame: &mut Frame, size: Rect, app: &AppState) {
    if let Some(PopupKind::SetStatus { ref options, selected }) = app.popup {
        let height = (options.len() as u16 + 2).min(size.height);
        let area = centered_rect(40, height, size);
        frame.render_widget(Clear, area);
        let block = Block::default()
            .title(" Set Status  [j/k] nav  [Enter] select  [d] delete  [Esc] cancel ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        const BUILTINS: &[&str] = &["Todo", "In Progress", "Done", "Blocked", "Cancelled"];
        let items: Vec<ListItem> = options
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let is_add = name == "+ Add new status";
                let is_builtin = BUILTINS.contains(&name.as_str());
                let color = if is_add {
                    Color::DarkGray
                } else {
                    app.status_map.get(name.as_str())
                        .map(|s| parse_color(&s.color))
                        .unwrap_or(Color::White)
                };
                let style = if i == selected {
                    Style::default().fg(Color::Black).bg(Color::White)
                } else {
                    Style::default().fg(color)
                };
                // Append a marker for built-ins to signal they can't be deleted
                let label = if is_builtin {
                    format!("{name} [built-in]")
                } else {
                    name.clone()
                };
                ListItem::new(Span::styled(label, style))
            })
            .collect();

        let mut state = ListState::default();
        state.select(Some(selected));
        let list = List::new(items);
        frame.render_stateful_widget(list, inner, &mut state);
    }
}

fn render_add_status_popup(frame: &mut Frame, size: Rect, app: &AppState) {
    let area = centered_rect(50, 7, size);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(" Add Status  [Enter] confirm  [Esc] cancel ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    if let Some(PopupKind::AddStatus { ref textarea, ref color_buf }) = app.popup {
        let inner_area = block.inner(area);
        frame.render_widget(block, area);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)])
            .split(inner_area);
        let label = Paragraph::new("Name:").style(Style::default().fg(Color::DarkGray));
        frame.render_widget(label, chunks[0]);
        frame.render_widget(textarea, chunks[1]);
        let color_hint = Paragraph::new(format!("Color: {} (edit in popup)", color_buf))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(color_hint, chunks[2]);
    }
}

fn render_confirm_delete_popup(frame: &mut Frame, size: Rect) {
    let area = centered_rect(50, 5, size);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(" Confirm Delete ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let text = Paragraph::new("Delete this todo and all its children?\n[y/Enter] yes   [n/Esc] cancel")
        .style(Style::default().fg(Color::White));
    frame.render_widget(text, inner);
}

fn render_help_popup(frame: &mut Frame, size: Rect) {
    let area = centered_rect(70, 38, size);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(" Help  [Esc/?/q] close ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let help = vec![
        Line::from(Span::styled("Navigation", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        Line::from("  j/k       Move cursor down/up"),
        Line::from("  h/l       Collapse/Expand"),
        Line::from("  gg/G      First/Last"),
        Line::from("  Enter     Toggle collapse"),
        Line::from(""),
        Line::from(Span::styled("Editing", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        Line::from("  a         Add sibling below"),
        Line::from("  A         Add child of current"),
        Line::from("  i/e       Edit title  E: Edit description"),
        Line::from("  dd        Delete (confirm)"),
        Line::from("  J/K       Move task down/up (same level)"),
        Line::from("  H/L       Dedent/Indent task  (also >/< )"),
        Line::from(""),
        Line::from(Span::styled("Tags", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        Line::from("  #         Edit tags on current task"),
        Line::from("            Enter: add tag  Ctrl+d: remove  Esc: confirm"),
        Line::from(""),
        Line::from(Span::styled("Tabs", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        Line::from("  Tab       Next tab"),
        Line::from("  Shift+Tab Previous tab"),
        Line::from("  c         Create new tab"),
        Line::from("  r         Rename current tab"),
        Line::from("  m         Move task to another tab"),
        Line::from("  X         Delete current tab"),
        Line::from(""),
        Line::from(Span::styled("Views", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        Line::from("  v         Open view picker / create view"),
        Line::from("  Esc       Exit current view"),
        Line::from(""),
        Line::from(Span::styled("Status & Time", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        Line::from("  Space     Toggle Done / Todo"),
        Line::from("  s         Status picker     t/T: timer / stop all"),
        Line::from(""),
        Line::from(Span::styled("Search & UI", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        Line::from("  /         Search  n/N next/prev"),
        Line::from("  p         Toggle detail panel"),
        Line::from("  q         Save & quit"),
    ];

    let para = Paragraph::new(help);
    frame.render_widget(para, inner);
}

pub fn parse_color(s: &str) -> Color {
    match s.to_lowercase().as_str() {
        "black"     => Color::Black,
        "red"       => Color::Red,
        "green"     => Color::Green,
        "yellow"    => Color::Yellow,
        "blue"      => Color::Blue,
        "magenta"   => Color::Magenta,
        "cyan"      => Color::Cyan,
        "white"     => Color::White,
        "dark_gray" | "darkgray" | "gray" => Color::DarkGray,
        "light_red"     => Color::LightRed,
        "light_green"   => Color::LightGreen,
        "light_yellow"  => Color::LightYellow,
        "light_blue"    => Color::LightBlue,
        "light_magenta" => Color::LightMagenta,
        "light_cyan"    => Color::LightCyan,
        _ => {
            // Try hex: #rrggbb
            if s.starts_with('#') && s.len() == 7 {
                if let (Ok(r), Ok(g), Ok(b)) = (
                    u8::from_str_radix(&s[1..3], 16),
                    u8::from_str_radix(&s[3..5], 16),
                    u8::from_str_radix(&s[5..7], 16),
                ) {
                    return Color::Rgb(r, g, b);
                }
            }
            Color::White
        }
    }
}

fn status_icon(status: &str) -> &'static str {
    match status {
        "Done"       => "✓",
        "Blocked"    => "✗",
        "Cancelled"  => "⊘",
        "In Progress" => "●",
        _            => "○",
    }
}

/// Deterministic color for a tag name based on a simple hash.
fn tag_to_color(tag: &str) -> Color {
    const PALETTE: &[Color] = &[
        Color::Cyan,
        Color::Magenta,
        Color::LightBlue,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightRed,
        Color::LightMagenta,
        Color::LightCyan,
    ];
    let hash: usize = tag.bytes().fold(0usize, |acc, b| acc.wrapping_mul(31).wrapping_add(b as usize));
    PALETTE[hash % PALETTE.len()]
}

fn render_tag_editor_popup(frame: &mut Frame, area: Rect, app: &mut AppState) {
    let popup_width = 50u16.min(area.width.saturating_sub(4));
    let popup_height = 14u16.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .title(" Edit Tags (Esc to confirm) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if let Some(PopupKind::EditTags { ref mut textarea, ref existing, ref selected }) = app.popup {
        let selected = *selected;
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),  // hint
                Constraint::Min(1),     // existing tags list
                Constraint::Length(1),  // separator
                Constraint::Length(1),  // input label
                Constraint::Length(1),  // input
            ])
            .split(inner);

        let hint = Paragraph::new(Line::from(vec![
            Span::styled("  \u{2191}\u{2193}", Style::default().fg(Color::White)),
            Span::styled(" navigate  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Ctrl+d", Style::default().fg(Color::White)),
            Span::styled(" remove  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Enter", Style::default().fg(Color::White)),
            Span::styled(" add  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc", Style::default().fg(Color::White)),
            Span::styled(" confirm", Style::default().fg(Color::DarkGray)),
        ]));
        frame.render_widget(hint, chunks[0]);

        // Existing tags
        let tag_items: Vec<ListItem> = existing
            .iter()
            .enumerate()
            .map(|(i, tag)| {
                let style = if i == selected {
                    Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(tag_to_color(tag))
                };
                ListItem::new(format!("  [{}]", tag)).style(style)
            })
            .collect();
        if tag_items.is_empty() {
            let empty = Paragraph::new("  (no tags)")
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(empty, chunks[1]);
        } else {
            let list = List::new(tag_items);
            frame.render_widget(list, chunks[1]);
        }

        let label = Paragraph::new("Add tag:")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(label, chunks[3]);

        frame.render_widget(&*textarea, chunks[4]);
    }
}

fn render_tab_bar(frame: &mut Frame, area: Rect, app: &AppState) {
    let mut spans: Vec<Span> = Vec::new();
    let in_view = app.active_view.is_some();
    for (i, tab) in app.tabs.iter().enumerate() {
        let is_active = i == app.active_tab_idx && !in_view;
        let tab_color = parse_color(&tab.color);
        if is_active {
            spans.push(Span::styled(
                format!(" {} ", tab.name),
                Style::default()
                    .fg(Color::Black)
                    .bg(tab_color)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                format!(" {} ", tab.name),
                Style::default().fg(Color::DarkGray),
            ));
        }
        if i + 1 < app.tabs.len() || !app.views.is_empty() {
            spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
        }
    }
    // Render views after tabs
    for (i, view) in app.views.iter().enumerate() {
        let is_active = app.active_view == Some(i);
        if is_active {
            spans.push(Span::styled(
                format!(" {} ", view.name),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                format!(" {} ", view.name),
                Style::default().fg(Color::Cyan),
            ));
        }
        if i + 1 < app.views.len() {
            spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
        }
    }
    let line = Line::from(spans);
    let para = Paragraph::new(line).style(Style::default().bg(Color::Reset));
    frame.render_widget(para, area);
}

fn render_create_tab_popup(frame: &mut Frame, area: Rect, app: &mut AppState) {
    let popup_width = 40u16.min(area.width.saturating_sub(4));
    let popup_height = 5u16;
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .title(" New Tab ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if let Some(PopupKind::CreateTabName { ref mut textarea }) = app.popup {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(inner);
        let label = Paragraph::new("Tab name:").style(Style::default().fg(Color::DarkGray));
        frame.render_widget(label, chunks[0]);
        frame.render_widget(&*textarea, chunks[1]);
    }
}

fn render_rename_tab_popup(frame: &mut Frame, area: Rect, app: &mut AppState) {
    let popup_width = 40u16.min(area.width.saturating_sub(4));
    let popup_height = 5u16;
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .title(" Rename Tab ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if let Some(PopupKind::RenameTab { ref mut textarea }) = app.popup {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(inner);
        let label = Paragraph::new("New name:").style(Style::default().fg(Color::DarkGray));
        frame.render_widget(label, chunks[0]);
        frame.render_widget(&*textarea, chunks[1]);
    }
}

fn render_tab_picker_popup(frame: &mut Frame, area: Rect, app: &AppState) {
    if let Some(PopupKind::TabPicker { ref options, selected }) = app.popup {
        let popup_height = (options.len() as u16 + 3).min(area.height.saturating_sub(4));
        let popup_width = 35u16.min(area.width.saturating_sub(4));
        let x = (area.width.saturating_sub(popup_width)) / 2;
        let y = (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        frame.render_widget(Clear, popup_area);
        let block = Block::default()
            .title(" Move to Tab ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        let items: Vec<ListItem> = options
            .iter()
            .enumerate()
            .map(|(i, (name, _))| {
                let style = if i == selected {
                    Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(format!("  {}", name)).style(style)
            })
            .collect();
        let list = List::new(items);
        frame.render_widget(list, inner);
    }
}

fn render_confirm_delete_tab_popup(frame: &mut Frame, area: Rect) {
    let popup_width = 40u16.min(area.width.saturating_sub(4));
    let popup_height = 4u16;
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .title(" Delete Tab? ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let msg = Paragraph::new("Delete this tab and all items? (y/n)")
        .style(Style::default().fg(Color::Red));
    frame.render_widget(msg, inner);
}

fn render_view_picker_popup(frame: &mut Frame, area: Rect, app: &AppState) {
    if let Some(PopupKind::ViewPicker { ref options, selected }) = app.popup {
        let popup_height = (options.len() as u16 + 3).min(area.height.saturating_sub(4));
        let popup_width = 35u16.min(area.width.saturating_sub(4));
        let x = (area.width.saturating_sub(popup_width)) / 2;
        let y = (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        frame.render_widget(Clear, popup_area);
        let block = Block::default()
            .title(" Views (d to delete) ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        let items: Vec<ListItem> = options
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let style = if i == selected {
                    Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)
                } else if name.starts_with('+') {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::Cyan)
                };
                ListItem::new(format!("  {}", name)).style(style)
            })
            .collect();
        let list = List::new(items);
        frame.render_widget(list, inner);
    }
}

fn render_create_view_popup(frame: &mut Frame, area: Rect, app: &mut AppState) {
    let popup_width = 40u16.min(area.width.saturating_sub(4));
    let popup_height = 5u16;
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .title(" New View ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if let Some(PopupKind::CreateView { ref mut textarea }) = app.popup {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(inner);
        let label = Paragraph::new("Tag to filter by:").style(Style::default().fg(Color::DarkGray));
        frame.render_widget(label, chunks[0]);
        frame.render_widget(&*textarea, chunks[1]);
    }
}
