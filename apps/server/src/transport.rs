use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    extract::{
        Path, Query, Request, State, WebSocketUpgrade,
        rejection::{JsonRejection, QueryRejection},
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use serde::{Deserialize, Serialize};
use tokio::sync::{Semaphore, broadcast};
use tower::limit::ConcurrencyLimitLayer;
use tower_http::{
    cors::CorsLayer, limit::RequestBodyLimitLayer, timeout::TimeoutLayer, trace::TraceLayer,
};
use word_arena_application::{
    AdministratorGameQuery, ApplicationError, ApplicationRuntime, AuthenticatedCredential,
    CapabilityError, CapabilityRole, CapabilityScope, GameActionCommand, GameId,
    HumanSpectatorGameQuery, HumanSpectatorReplayQuery, IdempotencyKey, IssueCapabilityRequest,
    PublicGameQuery, RepositoryError, SeatGameQuery,
};
use word_arena_engine::{GameMode, Language, Move, Ruleset, RulesetId, Turn};

use crate::{RuntimeLexicons, mcp::McpGateway};

/// Stable V1 REST and WebSocket schema version.
pub const API_SCHEMA_VERSION: u16 = 1;
/// Browser-safe WebSocket subprotocol used alongside an opaque capability.
pub const BROWSER_WEBSOCKET_PROTOCOL: &str = "word-arena-v1";
/// V1 public projection route contract.
pub const PUBLIC_GAME_PATH: &str = "/api/v1/games/{game_id}/public";
/// V1 seat-private projection route contract.
pub const SEAT_GAME_PATH: &str = "/api/v1/games/{game_id}/seat";
/// V1 trusted-human spectator projection route contract.
pub const SPECTATOR_GAME_PATH: &str = "/api/v1/games/{game_id}/spectator";
/// V1 trusted-human finished replay route contract.
pub const SPECTATOR_REPLAY_PATH: &str = "/api/v1/games/{game_id}/spectator/replay";
/// V1 public-only invalidation stream route contract.
pub const GAME_EVENTS_PATH: &str = "/api/v1/games/{game_id}/events";
const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_IN_FLIGHT_REQUESTS: usize = 128;
const MAX_WEBSOCKETS: usize = 64;
const WEBSOCKET_BUFFER: usize = 64;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const PUBLIC_CAPABILITY_LIFETIME_MS: i64 = 24 * 60 * 60 * 1_000;
const SPECTATOR_CAPABILITY_LIFETIME_MS: i64 = 24 * 60 * 60 * 1_000;
const DEADLINE_POLL_INTERVAL: Duration = Duration::from_millis(250);
const DEADLINE_BATCH_SIZE: u32 = 64;

/// Shared application transport state.
#[derive(Debug)]
pub struct ServerState {
    runtime: Arc<ApplicationRuntime>,
    notifications: NotificationHub,
    websocket_slots: Arc<Semaphore>,
    mcp: McpGateway,
}

impl ServerState {
    /// Creates isolated server state around one authoritative application.
    #[must_use]
    pub fn new(runtime: Arc<ApplicationRuntime>) -> Self {
        Self {
            mcp: McpGateway::new(&runtime),
            runtime,
            notifications: NotificationHub::default(),
            websocket_slots: Arc::new(Semaphore::new(MAX_WEBSOCKETS)),
        }
    }

    /// Underlying application runtime for trusted process orchestration.
    #[must_use]
    pub fn runtime(&self) -> &Arc<ApplicationRuntime> {
        &self.runtime
    }

    /// Cancels every active MCP session during trusted process shutdown.
    pub fn cancel_mcp(&self) {
        self.mcp.cancel();
    }
}

/// Public-only WebSocket invalidation marker.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GameInvalidation {
    /// Transport schema version.
    pub schema_version: u16,
    /// Changed game.
    pub game_id: GameId,
    /// New authoritative game version.
    pub version: u64,
}

#[derive(Debug, Default)]
struct NotificationHub {
    senders: Mutex<HashMap<GameId, broadcast::Sender<GameInvalidation>>>,
}

impl NotificationHub {
    fn subscribe(&self, game_id: &GameId) -> broadcast::Receiver<GameInvalidation> {
        let mut senders = self
            .senders
            .lock()
            .expect("notification mutex is not poisoned");
        senders
            .entry(game_id.clone())
            .or_insert_with(|| broadcast::channel(WEBSOCKET_BUFFER).0)
            .subscribe()
    }

