use std::{collections::BTreeSet, sync::Arc, time::Duration};

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use tower::ServiceExt;
use word_arena_application::{
    AgentRunId, ApplicationClock, ApplicationRuntime, CapabilityAdapters, CapabilityDigestKey,
    CapabilityRepository, CapabilityRole, CapabilityScope, GameId, GameIdSource, GameRepository,
    IdempotencyKey, IssueCapabilityRequest, LexiconResolver, SeedSource, UnixMillis,
    test_support::{
        InMemoryCapabilityRepository, InMemoryGameRepository, InMemoryLexiconResolver, ManualClock,
        SequenceCapabilityTokens, SequenceGameIds, SequenceSeeds,
    },
};
use word_arena_engine::{Language, Ruleset, Seat, WordValidator};
use word_arena_lexicon::{NormalizedKey, PackIdentity};
use word_arena_server::{GameInvalidation, MCP_PROTOCOL_VERSION, ServerState, api_app};

const NOW: UnixMillis = UnixMillis(1_700_000_000_000);

#[derive(Debug)]
struct AcceptingLexicon(PackIdentity);

impl WordValidator for AcceptingLexicon {
    fn identity(&self) -> &PackIdentity {
        &self.0
    }

    fn contains(&self, _key: &NormalizedKey) -> bool {
        true
    }
}

