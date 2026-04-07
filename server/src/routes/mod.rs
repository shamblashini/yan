mod sync;
mod ws;

use std::sync::Arc;

use axum::{routing::get, Router};

use crate::ServerState;

pub fn router(state: Arc<ServerState>) -> Router<Arc<ServerState>> {
    Router::new()
        .route("/health", get(health))
        .merge(sync::router(state))
        .merge(ws::router())
}

async fn health() -> &'static str {
    "ok"
}
