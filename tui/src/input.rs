use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui_textarea::TextArea;

use crate::app::{AppState, PopupKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Search,
}

pub fn handle_event(app: &mut AppState, event: Event) {
    match event {
        Event::Key(key) => handle_key(app, key),
        _ => {}
    }
}

fn handle_key(app: &mut AppState, key: KeyEvent) {
    // Ctrl-c always quits
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.should_quit = true;
        return;
    }

    if app.popup.is_some() {
        handle_popup_key(app, key);
        return;
    }

    match app.mode {
        Mode::Normal => handle_normal_key(app, key),
        Mode::Insert => handle_insert_key(app, key),
        Mode::Search => handle_search_key(app, key),
    }
}

fn handle_normal_key(app: &mut AppState, key: KeyEvent) {
    // Handle two-key sequences
    if let Some(pending) = app.pending_key.take() {
        match (pending, key.code) {
            ('d', KeyCode::Char('d')) => {
                app.open_confirm_delete();
                return;
            }
            ('g', KeyCode::Char('g')) => {
                app.move_to_top();
                return;
            }
            _ => {
                // Unknown sequence, fall through to single-key handling with the new key
            }
        }
    }

    match key.code {
        KeyCode::Char('j') | KeyCode::Down  => app.move_cursor(1),
        KeyCode::Char('k') | KeyCode::Up    => app.move_cursor(-1),
        KeyCode::Char('h') | KeyCode::Left  => app.collapse_current(),
        KeyCode::Char('l') | KeyCode::Right => app.expand_current(),
        KeyCode::Char('J')                  => app.move_item_down(),
        KeyCode::Char('K')                  => app.move_item_up(),
        KeyCode::Char('H')                  => app.dedent_item(),
        KeyCode::Char('L')                  => app.indent_item(),
        KeyCode::Char('G')                  => app.move_to_bottom(),
        KeyCode::Char('g')                  => { app.pending_key = Some('g'); }
        KeyCode::Char('a')                  => app.add_sibling_below(),
        KeyCode::Char('A')                  => app.add_child(),
        KeyCode::Char('i') | KeyCode::Char('e') => app.open_edit_title(),
        KeyCode::Char('E')                  => app.open_edit_description(),
        KeyCode::Char('d')                  => { app.pending_key = Some('d'); }
        KeyCode::Char('>')                  => app.indent_item(),
        KeyCode::Char('<')                  => app.dedent_item(),
        KeyCode::Char(' ')                  => app.toggle_done(),
        KeyCode::Char('s')                  => app.open_status_picker(),
        KeyCode::Char('t')                  => app.toggle_timer(),
        KeyCode::Char('T')                  => app.stop_all_timers(),
        KeyCode::Enter                      => app.toggle_collapse(),
        KeyCode::Char('/')                  => { app.mode = Mode::Search; app.search_query = Some(String::new()); }
        KeyCode::Char('n')                  => app.next_match(),
        KeyCode::Char('N')                  => app.prev_match(),
        KeyCode::Char('p')                  => app.toggle_detail_panel(),
        KeyCode::Char('#')                  => app.open_tag_editor(),
        KeyCode::Tab                        => app.next_tab(),
        KeyCode::BackTab                    => app.prev_tab(),
        KeyCode::Char('c')                  => app.open_create_tab(),
        KeyCode::Char('r')                  => app.open_rename_tab(),
        KeyCode::Char('m')                  => app.open_move_to_tab(),
        KeyCode::Char('X')                  => app.open_confirm_delete_tab(),
        KeyCode::Char('v')                  => app.open_view_picker(),
        KeyCode::Char('?')                  => { app.popup = Some(PopupKind::Help); }
        KeyCode::Char('q')                  => { app.save_and_quit(); }
        KeyCode::Esc                        => {
            if app.active_view.is_some() {
                app.deactivate_view();
            } else {
                app.pending_key = None;
            }
        }
        _ => {}
    }
}

