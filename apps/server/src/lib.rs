//! Word Arena HTTP service and its validated offline runtime resources.

use std::sync::Arc;

use axum::{Json, Router, extract::State, routing::get};
use serde::Serialize;
use tokio::net::TcpListener;
use word_arena_engine::Language;
use word_arena_lexicon::{
    InstalledPackError, LoadedLexicon, WordArenaPaths, load_installed_lexicon,
};

const ENGLISH_PACK_ID: &str = "word-arena-en-world-v1";
const FRENCH_PACK_ID: &str = "word-arena-fr-v1";

/// Fully verified immutable indexes retained for the server lifetime.
#[derive(Debug)]
pub struct RuntimeLexicons {
    english: Arc<LoadedLexicon>,
    french: Arc<LoadedLexicon>,
}

impl RuntimeLexicons {
    /// Loads both V1 packs from platform-local storage without network access.
    ///
    /// # Errors
    ///
    /// Returns [`InstalledPackError`] if either pack is absent, ambiguous,
    /// malformed, or fails complete manifest/FST validation.
    pub fn load(paths: &WordArenaPaths) -> Result<Self, InstalledPackError> {
        Ok(Self {
            english: Arc::new(load_installed_lexicon(paths, ENGLISH_PACK_ID)?),
            french: Arc::new(load_installed_lexicon(paths, FRENCH_PACK_ID)?),
        })
    }

    /// Loaded English index.
    #[must_use]
    pub fn english(&self) -> &Arc<LoadedLexicon> {
        &self.english
    }

    /// Loaded French index.
    #[must_use]
    pub fn french(&self) -> &Arc<LoadedLexicon> {
        &self.french
    }
}

/// Builds the service router around already validated offline lexicons.
pub fn app(lexicons: Arc<RuntimeLexicons>) -> Router {
    Router::new()
        .route("/health", get(health))
        .with_state(lexicons)
}

/// Serves the HTTP router using already validated offline lexicons.
///
/// # Errors
///
/// Returns an I/O error if the bound listener fails while serving.
pub async fn serve(listener: TcpListener, lexicons: Arc<RuntimeLexicons>) -> std::io::Result<()> {
    axum::serve(listener, app(lexicons)).await
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    service: &'static str,
    version: &'static str,
    languages: [&'static str; 4],
    lexicons: [LexiconHealth; 2],
}

#[derive(Debug, Serialize)]
struct LexiconHealth {
    locale: String,
    pack_id: String,
    pack_version: String,
    content_sha256: String,
    word_count: u64,
}

async fn health(State(lexicons): State<Arc<RuntimeLexicons>>) -> Json<HealthResponse> {
    let english = lexicons.english();
    let french = lexicons.french();
    Json(HealthResponse {
        status: "ok",
        service: "word-arena-server",
        version: env!("CARGO_PKG_VERSION"),
        languages: Language::ALL.map(Language::code),
        lexicons: [health_pack(english), health_pack(french)],
    })
}

fn health_pack(lexicon: &LoadedLexicon) -> LexiconHealth {
    let identity = lexicon.identity();
    LexiconHealth {
        locale: identity.locale.clone(),
        pack_id: identity.pack_id.clone(),
        pack_version: identity.pack_version.clone(),
        content_sha256: identity.content_sha256.clone(),
        word_count: lexicon.word_count(),
    }
}
