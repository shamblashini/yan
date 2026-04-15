mod app;
mod config;
mod input;
mod storage;
mod sync_client;
mod time_tracker;
mod todo;
mod ui;

use std::io;
use std::time::Duration;

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::{mpsc, watch};

use app::AppState;
use sync_client::SyncStatus;

#[tokio::main]
async fn main() -> io::Result<()> {
    // Handle --config wizard before starting the TUI
    if std::env::args().any(|a| a == "--config") {
        config::run_wizard();
        return Ok(());
    }

    // Load config (generates device_id on first run)
    let cfg = config::load();

    // Open SQLite database, run migrations, migrate from legacy TOML if present
    let db = storage::open_db();
    let (tabs, tab_roots, statuses) = storage::load_state(&db);
    let initial_collapsed = storage::load_collapse_state(&db);
    let initial_seq = storage::next_client_seq(&db);
    let db_path = storage::db_path();

    // Sync channels
    let (local_op_tx, local_op_rx) = mpsc::channel(256);
    let (remote_op_tx, remote_op_rx) = mpsc::channel::<Vec<yan_shared::ops::Operation>>(64);
    let (status_tx, status_rx) = watch::channel(SyncStatus::Disabled);
    let (err_tx, err_rx) = mpsc::channel::<String>(16);

    // Spawn background sync task if configured
    if cfg.is_sync_configured() {
        let cfg_clone = cfg.clone();
        tokio::spawn(sync_client::run(
            cfg_clone,
            db_path,
            local_op_rx,
            remote_op_tx,
            status_tx,
            err_tx,
        ));
    }

    // Build AppState
    let mut app = AppState::new(
        tabs,
        tab_roots,
        statuses,
        db,
        cfg.device_id,
        initial_seq,
        initial_collapsed,
        Some(local_op_tx),
        Some(remote_op_rx),
        Some(status_rx),
        Some(err_rx),
    );

    // Setup terminal — must happen on the main thread (crossterm requirement)
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Event loop (blocking on the tokio thread pool, but that's fine for a TUI)
    let tick = Duration::from_millis(200);
    let result = tokio::task::block_in_place(|| run_app(&mut terminal, &mut app, tick));

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    // Save timer state and statuses on exit
    app.save_to_db();

    result
}

fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut AppState,
    tick: Duration,
) -> io::Result<()>
where
    io::Error: From<B::Error>,
{
    loop {
        terminal.draw(|f| ui::render(f, app))?;

        // Poll sync channels (non-blocking)
        app.poll_sync();

        if event::poll(tick)? {
            let ev = event::read()?;
            input::handle_event(app, ev);
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}
