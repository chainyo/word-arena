use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::Arc,
};

use axum::{
    Json,
    body::Body,
    http::{Method, Request, Response, StatusCode, header::CONTENT_TYPE},
    response::IntoResponse,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use word_arena_application::{
    ApplicationRuntime, AuthenticatedCredential, CapabilityScope, GameId,
};

use crate::{
    API_SCHEMA_VERSION,
    mcp_resources::McpResourceSubscriptions,
    mcp_tools::{McpRequestAuthority, WordArenaMcp},
};

/// Stable MCP protocol release implemented by the server.
pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
const SESSION_HEADER: &str = "mcp-session-id";
const MAX_MCP_SESSIONS: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
struct SessionBinding {
    game_id: GameId,
    seat: word_arena_engine::Seat,
    token_digest: [u8; 32],
}

/// Authenticated stateful Streamable HTTP gateway shared by the Axum router.
#[derive(Clone)]
pub struct McpGateway {
    service: StreamableHttpService<WordArenaMcp, LocalSessionManager>,
    sessions: Arc<LocalSessionManager>,
    bindings: Arc<RwLock<HashMap<String, SessionBinding>>>,
    subscriptions: McpResourceSubscriptions,
    cancellation: CancellationToken,
}

impl fmt::Debug for McpGateway {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpGateway")
            .field("sessions", &"[ISOLATED]")
            .field("bindings", &"[DIGEST-ONLY]")
            .finish_non_exhaustive()
    }
}

impl McpGateway {
    /// Builds one stateful local MCP service with bounded idle sessions.
    #[must_use]
    pub fn new(runtime: &Arc<ApplicationRuntime>) -> Self {
        let cancellation = CancellationToken::new();
        let sessions = Arc::new(LocalSessionManager::default());
        let subscriptions = McpResourceSubscriptions::default();
        let config = StreamableHttpServerConfig::default()
            .with_allowed_origins(["http://127.0.0.1:5173", "http://localhost:5173"])
            .with_cancellation_token(cancellation.child_token());
        let service_runtime = Arc::clone(runtime);
        let service_subscriptions = subscriptions.clone();
        let service = StreamableHttpService::new(
            move || {
                Ok(WordArenaMcp::new(
                    Arc::clone(&service_runtime),
                    service_subscriptions.clone(),
                ))
            },
            Arc::clone(&sessions),
            config,
        );
        Self {
            service,
            sessions,
            bindings: Arc::new(RwLock::new(HashMap::new())),
            subscriptions,
            cancellation,
        }
    }

    /// Cancels active MCP sessions during process shutdown.
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    /// Notifies every subscribed MCP session bound to one changed game.
    pub async fn notify_game_updated(&self, game_id: &GameId) {
        self.subscriptions.notify_game_updated(game_id).await;
    }

    /// Authenticates and dispatches one game-scoped Streamable HTTP request.
    pub async fn handle(
        &self,
        runtime: &ApplicationRuntime,
        game_id: GameId,
        bearer: &str,
        mut request: Request<Body>,
    ) -> Response<Body> {
        let authenticated = runtime
            .authenticate_capability(bearer, &game_id, CapabilityScope::Act)
            .await;
        let Ok(AuthenticatedCredential::Seat(credential)) = authenticated else {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "authentication failed",
            );
        };
        let binding = SessionBinding {
            game_id,
            seat: credential.seat(),
            token_digest: Sha256::digest(bearer.as_bytes()).into(),
        };
        request
            .extensions_mut()
            .insert(McpRequestAuthority::new(credential));
        self.remove_closed_bindings().await;

        let method = request.method().clone();
        let requested_session = request
            .headers()
            .get(SESSION_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        if let Some(session_id) = requested_session.as_deref() {
            if self.bindings.read().await.get(session_id) != Some(&binding) {
                return error_response(
                    StatusCode::UNAUTHORIZED,
                    "session_authority_mismatch",
                    "MCP session is not valid for this capability",
                );
            }
        } else if self.bindings.read().await.len() >= MAX_MCP_SESSIONS {
            return error_response(
                StatusCode::TOO_MANY_REQUESTS,
                "session_limit",
                "too many MCP sessions are open",
            );
        }

        let response = self.service.clone().oneshot(request).await;
        let Ok(response) = response;
        let (parts, body) = response.into_parts();
        let response = Response::from_parts(parts, Body::new(body));

        if method == Method::DELETE {
            if let Some(session_id) = requested_session {
                self.bindings.write().await.remove(&session_id);
                self.subscriptions.remove_session(&session_id).await;
            }
        } else if response.status().is_success()
            && let Some(session_id) = response
                .headers()
                .get(SESSION_HEADER)
                .and_then(|value| value.to_str().ok())
        {
            self.bindings
                .write()
                .await
                .insert(session_id.to_owned(), binding);
        }
        response
    }

    async fn remove_closed_bindings(&self) {
        let live: HashSet<String> = self
            .sessions
            .sessions
            .read()
            .await
            .keys()
            .map(ToString::to_string)
            .collect();
        self.bindings
            .write()
            .await
            .retain(|session_id, _| live.contains(session_id));
        self.subscriptions.retain_sessions(&live).await;
    }
}

fn error_response(status: StatusCode, code: &str, message: &str) -> Response<Body> {
    let mut response = (
        status,
        Json(json!({
            "schema_version": API_SCHEMA_VERSION,
            "error": { "code": code, "message": message }
        })),
    )
        .into_response();
    response.headers_mut().insert(
        CONTENT_TYPE,
        "application/json".parse().expect("static MIME is valid"),
    );
    response
}