    fn publish(&self, invalidation: GameInvalidation) {
        let sender = self
            .senders
            .lock()
            .expect("notification mutex is not poisoned")
            .entry(invalidation.game_id.clone())
            .or_insert_with(|| broadcast::channel(WEBSOCKET_BUFFER).0)
            .clone();
        let _ = sender.send(invalidation);
    }
}

/// Strict versioned response envelope.
#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApiEnvelope<T> {
    /// Transport schema version.
    pub schema_version: u16,
    /// Typed payload.
    pub data: T,
}

/// Local operator game-creation request.
#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateGameRequest {
    /// Immutable language/ruleset selection.
    pub language: Language,
    /// Immutable competitive or practice behavior; competitive by default.
    #[serde(default)]
    pub mode: GameMode,
    /// Mandatory retry identity for durable creation deduplication.
    pub idempotency_key: IdempotencyKey,
}

/// Newly created game plus one-time local-operator observer capabilities.
#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateGameResponse {
    /// Stable game ID.
    pub game_id: GameId,
    /// Initial public projection.
    pub public: word_arena_engine::PublicProjection,
    /// One-time raw public observer capability.
    pub public_capability: String,
    /// One-time raw trusted-human spectator capability.
    pub spectator_capability: String,
}

/// Seat action body; role and seat are deliberately absent.
#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GameActionRequest {
    /// Expected authoritative game version.
    pub expected_version: u64,
    /// Explicit turn number, equal to the expected version.
    pub turn_number: u64,
    /// Mandatory retry identity.
    pub idempotency_key: IdempotencyKey,
    /// Typed engine action.
    pub action: Move,
}

/// Stable error payload with no private diagnostic bytes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApiErrorBody {
    /// Transport schema version.
    pub schema_version: u16,
    /// Stable machine-readable category.
    pub code: String,
    /// Concise safe summary.
    pub message: String,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    body: ApiErrorBody,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            body: ApiErrorBody {
                schema_version: API_SCHEMA_VERSION,
                code: code.to_owned(),
                message: message.to_owned(),
            },
        }
    }

    fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "a valid scoped capability is required",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

/// Builds the complete local health, REST, and WebSocket router.
pub fn application_app(lexicons: Arc<RuntimeLexicons>, state: Arc<ServerState>) -> Router {
    crate::app(lexicons).merge(api_app(state))
}

/// Builds the bounded V1 game API router without the lexicon health route.
///
/// This split keeps transport integration tests independent from installed
/// full-corpus lexicon artifacts.
pub fn api_app(state: Arc<ServerState>) -> Router {
    Router::new()
        .route("/api/v1/games", post(create_game))
        .route(PUBLIC_GAME_PATH, get(public_game))
        .route(SEAT_GAME_PATH, get(seat_game))
        .route(SPECTATOR_GAME_PATH, get(spectator_game))
        .route(SPECTATOR_REPLAY_PATH, get(spectator_replay))
        .route(
            "/api/v1/games/{game_id}/administrator",
            get(administrator_game),
        )
        .route("/api/v1/games/{game_id}/rules", get(game_rules))
        .route("/api/v1/games/{game_id}/actions", post(game_action))
        .route(GAME_EVENTS_PATH, get(game_events))
        .route("/api/v1/games/{game_id}/mcp", any(mcp_endpoint))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(ConcurrencyLimitLayer::new(MAX_IN_FLIGHT_REQUESTS))
        .layer(RequestBodyLimitLayer::new(MAX_REQUEST_BYTES))
        .layer(cors_layer())
}

/// Serves the complete router with process-signal graceful shutdown.
///
/// # Errors
///
/// Returns an I/O error if serving the bound listener fails.
pub async fn serve_application(
    listener: tokio::net::TcpListener,
    lexicons: Arc<RuntimeLexicons>,
    state: Arc<ServerState>,
) -> std::io::Result<()> {
    let mcp = state.mcp.clone();
    let worker_state = Arc::clone(&state);
    let deadline_worker = tokio::spawn(async move {
        let mut interval = tokio::time::interval(DEADLINE_POLL_INTERVAL);
        loop {
            interval.tick().await;
            match worker_state
                .runtime
                .service()
                .resolve_due_timeouts(DEADLINE_BATCH_SIZE)
                .await
            {
                Ok(commands) => {
                    for command in commands {
                        worker_state.mcp.notify_game_updated(&command.game_id).await;
                        worker_state.notifications.publish(GameInvalidation {
                            schema_version: API_SCHEMA_VERSION,
                            game_id: command.game_id,
                            version: command.expected_version.saturating_add(1),
                        });
                    }
                }
                Err(error) => tracing::warn!(%error, "deadline worker iteration failed"),
            }
        }
    });
    let result = axum::serve(listener, application_app(lexicons, state))
        .with_graceful_shutdown(shutdown_signal())
        .await;
    deadline_worker.abort();
    let _ = deadline_worker.await;
    mcp.cancel();
    result
}

