//! Word Arena HTTP service and its validated offline runtime resources.

use std::sync::Arc;

use axum::{Json, Router, extract::State, routing::get};
use serde::Serialize;
use tokio::net::TcpListener;
use word_arena_application::LexiconResolver;
use word_arena_engine::{Language, Ruleset, WordValidator};
use word_arena_lexicon::{
    InstalledPackError, LoadedLexicon, PackIdentity, WordArenaPaths, load_installed_lexicon_exact,
};

mod mcp;
mod mcp_resources;
mod mcp_tools;
mod runtime;
mod transport;

pub use mcp::MCP_PROTOCOL_VERSION;

pub use runtime::{ProductionRuntimeError, build_production_state};
pub use transport::{
    API_SCHEMA_VERSION, ApiEnvelope, ApiErrorBody, BROWSER_WEBSOCKET_PROTOCOL, CreateGameRequest,
    CreateGameResponse, GAME_EVENTS_PATH, GameActionRequest, GameInvalidation, PUBLIC_GAME_PATH,
    SEAT_GAME_PATH, SPECTATOR_GAME_PATH, ServerState, api_app, application_app, serve_application,
};

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
    /// Returns [`InstalledPackError`] if either exact production identity is
    /// absent, malformed, or fails complete manifest/FST validation.
    pub fn load(paths: &WordArenaPaths) -> Result<Self, InstalledPackError> {
        Self::load_exact(
            paths,
            &Ruleset::english_v1().lexicon,
            &Ruleset::french_v1().lexicon,
        )
    }

    /// Loads two explicitly pinned identities, primarily for isolated tests.
    ///
    /// # Errors
    ///
    /// Returns [`InstalledPackError`] if either exact identity is absent or
    /// fails complete manifest/FST validation.
    pub fn load_exact(
        paths: &WordArenaPaths,
        english: &PackIdentity,
        french: &PackIdentity,
    ) -> Result<Self, InstalledPackError> {
        Ok(Self {
            english: Arc::new(load_installed_lexicon_exact(paths, english)?),
            french: Arc::new(load_installed_lexicon_exact(paths, french)?),
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

    /// Returns the immutable query boundary for a curated offline language.
    #[must_use]
    pub fn validator(&self, language: Language) -> Option<Arc<dyn WordValidator>> {
        let lexicon: Arc<LoadedLexicon> = match language {
            Language::English => Arc::clone(&self.english),
            Language::French => Arc::clone(&self.french),
            Language::German | Language::Spanish => return None,
        };
        Some(lexicon)
    }
}

impl LexiconResolver for RuntimeLexicons {
    fn resolve(&self, identity: &PackIdentity) -> Option<Arc<dyn WordValidator>> {
        for lexicon in [&self.english, &self.french] {
            if lexicon.identity() == identity {
                let validator: Arc<LoadedLexicon> = Arc::clone(lexicon);
                return Some(validator);
            }
        }
        None
    }

    fn manifest(&self, identity: &PackIdentity) -> Option<word_arena_lexicon::PackManifest> {
        [&self.english, &self.french]
            .into_iter()
            .find(|lexicon| lexicon.identity() == identity)
            .map(|lexicon| lexicon.manifest().clone())
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
    languages: [&'static str; 2],
    lexicons: [LexiconHealth; 2],
}

#[derive(Debug, Serialize)]
struct LexiconHealth {
    locale: String,
    pack_id: String,
    pack_version: String,
    content_sha256: String,
    word_count: u64,
    source_id: String,
    source_revision: String,
    license_id: String,
}

async fn health(State(lexicons): State<Arc<RuntimeLexicons>>) -> Json<HealthResponse> {
    let english = lexicons.english();
    let french = lexicons.french();
    Json(HealthResponse {
        status: "ok",
        service: "word-arena-server",
        version: env!("CARGO_PKG_VERSION"),
        languages: Language::OFFLINE_V1.map(Language::code),
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
        source_id: lexicon.manifest().source.id.clone(),
        source_revision: lexicon.manifest().source.revision.clone(),
        license_id: lexicon.manifest().source.license_id.clone(),
    }
}
