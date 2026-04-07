mod broadcast;
mod db;
mod routes;

use std::sync::Arc;

use axum::Router;
use sqlx::PgPool;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use broadcast::Hub;

pub struct ServerState {
    pub pool: PgPool,
    pub hub: Arc<Hub>,
    /// Bearer token for auth. Set via AUTH_TOKEN env var.
    pub auth_token: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "yan_server=debug,tower_http=debug".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set (e.g. postgres://user:pass@localhost/yan)");
    let auth_token = std::env::var("AUTH_TOKEN")
        .expect("AUTH_TOKEN must be set");

    let pool = PgPool::connect(&database_url)
        .await
        .expect("Failed to connect to PostgreSQL");

    db::run_migrations(&pool).await;

    let hub = Arc::new(Hub::new());
    let state = Arc::new(ServerState { pool, hub, auth_token });

    let app = Router::new()
        .merge(routes::router(Arc::clone(&state)))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".into());
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!("Listening on {addr}");
    axum::serve(listener, app).await.unwrap();
}