fn handle_insert_key(app: &mut AppState, key: KeyEvent) {
    // Insert mode is only for popup textarea — shouldn't reach here normally
    app.mode = Mode::Normal;
    let _ = key;
}

fn handle_search_key(app: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.search_query = None;
            app.mode = Mode::Normal;
            app.rebuild_visible();
        }
        KeyCode::Enter => {
            app.mode = Mode::Normal;
            // Keep search active, just lock it
        }
        KeyCode::Backspace => {
            if let Some(ref mut q) = app.search_query {
                q.pop();
                app.rebuild_visible();
            }
        }
        KeyCode::Char(c) => {
            if let Some(ref mut q) = app.search_query {
                q.push(c);
                app.rebuild_visible();
            }
        }
        _ => {}
    }
}

fn handle_popup_key(app: &mut AppState, key: KeyEvent) {
    match &app.popup {
        Some(PopupKind::EditTitle { .. }) => handle_edit_title_key(app, key),
        Some(PopupKind::EditDescription { .. }) => handle_edit_desc_key(app, key),
        Some(PopupKind::SetStatus { .. }) => handle_status_picker_key(app, key),
        Some(PopupKind::AddStatus { .. }) => handle_add_status_key(app, key),
        Some(PopupKind::ConfirmDelete) => handle_confirm_delete_key(app, key),
        Some(PopupKind::EditTags { .. }) => handle_tag_editor_key(app, key),
        Some(PopupKind::CreateTabName { .. }) => handle_create_tab_key(app, key),
        Some(PopupKind::RenameTab { .. }) => handle_rename_tab_key(app, key),
        Some(PopupKind::TabPicker { .. }) => handle_tab_picker_key(app, key),
        Some(PopupKind::ConfirmDeleteTab) => handle_confirm_delete_tab_key(app, key),
        Some(PopupKind::ViewPicker { .. }) => handle_view_picker_key(app, key),
        Some(PopupKind::CreateView { .. }) => handle_create_view_key(app, key),
        Some(PopupKind::Help) => {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?')) {
                app.popup = None;
            }
        }
        None => {}
    }
}

fn handle_edit_title_key(app: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => {
            let text = if let Some(PopupKind::EditTitle { ref textarea }) = app.popup {
                textarea.lines().join(" ").trim().to_string()
            } else {
                String::new()
            };
            app.popup = None;
            app.mode = Mode::Normal;
            if !text.is_empty() {
                app.apply_edit_title(text);
            }
        }
        KeyCode::Esc => {
            app.popup = None;
            app.mode = Mode::Normal;
        }
        _ => {
            if let Some(PopupKind::EditTitle { ref mut textarea }) = app.popup {
                textarea.input(key);
            }
        }
    }
}

fn handle_edit_desc_key(app: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            let text = if let Some(PopupKind::EditDescription { ref textarea }) = app.popup {
                let lines = textarea.lines().join("\n");
                let trimmed = lines.trim().to_string();
                trimmed
            } else {
                String::new()
            };
            app.popup = None;
            app.mode = Mode::Normal;
            app.apply_edit_description(if text.is_empty() { None } else { Some(text) });
        }
        _ => {
            if let Some(PopupKind::EditDescription { ref mut textarea }) = app.popup {
                textarea.input(key);
            }
        }
    }
}

