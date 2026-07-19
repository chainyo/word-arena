use std::{collections::BTreeSet, fmt::Write as _, sync::Arc, time::Duration};

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
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
async fn authenticated_mcp_handshake_exposes_competitive_tools() {
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
    assert_eq!(initialized["result"]["capabilities"], json!({"tools":{}}));

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
    let tools = mcp_response_json(tools).await;
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 6);
    let schema_bytes = serde_json::to_vec(&tools["result"]["tools"]).unwrap();
    let schema_digest =
        Sha256::digest(schema_bytes)
            .iter()
            .fold(String::with_capacity(64), |mut hex, byte| {
                write!(&mut hex, "{byte:02x}").unwrap();
                hex
            });
    assert_eq!(
        schema_digest,
        include_str!("snapshots/mcp_competitive_tools_v1.sha256").trim()
    );
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end narrative proves all six tools compose across both languages"
)]
async fn competitive_mcp_tools_complete_english_and_french_games_with_retry_and_privacy() {
    for (language, expected_ruleset) in [
        (Language::English, "english-v1"),
        (Language::French, "french-v1"),
    ] {
        let fixture = fixture();
        let language_key = language.code();
        let game_id = create_game_language(&fixture, language_key, language).await;
        let seat_one = issue(
            &fixture,
            &game_id,
            CapabilityRole::Seat(Seat::One),
            [CapabilityScope::Act],
            Some(&format!("mcp-{language_key}-one")),
        )
        .await;
        let seat_two = issue(
            &fixture,
            &game_id,
            CapabilityRole::Seat(Seat::Two),
            [CapabilityScope::Act],
            Some(&format!("mcp-{language_key}-two")),
        )
        .await;
        let app = api_app(Arc::clone(&fixture.state));
        let (session_one, _) = mcp_initialize(app.clone(), &game_id, &seat_one).await;
        let (session_two, _) = mcp_initialize(app.clone(), &game_id, &seat_two).await;

        let observed_one = mcp_call(
            app.clone(),
            &game_id,
            &seat_one,
            &session_one,
            10,
            "observe_game",
            json!({"schema_version":1}),
        )
        .await;
        let observed_two = mcp_call(
            app.clone(),
            &game_id,
            &seat_two,
            &session_two,
            11,
            "observe_game",
            json!({"schema_version":1}),
        )
        .await;
        assert_eq!(observed_one["isError"], false);
        assert_eq!(observed_two["isError"], false);
        assert_eq!(observed_one["structuredContent"]["schema_version"], 1);
        assert_eq!(observed_one["structuredContent"]["game"]["seat"], "one");
        assert_eq!(observed_two["structuredContent"]["game"]["seat"], "two");
        assert_no_keys(
            &observed_one["structuredContent"],
            &["racks", "bag", "seed", "snapshot"],
        );
        assert_no_keys(
            &observed_two["structuredContent"],
            &["racks", "bag", "seed", "snapshot"],
        );

        let rules = mcp_call(
            app.clone(),
            &game_id,
            &seat_one,
            &session_one,
            12,
            "get_ruleset",
            json!({"schema_version":1}),
        )
        .await;
        assert_eq!(rules["isError"], false);
        assert_eq!(
            rules["structuredContent"]["ruleset"]["id"],
            expected_ruleset
        );

        let invalid = mcp_call(
            app.clone(),
            &game_id,
            &seat_one,
            &session_one,
            13,
            "play_tiles",
            json!({
                "schema_version":1,
                "expected_version":0,
                "turn_id":0,
                "idempotency_key":format!("{language_key}-invalid"),
                "placements":[]
            }),
        )
        .await;
        assert_eq!(invalid["isError"], true);
        assert!(
            invalid["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("illegal_action")
        );

        let rack_one = observed_one["structuredContent"]["game"]["rack"]
            .as_array()
            .unwrap();
        let placements = rack_one
            .iter()
            .take(2)
            .enumerate()
            .map(|(offset, tile)| {
                let face = &tile["face"];
                let is_blank = face["kind"] == "blank";
                json!({
                    "tile_id":tile["id"],
                    "row":7,
                    "column":7 + offset,
                    "letter":if is_blank { "A" } else { face["token"].as_str().unwrap() },
                    "is_blank":is_blank
                })
            })
            .collect::<Vec<_>>();
        let play_arguments = json!({
            "schema_version":1,
            "expected_version":0,
            "turn_id":0,
            "idempotency_key":format!("{language_key}-play"),
            "placements":placements
        });
        let played = mcp_call(
            app.clone(),
            &game_id,
            &seat_one,
            &session_one,
            14,
            "play_tiles",
            play_arguments.clone(),
        )
        .await;
        assert_eq!(played["isError"], false);
        assert_eq!(
            played["structuredContent"]["game"]["public"]["state"]["version"],
            1
        );
        let retried = mcp_call(
            app.clone(),
            &game_id,
            &seat_one,
            &session_one,
            15,
            "play_tiles",
            play_arguments,
        )
        .await;
        assert_eq!(retried["structuredContent"], played["structuredContent"]);

        let stale = mcp_call(
            app.clone(),
            &game_id,
            &seat_two,
            &session_two,
            16,
            "pass_turn",
            json!({
                "schema_version":1,
                "expected_version":0,
                "turn_id":0,
                "idempotency_key":format!("{language_key}-stale")
            }),
        )
        .await;
        assert_eq!(stale["isError"], true);
        assert!(
            stale["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("version_conflict")
        );

        let rack_two = observed_two["structuredContent"]["game"]["rack"]
            .as_array()
            .unwrap();
        let exchanged = mcp_call(
            app.clone(),
            &game_id,
            &seat_two,
            &session_two,
            17,
            "exchange_tiles",
            json!({
                "schema_version":1,
                "expected_version":1,
                "turn_id":1,
                "idempotency_key":format!("{language_key}-exchange"),
                "tile_ids":[rack_two[0]["id"]]
            }),
        )
        .await;
        assert_eq!(exchanged["isError"], false);
        assert_eq!(
            exchanged["structuredContent"]["game"]["public"]["state"]["version"],
            2
        );

        let passed = mcp_call(
            app.clone(),
            &game_id,
            &seat_one,
            &session_one,
            18,
            "pass_turn",
            json!({
                "schema_version":1,
                "expected_version":2,
                "turn_id":2,
                "idempotency_key":format!("{language_key}-pass")
            }),
        )
        .await;
        assert_eq!(passed["isError"], false);
        assert_eq!(
            passed["structuredContent"]["game"]["public"]["state"]["version"],
            3
        );

        let resigned = mcp_call(
            app.clone(),
            &game_id,
            &seat_two,
            &session_two,
            19,
            "resign",
            json!({
                "schema_version":1,
                "expected_version":3,
                "turn_id":3,
                "idempotency_key":format!("{language_key}-resign")
            }),
        )
        .await;
        assert_eq!(resigned["isError"], false);
        assert_eq!(
            resigned["structuredContent"]["game"]["public"]["state"]["phase"],
            "finished"
        );
        assert_no_keys(
            &resigned["structuredContent"],
            &["racks", "bag", "seed", "snapshot"],
        );

        for (token, session, request_id) in
            [(&seat_one, &session_one, 20), (&seat_two, &session_two, 21)]
        {
            let replay_view = mcp_call(
                app.clone(),
                &game_id,
                token,
                session,
                request_id,
                "observe_game",
                json!({"schema_version":1}),
            )
            .await;
            assert_eq!(
                replay_view["structuredContent"]["game"]["public"]["events"]
                    .as_array()
                    .unwrap()
                    .len(),
                5
            );
            assert_eq!(
                replay_view["structuredContent"]["game"]["public"]["state"]["phase"],
                "finished"
            );
            assert_no_keys(
                &replay_view["structuredContent"],
                &["racks", "bag", "seed", "snapshot"],
            );
        }
    }
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
    create_game_language(fixture, key, Language::English).await
}

async fn create_game_language(fixture: &Fixture, key: &str, language: Language) -> GameId {
    let service = fixture.runtime.service();
    service
        .create_game(service.prepare_create_game(language, IdempotencyKey::new(key).unwrap()))
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

async fn mcp_call(
    app: axum::Router,
    game_id: &GameId,
    token: &str,
    session_id: &str,
    id: u64,
    name: &str,
    arguments: Value,
) -> Value {
    let response = app
        .oneshot(mcp_request(
            Method::POST,
            game_id,
            Some(token),
            Some(session_id),
            Some(&json!({
                "jsonrpc":"2.0",
                "id":id,
                "method":"tools/call",
                "params":{"name":name,"arguments":arguments}
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = mcp_response_json(response).await;
    assert!(
        response.get("error").is_none(),
        "MCP protocol error: {response}"
    );
    response["result"].clone()
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
