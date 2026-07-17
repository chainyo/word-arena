use std::{env, error::Error, net::SocketAddr};

use axum::{Json, Router, routing::get};
use serde::Serialize;
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;
use word_arena_engine::Language;

const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1:3000";

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    service: &'static str,
    version: &'static str,
    languages: [&'static str; 4],
}

fn app() -> Router {
    Router::new().route("/health", get(health))
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        service: "word-arena-server",
        version: env!("CARGO_PKG_VERSION"),
        languages: Language::ALL.map(Language::code),
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .compact()
        .init();

    let bind_address = env::var("WORD_ARENA_BIND")
        .unwrap_or_else(|_| DEFAULT_BIND_ADDRESS.to_owned())
        .parse::<SocketAddr>()?;
    let listener = TcpListener::bind(bind_address).await?;

    info!(address = %bind_address, "Word Arena server listening");
    axum::serve(listener, app()).await?;

    Ok(())
}