fn handle_status_picker_key(app: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if let Some(PopupKind::SetStatus { ref options, ref mut selected }) = app.popup {
                if *selected + 1 < options.len() {
                    *selected += 1;
                }
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if let Some(PopupKind::SetStatus { ref mut selected, .. }) = app.popup {
                if *selected > 0 {
                    *selected -= 1;
                }
            }
        }
        KeyCode::Enter => {
            let chosen = if let Some(PopupKind::SetStatus { ref options, selected }) = app.popup {
                options.get(selected).cloned()
            } else {
                None
            };
            let add_new = if let Some(PopupKind::SetStatus { ref options, selected }) = app.popup {
                options.get(selected).map(|s| s == "+ Add new status").unwrap_or(false)
            } else {
                false
            };
            app.popup = None;
            if add_new {
                app.open_add_status();
            } else if let Some(status) = chosen {
                app.apply_set_status(status);
            }
        }
        KeyCode::Char('a') => {
            app.popup = None;
            app.open_add_status();
        }
        KeyCode::Char('d') => {
            // Get the selected status name (guard: not the "+ Add" entry)
            let to_remove = if let Some(PopupKind::SetStatus { ref options, selected }) = app.popup {
                let name = options.get(selected).cloned().unwrap_or_default();
                if name == "+ Add new status" { None } else { Some(name) }
            } else {
                None
            };
            if let Some(name) = to_remove {
                let removed = app.remove_status(&name);
                if removed {
                    // Rebuild the options list in-place with the entry gone
                    app.open_status_picker();
                }
            }
        }
        KeyCode::Esc => {
            app.popup = None;
        }
        _ => {}
    }
}

fn handle_add_status_key(app: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => {
            let (name, color) = if let Some(PopupKind::AddStatus { ref textarea, ref color_buf }) = app.popup {
                (textarea.lines().join("").trim().to_string(), color_buf.clone())
            } else {
                (String::new(), String::new())
            };
            app.popup = None;
            if !name.is_empty() {
                let color = if color.is_empty() { "white".to_string() } else { color };
                app.add_custom_status(name, color);
            }
        }
        KeyCode::Esc => {
            app.popup = None;
        }
        _ => {
            if let Some(PopupKind::AddStatus { ref mut textarea, .. }) = app.popup {
                textarea.input(key);
            }
        }
    }
}

fn handle_confirm_delete_key(app: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Char('y') | KeyCode::Enter => {
            app.popup = None;
            app.delete_current();
        }
        KeyCode::Char('n') | KeyCode::Esc => {
            app.popup = None;
        }
        _ => {}
    }
}

fn handle_create_tab_key(app: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => {
            let name = if let Some(PopupKind::CreateTabName { ref textarea }) = app.popup {
                textarea.lines().join("").trim().to_string()
            } else {
                String::new()
            };
            app.popup = None;
            app.mode = Mode::Normal;
            app.apply_create_tab(name);
        }
        KeyCode::Esc => {
            app.popup = None;
            app.mode = Mode::Normal;
        }
        _ => {
            if let Some(PopupKind::CreateTabName { ref mut textarea }) = app.popup {
                textarea.input(key);
            }
        }
    }
}

fn handle_rename_tab_key(app: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => {
            let name = if let Some(PopupKind::RenameTab { ref textarea }) = app.popup {
                textarea.lines().join("").trim().to_string()
            } else {
                String::new()
            };
            app.popup = None;
            app.mode = Mode::Normal;
            app.apply_rename_tab(name);
        }
        KeyCode::Esc => {
            app.popup = None;
            app.mode = Mode::Normal;
        }
        _ => {
            if let Some(PopupKind::RenameTab { ref mut textarea }) = app.popup {
                textarea.input(key);
            }
        }
    }
}

fn handle_tab_picker_key(app: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if let Some(PopupKind::TabPicker { ref options, ref mut selected }) = app.popup {
                if *selected + 1 < options.len() {
                    *selected += 1;
                }
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if let Some(PopupKind::TabPicker { ref mut selected, .. }) = app.popup {
                if *selected > 0 {
                    *selected -= 1;
                }
            }
        }
        KeyCode::Enter => {
            let target = if let Some(PopupKind::TabPicker { ref options, selected }) = app.popup {
                options.get(selected).map(|(_, idx)| *idx)
            } else {
                None
            };
            app.popup = None;
            if let Some(idx) = target {
                app.move_item_to_tab(idx);
            }
        }
        KeyCode::Esc => {
            app.popup = None;
        }
        _ => {}
    }
}

