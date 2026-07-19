use std::{env, error::Error, net::SocketAddr};

use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;
use word_arena_lexicon::WordArenaPaths;
use word_arena_server::{RuntimeLexicons, build_production_state, serve_application};

const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1:3000";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .compact()
        .init();

    let paths = WordArenaPaths::discover()?;
    let lexicons = std::sync::Arc::new(RuntimeLexicons::load(&paths)?);
    let state = build_production_state(&paths, std::sync::Arc::clone(&lexicons)).await?;
    let bind_address = env::var("WORD_ARENA_BIND")
        .unwrap_or_else(|_| DEFAULT_BIND_ADDRESS.to_owned())
        .parse::<SocketAddr>()?;
    let listener = TcpListener::bind(bind_address).await?;

    info!(address = %bind_address, "Word Arena server listening");
    serve_application(listener, lexicons, state).await?;

    Ok(())
}