async fn mcp_endpoint(
    State(state): State<Arc<ServerState>>,
    Path(game_id): Path<String>,
    request: Request,
) -> Response {
    let game_id = match parse_game_id(game_id) {
        Ok(game_id) => game_id,
        Err(error) => return error.into_response(),
    };
    let bearer = match bearer_token(request.headers()) {
        Ok(token) => token.to_owned(),
        Err(error) => return error.into_response(),
    };
    state
        .mcp
        .handle(&state.runtime, game_id, &bearer, request)
        .await
}

async fn create_game(
    State(state): State<Arc<ServerState>>,
    payload: Result<Json<CreateGameRequest>, JsonRejection>,
) -> Result<Json<ApiEnvelope<CreateGameResponse>>, ApiError> {
    let Json(request) = payload.map_err(|error| json_rejection(&error))?;
    let command = state.runtime.service().prepare_create_game_with_mode(
        request.language,
        request.mode,
        request.idempotency_key,
    );
    let created = state
        .runtime
        .service()
        .create_game(command)
        .await
        .map_err(|error| map_application_error(&error))?;
    let expires_at = created
        .created_at
        .0
        .checked_add(PUBLIC_CAPABILITY_LIFETIME_MS)
        .map(word_arena_application::UnixMillis)
        .ok_or_else(|| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "time", "time overflow"))?;
    let public_capability = state
        .runtime
        .issue_capability(IssueCapabilityRequest {
            game_id: created.game_id.clone(),
            role: CapabilityRole::Public,
            scopes: [CapabilityScope::ObservePublic].into_iter().collect(),
            expires_at,
            agent_run_id: None,
        })
        .await
        .map_err(|error| map_capability_error(&error))?;
    let spectator_expires_at = created
        .created_at
        .0
        .checked_add(SPECTATOR_CAPABILITY_LIFETIME_MS)
        .map(word_arena_application::UnixMillis)
        .ok_or_else(|| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "time", "time overflow"))?;
    let spectator_capability = state
        .runtime
        .issue_capability(IssueCapabilityRequest {
            game_id: created.game_id.clone(),
            role: CapabilityRole::HumanSpectator,
            scopes: [
                CapabilityScope::ObservePublic,
                CapabilityScope::ObserveHumanSpectator,
            ]
            .into_iter()
            .collect(),
            expires_at: spectator_expires_at,
            agent_run_id: None,
        })
        .await
        .map_err(|error| map_capability_error(&error))?;
    Ok(Json(ApiEnvelope {
        schema_version: API_SCHEMA_VERSION,
        data: CreateGameResponse {
            game_id: created.game_id,
            public: created.public,
            public_capability: public_capability.token.into_secret(),
            spectator_capability: spectator_capability.token.into_secret(),
        },
    }))
}