fn handle_confirm_delete_tab_key(app: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Char('y') | KeyCode::Enter => {
            app.popup = None;
            app.delete_current_tab();
        }
        KeyCode::Char('n') | KeyCode::Esc => {
            app.popup = None;
        }
        _ => {}
    }
}

fn handle_view_picker_key(app: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if let Some(PopupKind::ViewPicker { ref options, ref mut selected }) = app.popup {
                if *selected + 1 < options.len() {
                    *selected += 1;
                }
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if let Some(PopupKind::ViewPicker { ref mut selected, .. }) = app.popup {
                if *selected > 0 {
                    *selected -= 1;
                }
            }
        }
        KeyCode::Enter => {
            let (is_new, idx) = if let Some(PopupKind::ViewPicker { ref options, selected }) = app.popup {
                if selected == options.len() - 1 {
                    (true, 0)
                } else {
                    (false, selected)
                }
            } else {
                (false, 0)
            };
            app.popup = None;
            if is_new {
                app.open_create_view();
            } else {
                app.activate_view(idx);
            }
        }
        KeyCode::Char('d') => {
            // Delete the selected view
            let idx = if let Some(PopupKind::ViewPicker { ref options, selected }) = app.popup {
                if selected < options.len() - 1 { Some(selected) } else { None }
            } else {
                None
            };
            if let Some(idx) = idx {
                app.popup = None;
                app.delete_view(idx);
            }
        }
        KeyCode::Esc => {
            app.popup = None;
        }
        _ => {}
    }
}

fn handle_create_view_key(app: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => {
            let tag = if let Some(PopupKind::CreateView { ref textarea }) = app.popup {
                textarea.lines().join("").trim().to_string()
            } else {
                String::new()
            };
            app.popup = None;
            app.mode = Mode::Normal;
            app.apply_create_view(tag);
        }
        KeyCode::Esc => {
            app.popup = None;
            app.mode = Mode::Normal;
        }
        _ => {
            if let Some(PopupKind::CreateView { ref mut textarea }) = app.popup {
                textarea.input(key);
            }
        }
    }
}

fn handle_tag_editor_key(app: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => {
            // Add the typed tag to existing list
            let tag = if let Some(PopupKind::EditTags { ref textarea, .. }) = app.popup {
                textarea.lines().join("").trim().to_string()
            } else {
                String::new()
            };
            if !tag.is_empty() {
                if let Some(PopupKind::EditTags { ref mut textarea, ref mut existing, .. }) = app.popup {
                    if !existing.contains(&tag) {
                        existing.push(tag);
                    }
                    *textarea = crate::input::new_textarea("");
                }
            }
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Remove selected tag
            if let Some(PopupKind::EditTags { ref mut existing, ref mut selected, .. }) = app.popup {
                if !existing.is_empty() {
                    existing.remove(*selected);
                    if *selected >= existing.len() && *selected > 0 {
                        *selected -= 1;
                    }
                }
            }
        }
        KeyCode::Up => {
            if let Some(PopupKind::EditTags { ref mut selected, .. }) = app.popup {
                if *selected > 0 {
                    *selected -= 1;
                }
            }
        }
        KeyCode::Down => {
            if let Some(PopupKind::EditTags { ref existing, ref mut selected, .. }) = app.popup {
                if *selected + 1 < existing.len() {
                    *selected += 1;
                }
            }
        }
        KeyCode::Esc => {
            // Confirm: apply tags and close
            let tags = if let Some(PopupKind::EditTags { ref existing, .. }) = app.popup {
                existing.clone()
            } else {
                Vec::new()
            };
            app.popup = None;
            app.mode = Mode::Normal;
            app.apply_tags(tags);
        }
        _ => {
            if let Some(PopupKind::EditTags { ref mut textarea, .. }) = app.popup {
                textarea.input(key);
            }
        }
    }
}

pub fn new_textarea(initial: &str) -> TextArea<'static> {
    let mut ta = TextArea::default();
    if !initial.is_empty() {
        ta.insert_str(initial);
    }
    ta
}