#[tokio::test]
async fn create_public_observe_and_errors_use_strict_scoped_http() {
    let fixture = fixture();
    let app = api_app(Arc::clone(&fixture.state));
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/games",
            None,
            &json!({"language":"english","idempotency_key":"create"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let created = response_json(response).await;
    assert_eq!(created["schema_version"], 1);
    let game_id = created["data"]["game_id"].as_str().unwrap();
    let token = created["data"]["public_capability"].as_str().unwrap();
    assert_no_keys(&created, &["rack", "racks", "seed", "bag", "snapshot"]);

    let missing_auth = app
        .clone()
        .oneshot(empty_request(
            Method::GET,
            &format!("/api/v1/games/{game_id}/public"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(missing_auth.status(), StatusCode::UNAUTHORIZED);
    let error = response_json(missing_auth).await;
    assert_eq!(error["code"], "unauthorized");
    assert_eq!(error["schema_version"], 1);

    let public = app
        .clone()
        .oneshot(empty_request(
            Method::GET,
            &format!("/api/v1/games/{game_id}/public"),
            Some(token),
        ))
        .await
        .unwrap();
    assert_eq!(public.status(), StatusCode::OK);
    let public = response_json(public).await;
    assert_no_keys(&public, &["rack", "racks", "seed", "bag", "snapshot"]);

    let escalated = app
        .clone()
        .oneshot(empty_request(
            Method::GET,
            &format!("/api/v1/games/{game_id}/seat"),
            Some(token),
        ))
        .await
        .unwrap();
    assert_eq!(escalated.status(), StatusCode::UNAUTHORIZED);

    let rules = get_json(
        app.clone(),
        &format!("/api/v1/games/{game_id}/rules"),
        token,
    )
    .await;
    assert_eq!(rules["data"]["id"], "english-v1");

    let unknown_field = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/games",
            None,
            &json!({"language":"english","idempotency_key":"strict","role":"administrator"}),
        ))
        .await
        .unwrap();
    assert_eq!(unknown_field.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(unknown_field).await["code"],
        "invalid_request"
    );

    let oversized = app
        .oneshot(json_request(
            Method::POST,
            "/api/v1/games",
            None,
            &json!({
                "language":"english",
                "idempotency_key":"x".repeat(70_000)
            }),
        ))
        .await
        .unwrap();
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn role_routes_serialize_only_their_bound_projection() {
    let fixture = fixture();
    let game_id = create_game(&fixture, "roles").await;
    let seat_token = issue(
        &fixture,
        &game_id,
        CapabilityRole::Seat(Seat::One),
        [
            CapabilityScope::ObservePublic,
            CapabilityScope::ObserveSeat,
            CapabilityScope::Act,
        ],
        Some("seat-run"),
    )
    .await;
    let spectator_token = issue(
        &fixture,
        &game_id,
        CapabilityRole::HumanSpectator,
        [
            CapabilityScope::ObservePublic,
            CapabilityScope::ObserveHumanSpectator,
        ],
        None,
    )
    .await;
    let administrator_token = issue(
        &fixture,
        &game_id,
        CapabilityRole::Administrator,
        [CapabilityScope::ObserveAdministrator],
        None,
    )
    .await;
    let app = api_app(Arc::clone(&fixture.state));

    let seat = get_json(
        app.clone(),
        &format!("/api/v1/games/{game_id}/seat"),
        &seat_token,
    )
    .await;
    assert!(seat.to_string().contains("\"rack\""));
    assert_no_keys(&seat, &["racks", "seed", "bag", "snapshot"]);

    let spectator = get_json(
        app.clone(),
        &format!("/api/v1/games/{game_id}/spectator"),
        &spectator_token,
    )
    .await;
    assert!(spectator.to_string().contains("\"racks\""));
    assert_no_keys(&spectator, &["seed", "bag", "snapshot"]);

    let administrator = get_json(
        app,
        &format!("/api/v1/games/{game_id}/administrator"),
        &administrator_token,
    )
    .await;
    assert!(administrator.to_string().contains("snapshot"));
}

#[tokio::test]
async fn actions_derive_seat_from_auth_and_reject_privilege_escalation() {
    let fixture = fixture();
    let game_id = create_game(&fixture, "actions").await;
    let seat_token = issue(
        &fixture,
        &game_id,
        CapabilityRole::Seat(Seat::One),
        [CapabilityScope::ObserveSeat, CapabilityScope::Act],
        None,
    )
    .await;
    let spectator_token = issue(
        &fixture,
        &game_id,
        CapabilityRole::HumanSpectator,
        [CapabilityScope::ObserveHumanSpectator],
        None,
    )
    .await;
    let app = api_app(Arc::clone(&fixture.state));

    let denied = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/api/v1/games/{game_id}/actions"),
            Some(&spectator_token),
            &action_body(0),
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

    let accepted = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/api/v1/games/{game_id}/actions"),
            Some(&seat_token),
            &action_body(0),
        ))
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::OK);
    let accepted = response_json(accepted).await;
    assert_eq!(accepted["data"]["game"]["public"]["state"]["version"], 1);

    let caller_selected_seat = app
        .oneshot(json_request(
            Method::POST,
            &format!("/api/v1/games/{game_id}/actions"),
            Some(&seat_token),
            &json!({
                "expected_version":1,
                "turn_number":1,
                "idempotency_key":"bad-shape",
                "action":{"type":"pass"},
                "seat":"two"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(caller_selected_seat.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn websocket_reconnects_from_version_and_rest_converges() {
    let fixture = fixture();
    let game_id = create_game(&fixture, "websocket").await;
    let seat_token = issue(
        &fixture,
        &game_id,
        CapabilityRole::Seat(Seat::One),
        [
            CapabilityScope::ObservePublic,
            CapabilityScope::ObserveSeat,
            CapabilityScope::Act,
        ],
        None,
    )
    .await;
    let public_token = issue(
        &fixture,
        &game_id,
        CapabilityRole::Public,
        [CapabilityScope::ObservePublic],
        None,
    )
    .await;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server_state = Arc::clone(&fixture.state);
    let server = tokio::spawn(async move {
        axum::serve(listener, api_app(server_state)).await.unwrap();
    });
    let url = format!("ws://{address}/api/v1/games/{game_id}/events?after_version=0");
    let mut websocket = connect_websocket(&url, &public_token).await;

    let app = api_app(Arc::clone(&fixture.state));
    let action = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/api/v1/games/{game_id}/actions"),
            Some(&seat_token),
            &action_body(0),
        ))
        .await
        .unwrap();
    assert_eq!(action.status(), StatusCode::OK);
    let marker = receive_marker(&mut websocket).await;
    assert_eq!(marker.game_id, game_id);
    assert_eq!(marker.version, 1);
    websocket.close(None).await.unwrap();

    let mut reconnected = connect_websocket(&url, &public_token).await;
    let replayed = receive_marker(&mut reconnected).await;
    assert_eq!(replayed.version, 1);
    let snapshot = get_json(
        app,
        &format!("/api/v1/games/{game_id}/public"),
        &public_token,
    )
    .await;
    assert_eq!(snapshot["data"]["game"]["state"]["version"], 1);
    reconnected.close(None).await.unwrap();
    server.abort();
}

#[tokio::test]
async fn authenticated_mcp_handshake_exposes_metadata_without_game_tools() {
    let fixture = fixture();
    let game_id = create_game(&fixture, "mcp-handshake").await;
    let seat_token = issue(
        &fixture,
        &game_id,
        CapabilityRole::Seat(Seat::One),
        [CapabilityScope::Act],
        Some("mcp-seat-one"),
    )
    .await;
    let app = api_app(Arc::clone(&fixture.state));
    let (session_id, initialized) = mcp_initialize(app.clone(), &game_id, &seat_token).await;
    assert_eq!(
        initialized["result"]["protocolVersion"],
        MCP_PROTOCOL_VERSION
    );
    assert_eq!(initialized["result"]["serverInfo"]["name"], "word-arena");
    assert_eq!(initialized["result"]["capabilities"], json!({}));

    let tools = app
        .oneshot(mcp_request(
            Method::POST,
            &game_id,
            Some(&seat_token),
            Some(&session_id),
            Some(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}})),
        ))
        .await
        .unwrap();
    assert_eq!(tools.status(), StatusCode::OK);
    assert_eq!(mcp_response_json(tools).await["result"]["tools"], json!([]));
}

#[tokio::test]
async fn mcp_authentication_and_session_binding_reject_cross_capability_reuse() {
    let fixture = fixture();
    let game_id = create_game(&fixture, "mcp-isolation").await;
    let other_game_id = create_game(&fixture, "mcp-other-game").await;
    let seat_one = issue(
        &fixture,
        &game_id,
        CapabilityRole::Seat(Seat::One),
        [CapabilityScope::Act],
        Some("mcp-one"),
    )
    .await;
    let seat_two = issue(
        &fixture,
        &game_id,
        CapabilityRole::Seat(Seat::Two),
        [CapabilityScope::Act],
        Some("mcp-two"),
    )
    .await;
    let app = api_app(Arc::clone(&fixture.state));
    let (session_id, _) = mcp_initialize(app.clone(), &game_id, &seat_one).await;

    for request in [
        mcp_request(
            Method::POST,
            &game_id,
            Some("invalid"),
            Some(&session_id),
            Some(&json!({"jsonrpc":"2.0","id":2,"method":"ping"})),
        ),
        mcp_request(
            Method::POST,
            &game_id,
            Some(&seat_two),
            Some(&session_id),
            Some(&json!({"jsonrpc":"2.0","id":3,"method":"ping"})),
        ),
        mcp_request(
            Method::POST,
            &other_game_id,
            Some(&seat_one),
            Some(&session_id),
            Some(&json!({"jsonrpc":"2.0","id":4,"method":"ping"})),
        ),
    ] {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn mcp_malformed_message_cancellation_and_session_delete_are_bounded() {
    let fixture = fixture();
    let game_id = create_game(&fixture, "mcp-cancel").await;
    let token = issue(
        &fixture,
        &game_id,
        CapabilityRole::Seat(Seat::One),
        [CapabilityScope::Act],
        Some("mcp-cancel-seat"),
    )
    .await;
    let app = api_app(Arc::clone(&fixture.state));
    let mut foreign_origin = mcp_request(
        Method::POST,
        &game_id,
        Some(&token),
        None,
        Some(&json!({
            "jsonrpc":"2.0",
            "id":0,
            "method":"initialize",
            "params":{
                "protocolVersion":MCP_PROTOCOL_VERSION,
                "capabilities":{},
                "clientInfo":{"name":"foreign","version":"1"}
            }
        })),
    );
    foreign_origin
        .headers_mut()
        .insert(header::ORIGIN, "https://example.invalid".parse().unwrap());
    let foreign_origin = app.clone().oneshot(foreign_origin).await.unwrap();
    assert_eq!(foreign_origin.status(), StatusCode::FORBIDDEN);

    let malformed = app
        .clone()
        .oneshot(
            request_builder(
                Method::POST,
                &format!("/api/v1/games/{game_id}/mcp"),
                Some(&token),
            )
            .header(header::HOST, "localhost")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header("mcp-protocol-version", MCP_PROTOCOL_VERSION)
            .body(Body::from("{"))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let (session_id, _) = mcp_initialize(app.clone(), &game_id, &token).await;
    let cancelled = app
        .clone()
        .oneshot(mcp_request(
            Method::POST,
            &game_id,
            Some(&token),
            Some(&session_id),
            Some(&json!({
                "jsonrpc":"2.0",
                "method":"notifications/cancelled",
                "params":{"requestId":77,"reason":"test cancellation"}
            })),
        ))
        .await
        .unwrap();
    assert_eq!(cancelled.status(), StatusCode::ACCEPTED);

    let deleted = app
        .clone()
        .oneshot(mcp_request(
            Method::DELETE,
            &game_id,
            Some(&token),
            Some(&session_id),
            None,
        ))
        .await
        .unwrap();
    assert!(deleted.status().is_success());
    let after_delete = app
        .oneshot(mcp_request(
            Method::POST,
            &game_id,
            Some(&token),
            Some(&session_id),
            Some(&json!({"jsonrpc":"2.0","id":8,"method":"ping"})),
        ))
        .await
        .unwrap();
    assert_eq!(after_delete.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mcp_gateway_cancels_active_sessions_for_graceful_shutdown() {
    let fixture = fixture();
    let game_id = create_game(&fixture, "mcp-shutdown").await;
    let token = issue(
        &fixture,
        &game_id,
        CapabilityRole::Seat(Seat::One),
        [CapabilityScope::Act],
        Some("mcp-shutdown-seat"),
    )
    .await;
    let app = api_app(Arc::clone(&fixture.state));
    let (session_id, _) = mcp_initialize(app.clone(), &game_id, &token).await;
    let stream = app
        .oneshot(mcp_request(
            Method::GET,
            &game_id,
            Some(&token),
            Some(&session_id),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);
    fixture.state.cancel_mcp();
    let closed = tokio::time::timeout(
        Duration::from_millis(250),
        to_bytes(stream.into_body(), 128 * 1024),
    )
    .await;
    assert!(
        closed.is_ok(),
        "MCP SSE stream did not close during shutdown"
    );
}

struct Fixture {
    runtime: Arc<ApplicationRuntime>,
    state: Arc<ServerState>,
}

fn fixture() -> Fixture {
    let validators = [Language::English, Language::French].map(|language| {
        let ruleset = Ruleset::for_language(language).unwrap();
        Arc::new(AcceptingLexicon(ruleset.lexicon)) as Arc<dyn WordValidator>
    });
    let game_repository: Arc<dyn GameRepository> = Arc::new(InMemoryGameRepository::default());
    let resolver: Arc<dyn LexiconResolver> = Arc::new(InMemoryLexiconResolver::new(validators));
    let ids: Arc<dyn GameIdSource> = Arc::new(SequenceGameIds::new("http-game"));
    let seeds: Arc<dyn SeedSource> = Arc::new(SequenceSeeds::new(99));
    let clock: Arc<dyn ApplicationClock> = Arc::new(ManualClock::new(NOW));
    let capabilities: Arc<dyn CapabilityRepository> =
        Arc::new(InMemoryCapabilityRepository::default());
    let runtime = Arc::new(ApplicationRuntime::new(
        game_repository,
        resolver,
        ids,
        seeds,
        clock,
        CapabilityAdapters::new(
            capabilities,
            Arc::new(SequenceCapabilityTokens::new(1)),
            CapabilityDigestKey::new([21; 32]),
        ),
    ));
    let state = Arc::new(ServerState::new(Arc::clone(&runtime)));
    Fixture { runtime, state }
}

async fn create_game(fixture: &Fixture, key: &str) -> GameId {
    let service = fixture.runtime.service();
    service
        .create_game(
            service.prepare_create_game(Language::English, IdempotencyKey::new(key).unwrap()),
        )
        .await
        .unwrap()
        .game_id
}

async fn issue<const N: usize>(
    fixture: &Fixture,
    game_id: &GameId,
    role: CapabilityRole,
    scopes: [CapabilityScope; N],
    agent_run_id: Option<&str>,
) -> String {
    fixture
        .runtime
        .issue_capability(IssueCapabilityRequest {
            game_id: game_id.clone(),
            role,
            scopes: scopes.into_iter().collect::<BTreeSet<_>>(),
            expires_at: UnixMillis(NOW.0 + 60_000),
            agent_run_id: agent_run_id.map(|id| AgentRunId::new(id).unwrap()),
        })
        .await
        .unwrap()
        .token
        .into_secret()
}

fn empty_request(method: Method, uri: &str, token: Option<&str>) -> Request<Body> {
    request_builder(method, uri, token)
        .body(Body::empty())
        .unwrap()
}

fn json_request(method: Method, uri: &str, token: Option<&str>, body: &Value) -> Request<Body> {
    request_builder(method, uri, token)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn mcp_request(
    method: Method,
    game_id: &GameId,
    token: Option<&str>,
    session_id: Option<&str>,
    body: Option<&Value>,
) -> Request<Body> {
    let mut builder = request_builder(method, &format!("/api/v1/games/{game_id}/mcp"), token)
        .header(header::HOST, "localhost")
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header("mcp-protocol-version", MCP_PROTOCOL_VERSION);
    if let Some(session_id) = session_id {
        builder = builder.header("mcp-session-id", session_id);
    }
    match body {
        Some(body) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

async fn mcp_initialize(app: axum::Router, game_id: &GameId, token: &str) -> (String, Value) {
    let response = app
        .clone()
        .oneshot(mcp_request(
            Method::POST,
            game_id,
            Some(token),
            None,
            Some(&json!({
                "jsonrpc":"2.0",
                "id":1,
                "method":"initialize",
                "params":{
                    "protocolVersion":MCP_PROTOCOL_VERSION,
                    "capabilities":{},
                    "clientInfo":{"name":"word-arena-test","version":"1"}
                }
            })),
        ))
        .await
        .unwrap();
    if response.status() != StatusCode::OK {
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
        panic!(
            "MCP initialize returned {status}: {}",
            String::from_utf8_lossy(&bytes)
        );
    }
    let session_id = response
        .headers()
        .get("mcp-session-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let initialized = mcp_response_json(response).await;
    let notification = app
        .oneshot(mcp_request(
            Method::POST,
            game_id,
            Some(token),
            Some(&session_id),
            Some(&json!({"jsonrpc":"2.0","method":"notifications/initialized"})),
        ))
        .await
        .unwrap();
    assert_eq!(notification.status(), StatusCode::ACCEPTED);
    (session_id, initialized)
}

async fn mcp_response_json(response: axum::response::Response) -> Value {
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    if content_type.starts_with("text/event-stream") {
        let text = std::str::from_utf8(&bytes).unwrap();
        let data = text
            .lines()
            .filter_map(|line| line.strip_prefix("data:").map(str::trim))
            .find(|data| !data.is_empty())
            .unwrap();
        serde_json::from_str(data).unwrap()
    } else {
        serde_json::from_slice(&bytes).unwrap()
    }
}

fn request_builder(method: Method, uri: &str, token: Option<&str>) -> axum::http::request::Builder {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    builder
}

fn action_body(version: u64) -> Value {
    json!({
        "expected_version":version,
        "turn_number":version,
        "idempotency_key":format!("action-{version}"),
        "action":{"type":"pass"}
    })
}

async fn response_json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn get_json(app: axum::Router, uri: &str, token: &str) -> Value {
    let response = app
        .oneshot(empty_request(Method::GET, uri, Some(token)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

async fn connect_websocket(
    url: &str,
    token: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    connect_async(request).await.unwrap().0
}

async fn receive_marker(
    websocket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> GameInvalidation {
    let message = tokio::time::timeout(Duration::from_secs(2), websocket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let Message::Text(text) = message else {
        panic!("expected text invalidation");
    };
    serde_json::from_str(&text).unwrap()
}

fn assert_no_keys(value: &Value, forbidden: &[&str]) {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                assert!(!forbidden.contains(&key.as_str()), "forbidden key {key}");
                assert_no_keys(child, forbidden);
            }
        }
        Value::Array(values) => {
            for child in values {
                assert_no_keys(child, forbidden);
            }
        }
        _ => {}
    }
}