async fn public_game(
    State(state): State<Arc<ServerState>>,
    Path(game_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ApiEnvelope<word_arena_application::PublicGameView>>, ApiError> {
    let game_id = parse_game_id(game_id)?;
    let authenticated =
        authenticate(&state, &headers, &game_id, CapabilityScope::ObservePublic).await?;
    let view = state
        .runtime
        .service()
        .public_game(
            &authenticated.public_viewer(),
            PublicGameQuery {
                game_id: game_id.clone(),
            },
        )
        .await
        .map_err(|error| map_application_error(&error))?;
    Ok(envelope(view))
}

async fn seat_game(
    State(state): State<Arc<ServerState>>,
    Path(game_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ApiEnvelope<word_arena_application::SeatGameView>>, ApiError> {
    let game_id = parse_game_id(game_id)?;
    let authenticated =
        authenticate(&state, &headers, &game_id, CapabilityScope::ObserveSeat).await?;
    let AuthenticatedCredential::Seat(credential) = authenticated else {
        return Err(ApiError::unauthorized());
    };
    let view = state
        .runtime
        .service()
        .seat_game(&credential, SeatGameQuery { game_id })
        .await
        .map_err(|error| map_application_error(&error))?;
    Ok(envelope(view))
}

async fn spectator_game(
    State(state): State<Arc<ServerState>>,
    Path(game_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ApiEnvelope<word_arena_application::HumanSpectatorGameView>>, ApiError> {
    let game_id = parse_game_id(game_id)?;
    let authenticated = authenticate(
        &state,
        &headers,
        &game_id,
        CapabilityScope::ObserveHumanSpectator,
    )
    .await?;
    let AuthenticatedCredential::HumanSpectator(credential) = authenticated else {
        return Err(ApiError::unauthorized());
    };
    let view = state
        .runtime
        .service()
        .human_spectator_game(&credential, HumanSpectatorGameQuery { game_id })
        .await
        .map_err(|error| map_application_error(&error))?;
    Ok(envelope(view))
}

async fn spectator_replay(
    State(state): State<Arc<ServerState>>,
    Path(game_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ApiEnvelope<word_arena_application::HumanSpectatorReplayView>>, ApiError> {
    let game_id = parse_game_id(game_id)?;
    let authenticated = authenticate(
        &state,
        &headers,
        &game_id,
        CapabilityScope::ObserveHumanSpectator,
    )
    .await?;
    let AuthenticatedCredential::HumanSpectator(credential) = authenticated else {
        return Err(ApiError::unauthorized());
    };
    let view = state
        .runtime
        .service()
        .human_spectator_replay(&credential, HumanSpectatorReplayQuery { game_id })
        .await
        .map_err(|error| map_application_error(&error))?;
    Ok(envelope(view))
}

async fn administrator_game(
    State(state): State<Arc<ServerState>>,
    Path(game_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ApiEnvelope<word_arena_application::AdministratorGameView>>, ApiError> {
    let game_id = parse_game_id(game_id)?;
    let authenticated = authenticate(
        &state,
        &headers,
        &game_id,
        CapabilityScope::ObserveAdministrator,
    )
    .await?;
    let AuthenticatedCredential::Administrator(credential) = authenticated else {
        return Err(ApiError::unauthorized());
    };
    let view = state
        .runtime
        .service()
        .administrator_game(&credential, AdministratorGameQuery { game_id })
        .await
        .map_err(|error| map_application_error(&error))?;
    Ok(envelope(view))
}

async fn game_rules(
    State(state): State<Arc<ServerState>>,
    Path(game_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ApiEnvelope<Ruleset>>, ApiError> {
    let game_id = parse_game_id(game_id)?;
    let authenticated =
        authenticate(&state, &headers, &game_id, CapabilityScope::ObservePublic).await?;
    let view = state
        .runtime
        .service()
        .public_game(&authenticated.public_viewer(), PublicGameQuery { game_id })
        .await
        .map_err(|error| map_application_error(&error))?;
    let ruleset = match view.game.state.ruleset_id {
        RulesetId::EnglishV1 => Ruleset::english_v1(),
        RulesetId::FrenchV1 => Ruleset::french_v1(),
    };
    Ok(envelope(ruleset))
}

async fn game_action(
    State(state): State<Arc<ServerState>>,
    Path(game_id): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<GameActionRequest>, JsonRejection>,
) -> Result<Json<ApiEnvelope<word_arena_application::GameActionResult>>, ApiError> {
    let Json(request) = payload.map_err(|error| json_rejection(&error))?;
    let game_id = parse_game_id(game_id)?;
    let authenticated = authenticate(&state, &headers, &game_id, CapabilityScope::Act).await?;
    let AuthenticatedCredential::Seat(credential) = authenticated else {
        return Err(ApiError::unauthorized());
    };
    let result = state
        .runtime
        .service()
        .act(
            &credential,
            GameActionCommand {
                game_id: game_id.clone(),
                expected_version: request.expected_version,
                turn: Turn {
                    number: request.turn_number,
                    seat: credential.seat(),
                },
                idempotency_key: request.idempotency_key,
                action: request.action,
            },
        )
        .await
        .map_err(|error| map_application_error(&error))?;
    state.notifications.publish(GameInvalidation {
        schema_version: API_SCHEMA_VERSION,
        game_id: game_id.clone(),
        version: result.game.public.state.version,
    });
    state.mcp.notify_game_updated(&game_id).await;
    Ok(envelope(result))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct EventQuery {
    after_version: u64,
}

async fn game_events(
    websocket: WebSocketUpgrade,
    State(state): State<Arc<ServerState>>,
    Path(game_id): Path<String>,
    query: Result<Query<EventQuery>, QueryRejection>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let Query(query) = query.map_err(|_| invalid_payload())?;
    let game_id = parse_game_id(game_id)?;
    let (token, browser_subprotocol) = websocket_bearer_token(&headers)?;
    let authenticated = state
        .runtime
        .authenticate_capability(&token, &game_id, CapabilityScope::ObservePublic)
        .await
        .map_err(|error| map_capability_error(&error))?;
    let public_credential = authenticated.public_viewer();
    let receiver = state.notifications.subscribe(&game_id);
    let current = state
        .runtime
        .service()
        .public_game(
            &public_credential,
            PublicGameQuery {
                game_id: game_id.clone(),
            },
        )
        .await
        .map_err(|error| map_application_error(&error))?;
    if query.after_version > current.game.state.version {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "future_version",
            "after_version exceeds the authoritative version",
        ));
    }
    let permit = Arc::clone(&state.websocket_slots)
        .try_acquire_owned()
        .map_err(|_| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "connection_limit",
                "too many websocket connections",
            )
        })?;
    let websocket = if browser_subprotocol {
        websocket.protocols([BROWSER_WEBSOCKET_PROTOCOL])
    } else {
        websocket
    };
    Ok(websocket
        .max_message_size(8 * 1024)
        .max_frame_size(8 * 1024)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            websocket_loop(
                socket,
                state,
                public_credential,
                game_id,
                query.after_version,
                current.game.state.version,
                receiver,
            )
            .await;
        }))
}

async fn websocket_loop(
    mut socket: WebSocket,
    state: Arc<ServerState>,
    public_credential: word_arena_application::PublicViewerCredential,
    game_id: GameId,
    mut last_version: u64,
    current_version: u64,
    mut receiver: broadcast::Receiver<GameInvalidation>,
) {
    if current_version > last_version {
        if send_invalidation(&mut socket, &game_id, current_version)
            .await
            .is_err()
        {
            return;
        }
        last_version = current_version;
    }
    loop {
        tokio::select! {
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                Some(Ok(Message::Ping(bytes))) => {
                    if socket.send(Message::Pong(bytes)).await.is_err() { return; }
                }
                Some(Ok(_)) => {}
            },
            notification = receiver.recv() => match notification {
                Ok(notification) if notification.version > last_version => {
                    if send_marker(&mut socket, &notification).await.is_err() { return; }
                    last_version = notification.version;
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let current = state.runtime.service().public_game(
                        &public_credential,
                        PublicGameQuery { game_id: game_id.clone() },
                    ).await;
                    let Ok(current) = current else { return; };
                    let version = current.game.state.version;
                    if version > last_version {
                        if send_invalidation(&mut socket, &game_id, version).await.is_err() { return; }
                        last_version = version;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }
}

async fn send_invalidation(
    socket: &mut WebSocket,
    game_id: &GameId,
    version: u64,
) -> Result<(), axum::Error> {
    send_marker(
        socket,
        &GameInvalidation {
            schema_version: API_SCHEMA_VERSION,
            game_id: game_id.clone(),
            version,
        },
    )
    .await
}

async fn send_marker(
    socket: &mut WebSocket,
    invalidation: &GameInvalidation,
) -> Result<(), axum::Error> {
    let json = serde_json::to_string(invalidation).map_err(axum::Error::new)?;
    socket.send(Message::Text(json.into())).await
}

async fn authenticate(
    state: &ServerState,
    headers: &HeaderMap,
    game_id: &GameId,
    scope: CapabilityScope,
) -> Result<AuthenticatedCredential, ApiError> {
    let token = bearer_token(headers)?;
    state
        .runtime
        .authenticate_capability(token, game_id, scope)
        .await
        .map_err(|error| map_capability_error(&error))
}

fn bearer_token(headers: &HeaderMap) -> Result<&str, ApiError> {
    let value = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(ApiError::unauthorized)?;
    value
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty() && token.len() <= 256)
        .ok_or_else(ApiError::unauthorized)
}

fn websocket_bearer_token(headers: &HeaderMap) -> Result<(String, bool), ApiError> {
    if let Ok(token) = bearer_token(headers) {
        return Ok((token.to_owned(), false));
    }
    let protocols = headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(ApiError::unauthorized)?;
    let mut values = protocols.split(',').map(str::trim);
    if !values.any(|value| value == BROWSER_WEBSOCKET_PROTOCOL) {
        return Err(ApiError::unauthorized());
    }
    let token = protocols
        .split(',')
        .map(str::trim)
        .find(|value| value.starts_with("wa_cap_v1.") && value.len() <= 256)
        .ok_or_else(ApiError::unauthorized)?;
    Ok((token.to_owned(), true))
}

fn parse_game_id(value: String) -> Result<GameId, ApiError> {
    GameId::new(value).map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_game_id",
            "game ID is invalid",
        )
    })
}

fn invalid_payload() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "invalid_request",
        "request payload does not match the V1 schema",
    )
}

fn json_rejection(error: &JsonRejection) -> ApiError {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            "request payload exceeds the configured limit",
        )
    } else {
        invalid_payload()
    }
}

fn envelope<T>(data: T) -> Json<ApiEnvelope<T>> {
    Json(ApiEnvelope {
        schema_version: API_SCHEMA_VERSION,
        data,
    })
}

fn map_capability_error(error: &CapabilityError) -> ApiError {
    match error {
        CapabilityError::Unauthorized | CapabilityError::InvalidRequest => ApiError::unauthorized(),
        CapabilityError::Game(RepositoryError::NotFound) => ApiError::new(
            StatusCode::NOT_FOUND,
            "game_not_found",
            "game was not found",
        ),
        CapabilityError::EntropyUnavailable
        | CapabilityError::Game(_)
        | CapabilityError::Repository(_) => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "service_unavailable",
            "service is temporarily unavailable",
        ),
    }
}

fn map_application_error(error: &ApplicationError) -> ApiError {
    match error {
        ApplicationError::Repository(RepositoryError::NotFound) => ApiError::new(
            StatusCode::NOT_FOUND,
            "game_not_found",
            "game was not found",
        ),
        ApplicationError::Repository(RepositoryError::Conflict)
        | ApplicationError::Engine(word_arena_engine::GameError::StaleVersion { .. })
        | ApplicationError::ActionRejected(
            word_arena_application::ActionRejection::VersionConflict
            | word_arena_application::ActionRejection::IdempotencyConflict,
        ) => ApiError::new(
            StatusCode::CONFLICT,
            "version_conflict",
            "game version changed",
        ),
        ApplicationError::WrongGameAuthority { .. }
        | ApplicationError::WrongSeatAuthority { .. } => ApiError::unauthorized(),
        ApplicationError::InvalidGameId | ApplicationError::InvalidIdempotencyKey => ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "request is invalid",
        ),
        ApplicationError::TurnVersionMismatch { .. }
        | ApplicationError::Engine(_)
        | ApplicationError::ActionRejected(
            word_arena_application::ActionRejection::IllegalAction { .. },
        ) => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "illegal_action",
            "action is not legal",
        ),
        ApplicationError::ActionRejected(
            word_arena_application::ActionRejection::DeadlineNotReached,
        ) => ApiError::new(
            StatusCode::CONFLICT,
            "deadline_not_reached",
            "turn deadline has not been reached",
        ),
        ApplicationError::PracticeOnly => ApiError::new(
            StatusCode::FORBIDDEN,
            "practice_only",
            "move preview is available only in practice games",
        ),
        ApplicationError::PreviewRateLimited { .. } => ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "preview_rate_limited",
            "move preview rate limit reached",
        ),
        ApplicationError::ReplayNotReady => ApiError::new(
            StatusCode::CONFLICT,
            "replay_not_ready",
            "replay is available after the game finishes",
        ),
        ApplicationError::MissingLexicon { .. }
        | ApplicationError::PreviewUnavailable
        | ApplicationError::Repository(_) => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "service_unavailable",
            "service is temporarily unavailable",
        ),
    }
}

fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin([
            HeaderValue::from_static("http://127.0.0.1:5173"),
            HeaderValue::from_static("http://localhost:5173"),
        ])
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers([
            header::ACCEPT,
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            HeaderName::from_static("last-event-id"),
            HeaderName::from_static("mcp-protocol-version"),
            HeaderName::from_static("mcp-session-id"),
        ])
        .expose_headers([
            HeaderName::from_static("mcp-protocol-version"),
            HeaderName::from_static("mcp-session-id"),
        ])
}

async fn shutdown_signal() {
    if tokio::signal::ctrl_c().await.is_err() {
        tracing::error!("failed to install shutdown signal handler");
    }
    tracing::info!("graceful shutdown requested");
}
