use std::{
    collections::{BTreeSet, HashMap},
    fmt::Write as _,
    sync::Arc,
    time::Duration,
};

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream},
    net::TcpListener,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use tower::ServiceExt;
use word_arena_agent_runtime::HarnessExecutables;
use word_arena_application::{
    AgentRunId, ApplicationClock, ApplicationRuntime, AuditOutcome, CapabilityAdapters,
    CapabilityDigestKey, CapabilityError, CapabilityRole, CapabilityScope, GameId, GameIdSource,
    GameRepository, IdempotencyKey, IssueCapabilityRequest, LexiconResolver, PreviewPolicy,
    SeedSource, UnixMillis,
    test_support::{
        InMemoryCapabilityRepository, InMemoryGameRepository, ManualClock,
        SequenceCapabilityTokens, SequenceGameIds, SequenceSeeds,
    },
};
use word_arena_cli::{
    args::ConfigOverrides, bridge::run_bridge, client::RemoteClient, config::ResolvedConfig,
    error::CliError,
};
use word_arena_engine::{
    Game, GameMode, Language, PROJECTION_SCHEMA_VERSION, REPLAY_SCHEMA_VERSION, Ruleset, Seat,
    WordValidator,
};
use word_arena_lexicon::{
    BuilderDescriptor, FileDescriptor, NormalizedKey, PackIdentity, PackManifest, PolicyDescriptor,
    REQUIRED_PAYLOAD_FILES, SourceDescriptor,
};
use word_arena_server::{
    AGENT_CATALOG_PATH, AGENT_MATCH_ACTIVITY_PATH, AGENT_MATCH_RECOVERY_PATH,
    AGENT_MATCH_STATUS_PATH, AGENT_MATCHES_PATH, API_SCHEMA_VERSION, AgentMatchManager,
    AgentMatchManagerConfig, BROWSER_WEBSOCKET_PROTOCOL, GAME_EVENTS_PATH, GameInvalidation,
    MCP_PROTOCOL_VERSION, PUBLIC_GAME_PATH, SEAT_GAME_PATH, SPECTATOR_GAME_PATH,
    SPECTATOR_REPLAY_PATH, ServerState, api_app,
};

const NOW: UnixMillis = UnixMillis(1_700_000_000_000);

fn four_player_agent_seats() -> Value {
    json!([
        {"kind":"agent","harness":"codex"},
        {"kind":"human","name":"Human"},
        {"kind":"agent","harness":"claude_code"},
        {"kind":"agent","harness":"cline"}
    ])
}

#[test]
fn web_api_contract_matches_authoritative_server_constants() {
    let contract: Value =
        serde_json::from_str(include_str!("../../../contracts/web-api-v1.json")).unwrap();
    assert_eq!(contract["api_schema_version"], API_SCHEMA_VERSION);
    assert_eq!(
        contract["projection_schema_version"],
        PROJECTION_SCHEMA_VERSION
    );
    assert_eq!(contract["replay_schema_version"], REPLAY_SCHEMA_VERSION);
    assert_eq!(
        contract["player_count"],
        json!({"minimum":2,"default":2,"maximum":4})
    );
    assert_eq!(
        contract["seat_values"],
        json!(["one", "two", "three", "four"])
    );
    assert_eq!(
        contract["browser_websocket_protocol"],
        BROWSER_WEBSOCKET_PROTOCOL
    );
    assert_eq!(contract["projection_paths"]["public"], PUBLIC_GAME_PATH);
    assert_eq!(contract["projection_paths"]["seat"], SEAT_GAME_PATH);
    assert_eq!(
        contract["projection_paths"]["spectator"],
        SPECTATOR_GAME_PATH
    );
    assert_eq!(contract["events_path"], GAME_EVENTS_PATH);
    assert_eq!(contract["agent_paths"]["catalog"], AGENT_CATALOG_PATH);
    assert_eq!(contract["agent_paths"]["matches"], AGENT_MATCHES_PATH);
    assert_eq!(contract["agent_paths"]["status"], AGENT_MATCH_STATUS_PATH);
    assert_eq!(
        contract["agent_paths"]["activity"],
        AGENT_MATCH_ACTIVITY_PATH
    );
    assert_eq!(
        contract["agent_paths"]["spectator_recovery"],
        AGENT_MATCH_RECOVERY_PATH
    );
    assert_eq!(contract["spectator_replay_path"], SPECTATOR_REPLAY_PATH);
    assert_eq!(
        contract["view_fields"],
        json!(["observed_at", "turn_deadline", "game"])
    );
    assert_eq!(
        contract["turn_deadline_fields"],
        json!(["turn", "seat", "deadline_at", "policy_version"])
    );
    assert_eq!(
        contract["invalidation_fields"],
        json!(["schema_version", "game_id", "version"])
    );
}

#[tokio::test]
async fn agent_catalog_and_match_creation_fail_closed() {
    let fixture = fixture();
    let temporary = tempfile::tempdir().unwrap();
    let missing = temporary.path().join("not-installed").display().to_string();
    let manager = AgentMatchManager::new(AgentMatchManagerConfig {
        executables: HarnessExecutables {
            codex: missing.clone(),
            claude_code: missing.clone(),
            cline: missing.clone(),
            pi: missing,
        },
        workspace_root: temporary.path().join("runs"),
        mcp_origin: "http://127.0.0.1:3000".to_owned(),
        codex_auth_file: None,
        match_repository: None,
    });
    let state = Arc::new(ServerState::with_agent_manager(
        Arc::clone(&fixture.runtime),
        manager,
    ));
    let app = api_app(state);

    let catalog = app
        .clone()
        .oneshot(empty_request(Method::GET, AGENT_CATALOG_PATH, None))
        .await
        .unwrap();
    assert_eq!(catalog.status(), StatusCode::OK);
    let catalog = response_json(catalog).await;
    let entries = catalog["data"].as_array().unwrap();
    assert_eq!(entries.len(), 4);
    assert!(entries.iter().all(|entry| entry["available"] == false));

    let no_agent = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            AGENT_MATCHES_PATH,
            None,
            &json!({
                "language":"english",
                "seats":[
                    {"kind":"human","name":"Alice"},
                    {"kind":"human","name":"Bob"}
                ],
                "idempotency_key":"no-agent"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(no_agent.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(response_json(no_agent).await["code"], "agent_required");

    let unavailable = app
        .oneshot(json_request(
            Method::POST,
            AGENT_MATCHES_PATH,
            None,
            &json!({
                "language":"english",
                "seats":four_player_agent_seats(),
                "idempotency_key":"unavailable-agent"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(unavailable.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        response_json(unavailable).await["code"],
        "agent_unavailable"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn local_match_list_and_spectator_recovery_are_refresh_safe() {
    use std::os::unix::fs::PermissionsExt as _;

    let fixture = fixture();
    let temporary = tempfile::tempdir().unwrap();
    let executable = temporary.path().join("fake-agent");
    std::fs::write(
        &executable,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'fake 999.0.0'; exit 0; fi\nexit 1\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&executable, permissions).unwrap();
    let executable = executable.display().to_string();
    let manager = AgentMatchManager::new(AgentMatchManagerConfig {
        executables: HarnessExecutables {
            codex: executable.clone(),
            claude_code: executable.clone(),
            cline: executable.clone(),
            pi: executable,
        },
        workspace_root: temporary.path().join("runs"),
        mcp_origin: "http://127.0.0.1:9".to_owned(),
        codex_auth_file: None,
        match_repository: None,
    });
    let state = Arc::new(ServerState::with_agent_manager(
        Arc::clone(&fixture.runtime),
        manager,
    ));
    let app = api_app(state);

    let created = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            AGENT_MATCHES_PATH,
            None,
            &json!({
                "language":"english",
                "mode":"practice",
                "seats":four_player_agent_seats(),
                "idempotency_key":"refresh-safe"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created = response_json(created).await;
    let game_id = created["data"]["game_id"].as_str().unwrap();
    assert_eq!(
        created["data"]["public"]["state"]["scores"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        created["data"]["status"]["seats"].as_array().unwrap().len(),
        4
    );

    let listed = app
        .clone()
        .oneshot(empty_request(Method::GET, AGENT_MATCHES_PATH, None))
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = response_json(listed).await;
    assert_eq!(listed["data"]["matches"][0]["game_id"], game_id);
    assert_eq!(listed["data"]["matches"][0]["language"], "english");
    assert_eq!(listed["data"]["matches"][0]["mode"], "practice");
    assert_no_keys(
        &listed["data"],
        &["capability", "token", "rack", "bag", "seed"],
    );

    let recovery_path = AGENT_MATCH_RECOVERY_PATH.replace("{game_id}", game_id);
    let recovered = app
        .clone()
        .oneshot(empty_request(Method::POST, &recovery_path, None))
        .await
        .unwrap();
    assert_eq!(recovered.status(), StatusCode::OK);
    let recovered = response_json(recovered).await;
    let spectator = recovered["data"]["spectator_capability"].as_str().unwrap();
    assert_ne!(spectator, created["data"]["spectator_capability"]);

    let status_path = AGENT_MATCH_STATUS_PATH.replace("{game_id}", game_id);
    let reopened = app
        .oneshot(empty_request(Method::GET, &status_path, Some(spectator)))
        .await
        .unwrap();
    assert_eq!(reopened.status(), StatusCode::OK);
    assert_eq!(
        response_json(reopened).await["data"]["seats"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
}

#[tokio::test]
async fn agent_activity_is_human_spectator_only() {
    let fixture = fixture();
    let game_id = create_game(&fixture, "activity-auth").await;
    let public = issue(
        &fixture,
        &game_id,
        CapabilityRole::Public,
        [CapabilityScope::ObservePublic],
        None,
    )
    .await;
    let spectator = issue(
        &fixture,
        &game_id,
        CapabilityRole::HumanSpectator,
        [CapabilityScope::ObserveHumanSpectator],
        None,
    )
    .await;
    let app = api_app(Arc::clone(&fixture.state));
    let path = AGENT_MATCH_ACTIVITY_PATH.replace("{game_id}", game_id.as_str());

    let denied = app
        .clone()
        .oneshot(empty_request(Method::GET, &path, Some(&public)))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let absent = app
        .oneshot(empty_request(Method::GET, &path, Some(&spectator)))
        .await
        .unwrap();
    assert_eq!(absent.status(), StatusCode::NOT_FOUND);
}

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

#[derive(Debug)]
struct FixtureLexicons([(Arc<AcceptingLexicon>, PackManifest); 2]);

impl LexiconResolver for FixtureLexicons {
    fn resolve(&self, identity: &PackIdentity) -> Option<Arc<dyn WordValidator>> {
        self.0
            .iter()
            .find(|(validator, _)| validator.identity() == identity)
            .map(|(validator, _)| {
                let validator: Arc<AcceptingLexicon> = Arc::clone(validator);
                validator as Arc<dyn WordValidator>
            })
    }

    fn manifest(&self, identity: &PackIdentity) -> Option<PackManifest> {
        self.0
            .iter()
            .find(|(_, manifest)| manifest.identity() == *identity)
            .map(|(_, manifest)| manifest.clone())
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
    let spectator_token = created["data"]["spectator_capability"].as_str().unwrap();
    assert!(spectator_token.starts_with("wa_cap_v1."));
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
    assert_eq!(public["data"]["turn_deadline"]["turn"], 0);
    assert_eq!(public["data"]["turn_deadline"]["seat"], "one");
    assert!(public["data"]["turn_deadline"]["deadline_at"].is_number());

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
async fn finished_replay_requires_human_spectator_authority_and_reveals_exact_inputs() {
    let fixture = fixture();
    let game_id = create_game(&fixture, "replay-route").await;
    let seat_token = issue(
        &fixture,
        &game_id,
        CapabilityRole::Seat(Seat::One),
        [CapabilityScope::Act],
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
    let spectator_token = issue(
        &fixture,
        &game_id,
        CapabilityRole::HumanSpectator,
        [CapabilityScope::ObserveHumanSpectator],
        None,
    )
    .await;
    let app = api_app(Arc::clone(&fixture.state));

    let active = app
        .clone()
        .oneshot(empty_request(
            Method::GET,
            &format!("/api/v1/games/{game_id}/spectator/replay"),
            Some(&spectator_token),
        ))
        .await
        .unwrap();
    assert_eq!(active.status(), StatusCode::CONFLICT);

    let finished = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/api/v1/games/{game_id}/actions"),
            Some(&seat_token),
            &json!({
                "expected_version": 0,
                "turn_number": 0,
                "idempotency_key": "replay-resign",
                "action": {"type": "resign"}
            }),
        ))
        .await
        .unwrap();
    assert_eq!(finished.status(), StatusCode::OK);

    let denied = app
        .clone()
        .oneshot(empty_request(
            Method::GET,
            &format!("/api/v1/games/{game_id}/spectator/replay"),
            Some(&public_token),
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

    let replay = get_json(
        app,
        &format!("/api/v1/games/{game_id}/spectator/replay"),
        &spectator_token,
    )
    .await;
    assert_eq!(
        replay["data"]["replay"]["schema_version"],
        REPLAY_SCHEMA_VERSION
    );
    assert!(replay["data"]["replay"]["seed_reveal"].is_array());
    assert_eq!(
        replay["data"]["replay"]["ruleset_identity"]["ruleset_id"],
        "english-v1"
    );
    assert_eq!(replay["data"]["replay"]["lexicon"]["locale"], "en");
    assert_no_keys(
        &replay,
        &[
            "capability",
            "public_capability",
            "spectator_capability",
            "snapshot",
            "bag",
        ],
    );
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
    assert_eq!(accepted["data"]["turn_deadline"]["turn"], 1);
    assert_eq!(accepted["data"]["turn_deadline"]["seat"], "two");

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
    let mut websocket = connect_browser_websocket(&url, &public_token).await;

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
    assert_eq!(
        initialized["result"]["capabilities"],
        json!({"resources":{"subscribe":true},"tools":{}})
    );

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
    let contract: Value =
        serde_json::from_str(include_str!("snapshots/mcp_client_contract_v1.json")).unwrap();
    assert_eq!(contract["protocol_version"], MCP_PROTOCOL_VERSION);
    let actual_names = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    let expected_names = contract["competitive_tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(actual_names, expected_names);
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
    assert_eq!(
        schema_digest,
        contract["competitive_tool_schema_sha256"].as_str().unwrap()
    );
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end narrative proves the complete practice-preview security boundary"
)]
async fn practice_preview_is_scoped_hidden_bounded_audited_and_non_mutating() {
    let fixture = fixture_with_preview_policy(PreviewPolicy {
        version: 3,
        max_requests: 2,
        window_ms: 60_000,
    });
    let competitive = create_game(&fixture, "preview-competitive").await;
    assert!(matches!(
        fixture
            .runtime
            .issue_capability(IssueCapabilityRequest {
                game_id: competitive.clone(),
                role: CapabilityRole::Seat(Seat::One),
                scopes: BTreeSet::from([CapabilityScope::Act, CapabilityScope::Preview]),
                expires_at: UnixMillis(NOW.0 + 60_000),
                agent_run_id: Some(AgentRunId::new("invalid-competitive-preview").unwrap()),
            })
            .await,
        Err(CapabilityError::InvalidRequest)
    ));
    let competitive_token = issue(
        &fixture,
        &competitive,
        CapabilityRole::Seat(Seat::One),
        [CapabilityScope::Act],
        Some("competitive-preview-denial"),
    )
    .await;
    let app = api_app(Arc::clone(&fixture.state));
    let (competitive_session, _) =
        mcp_initialize(app.clone(), &competitive, &competitive_token).await;
    let competitive_tools = mcp_rpc(
        app.clone(),
        &competitive,
        &competitive_token,
        &competitive_session,
        20,
        "tools/list",
        json!({}),
    )
    .await;
    let competitive_names = competitive_tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(!competitive_names.contains(&"preview_tiles"));
    let denied = mcp_call(
        app.clone(),
        &competitive,
        &competitive_token,
        &competitive_session,
        21,
        "preview_tiles",
        json!({
            "schema_version":1,
            "expected_version":0,
            "turn_id":0,
            "placements":[]
        }),
    )
    .await;
    assert_eq!(denied["isError"], true);
    assert!(
        denied["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("unauthorized")
    );

    let practice = create_game_mode(
        &fixture,
        "preview-practice",
        Language::English,
        GameMode::Practice,
    )
    .await;
    let practice_token = issue(
        &fixture,
        &practice,
        CapabilityRole::Seat(Seat::One),
        [CapabilityScope::Act, CapabilityScope::Preview],
        Some("practice-preview"),
    )
    .await;
    let (practice_session, _) = mcp_initialize(app.clone(), &practice, &practice_token).await;
    let practice_tools = mcp_rpc(
        app.clone(),
        &practice,
        &practice_token,
        &practice_session,
        22,
        "tools/list",
        json!({}),
    )
    .await;
    let practice_names = practice_tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(practice_names.len(), 7);
    assert!(practice_names.contains(&"preview_tiles"));

    let before = mcp_call(
        app.clone(),
        &practice,
        &practice_token,
        &practice_session,
        23,
        "observe_game",
        json!({"schema_version":1}),
    )
    .await;
    assert_eq!(
        before["structuredContent"]["game"]["public"]["state"]["mode"],
        "practice"
    );
    let rack = before["structuredContent"]["game"]["rack"]
        .as_array()
        .unwrap();
    let placements = rack
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
    let preview_arguments = json!({
        "schema_version":1,
        "expected_version":0,
        "turn_id":0,
        "placements":placements
    });
    let preview = mcp_call(
        app.clone(),
        &practice,
        &practice_token,
        &practice_session,
        24,
        "preview_tiles",
        preview_arguments.clone(),
    )
    .await;
    assert_eq!(preview["isError"], false);
    assert_eq!(preview["structuredContent"]["base_version"], 0);
    let after = mcp_call(
        app.clone(),
        &practice,
        &practice_token,
        &practice_session,
        25,
        "observe_game",
        json!({"schema_version":1}),
    )
    .await;
    assert_eq!(
        after["structuredContent"]["game"],
        before["structuredContent"]["game"]
    );

    let second = mcp_call(
        app.clone(),
        &practice,
        &practice_token,
        &practice_session,
        26,
        "preview_tiles",
        preview_arguments.clone(),
    )
    .await;
    assert_eq!(second["structuredContent"], preview["structuredContent"]);
    let limited = mcp_call(
        app.clone(),
        &practice,
        &practice_token,
        &practice_session,
        27,
        "preview_tiles",
        preview_arguments.clone(),
    )
    .await;
    assert_eq!(limited["isError"], true);
    assert!(
        limited["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("rate_limited")
    );

    let mut commit_arguments = preview_arguments;
    commit_arguments["idempotency_key"] = json!("preview-equivalent-commit");
    let committed = mcp_call(
        app,
        &practice,
        &practice_token,
        &practice_session,
        28,
        "play_tiles",
        commit_arguments,
    )
    .await;
    assert_eq!(committed["isError"], false);
    assert_eq!(
        committed["structuredContent"]["event"],
        preview["structuredContent"]["event"]
    );
    let preview_audits = fixture
        .capabilities
        .audits()
        .into_iter()
        .filter(|audit| audit.scope == Some(CapabilityScope::Preview))
        .collect::<Vec<_>>();
    assert!(
        preview_audits
            .iter()
            .any(|audit| audit.outcome == AuditOutcome::DeniedScope)
    );
    assert!(
        preview_audits
            .iter()
            .any(|audit| audit.outcome == AuditOutcome::Success)
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
        assert_persisted_replay(&fixture, &game_id).await;
    }
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "the full two-client stdio lifecycle is intentionally visible as one protocol scenario"
)]
async fn scripted_stdio_mcp_clients_finish_and_replay_english_and_french() {
    let mut scenarios = Vec::new();
    for language in [Language::English, Language::French] {
        let fixture = fixture();
        let game_id = create_game_language(&fixture, language.code(), language).await;
        let seat_one_token = issue(
            &fixture,
            &game_id,
            CapabilityRole::Seat(Seat::One),
            [CapabilityScope::Act],
            Some(&format!("stdio-{}-one", language.code())),
        )
        .await;
        let seat_two_token = issue(
            &fixture,
            &game_id,
            CapabilityRole::Seat(Seat::Two),
            [CapabilityScope::Act],
            Some(&format!("stdio-{}-two", language.code())),
        )
        .await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_state = Arc::clone(&fixture.state);
        let server = tokio::spawn(async move {
            axum::serve(listener, api_app(server_state)).await.unwrap();
        });
        let server_url = format!("http://{address}");
        let mut seat_one = StdioMcpClient::connect(&server_url, &game_id, &seat_one_token).await;
        let mut seat_two = StdioMcpClient::connect(&server_url, &game_id, &seat_two_token).await;

        let tools = seat_one.rpc("tools/list", json!({})).await;
        assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 6);
        let resources = seat_one.rpc("resources/list", json!({})).await;
        assert_eq!(
            resources["result"]["resources"].as_array().unwrap().len(),
            5
        );
        let history_uri = format!("word-arena://games/{game_id}/history");
        let history = seat_one
            .rpc("resources/read", json!({"uri":history_uri}))
            .await;
        assert_eq!(
            history["result"]["contents"][0]["mimeType"],
            "application/json"
        );
        let observed_one = seat_one
            .call("observe_game", json!({"schema_version":1}))
            .await;
        let observed_two = seat_two
            .call("observe_game", json!({"schema_version":1}))
            .await;
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

        for version in 0..6_u64 {
            let arguments = json!({
                "schema_version":1,
                "expected_version":version,
                "turn_id":version,
                "idempotency_key":format!("stdio-{}-pass-{version}", language.code())
            });
            let client = if version % 2 == 0 {
                &mut seat_one
            } else {
                &mut seat_two
            };
            let passed = client.call("pass_turn", arguments.clone()).await;
            assert_eq!(passed["isError"], false);
            assert_eq!(
                passed["structuredContent"]["game"]["public"]["state"]["version"],
                version + 1
            );
            if version == 0 {
                let retried = client.call("pass_turn", arguments).await;
                assert_eq!(retried["structuredContent"], passed["structuredContent"]);
            }
        }
        let final_one = seat_one
            .call("observe_game", json!({"schema_version":1}))
            .await;
        let final_two = seat_two
            .call("observe_game", json!({"schema_version":1}))
            .await;
        for final_view in [&final_one, &final_two] {
            assert_eq!(
                final_view["structuredContent"]["game"]["public"]["state"]["phase"],
                "finished"
            );
            assert_eq!(
                final_view["structuredContent"]["game"]["public"]["events"]
                    .as_array()
                    .unwrap()
                    .len(),
                7
            );
            assert_no_keys(
                &final_view["structuredContent"],
                &["racks", "bag", "seed", "snapshot"],
            );
        }
        seat_one.close().await;
        seat_two.close().await;
        server.abort();
        assert_persisted_replay(&fixture, &game_id).await;
        scenarios.push(json!({
            "language":language.code(),
            "transport":"stdio_bridge",
            "clients":2,
            "committed_actions":6,
            "retry_verified":true,
            "terminal_phase":"finished",
            "replay_verified":true
        }));
    }
    let transcript = json!({
        "schema_version":1,
        "protocol_version":MCP_PROTOCOL_VERSION,
        "contains_credentials":false,
        "contains_private_game_data":false,
        "scenarios":scenarios
    });
    let expected: Value =
        serde_json::from_str(include_str!("snapshots/mcp_stdio_scenarios_v1.json")).unwrap();
    assert_eq!(transcript, expected);
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one table-driven contract test compares every resource across both languages"
)]
async fn authenticated_mcp_resources_are_stable_and_private_in_both_languages() {
    for language in [Language::English, Language::French] {
        let fixture = fixture();
        let game_id = create_game_language(&fixture, language.code(), language).await;
        let token = issue(
            &fixture,
            &game_id,
            CapabilityRole::Seat(Seat::One),
            [CapabilityScope::Act],
            Some(&format!("resources-{}", language.code())),
        )
        .await;
        let app = api_app(Arc::clone(&fixture.state));
        let (session_id, _) = mcp_initialize(app.clone(), &game_id, &token).await;
        let prefix = format!("word-arena://games/{game_id}");

        let listed = mcp_rpc(
            app.clone(),
            &game_id,
            &token,
            &session_id,
            30,
            "resources/list",
            json!({}),
        )
        .await;
        let uris = listed["result"]["resources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|resource| resource["uri"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            uris,
            [
                format!("{prefix}/public"),
                format!("{prefix}/seat"),
                format!("{prefix}/history"),
                format!("{prefix}/ruleset"),
                format!("{prefix}/lexicon-manifest"),
            ]
        );
        assert!(
            listed["result"]["resources"]
                .as_array()
                .unwrap()
                .iter()
                .all(|resource| resource["mimeType"] == "application/json")
        );

        let templates = mcp_rpc(
            app.clone(),
            &game_id,
            &token,
            &session_id,
            31,
            "resources/templates/list",
            json!({}),
        )
        .await;
        let template_uris = templates["result"]["resourceTemplates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|resource| resource["uriTemplate"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            template_uris,
            [
                "word-arena://games/{game_id}/public",
                "word-arena://games/{game_id}/seat",
                "word-arena://games/{game_id}/history",
                "word-arena://games/{game_id}/ruleset",
                "word-arena://games/{game_id}/lexicon-manifest",
            ]
        );
        let contract: Value =
            serde_json::from_str(include_str!("snapshots/mcp_client_contract_v1.json")).unwrap();
        let actual_templates = template_uris.into_iter().collect::<BTreeSet<_>>();
        let expected_templates = contract["resource_templates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|uri| uri.as_str().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(actual_templates, expected_templates);

        let public = mcp_read_resource(
            app.clone(),
            &game_id,
            &token,
            &session_id,
            32,
            &format!("{prefix}/public"),
        )
        .await;
        let seat = mcp_read_resource(
            app.clone(),
            &game_id,
            &token,
            &session_id,
            33,
            &format!("{prefix}/seat"),
        )
        .await;
        let history = mcp_read_resource(
            app.clone(),
            &game_id,
            &token,
            &session_id,
            34,
            &format!("{prefix}/history"),
        )
        .await;
        let ruleset = mcp_read_resource(
            app.clone(),
            &game_id,
            &token,
            &session_id,
            35,
            &format!("{prefix}/ruleset"),
        )
        .await;
        let manifest = mcp_read_resource(
            app.clone(),
            &game_id,
            &token,
            &session_id,
            36,
            &format!("{prefix}/lexicon-manifest"),
        )
        .await;

        for resource in [&public, &seat, &history, &ruleset, &manifest] {
            assert_eq!(resource["schema_version"], 1);
            assert_eq!(resource["game_id"], game_id.as_str());
            assert_eq!(resource["version"], 0);
            assert_no_keys(resource, &["racks", "bag", "seed", "snapshot"]);
        }
        assert!(public["data"].get("rack").is_none());
        assert_eq!(seat["data"]["seat"], "one");
        assert!(seat["data"]["rack"].is_array());
        assert_eq!(
            history["data"]["public_events"].as_array().unwrap().len(),
            1
        );
        assert!(
            history["data"]["private_events"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            ruleset["data"]["language"],
            serde_json::to_value(language).unwrap()
        );
        assert_eq!(
            manifest["data"]["pack_id"],
            public["data"]["state"]["lexicon"]["pack_id"]
        );
        assert_eq!(
            manifest["data"]["content_sha256"],
            public["data"]["state"]["lexicon"]["content_sha256"]
        );

        let observed = mcp_call(
            app.clone(),
            &game_id,
            &token,
            &session_id,
            37,
            "observe_game",
            json!({"schema_version":1}),
        )
        .await;
        assert_eq!(
            observed["structuredContent"]["game"]["public"]["state"]["version"],
            public["version"]
        );

        let other_game =
            create_game_language(&fixture, &format!("other-{}", language.code()), language).await;
        let forbidden = mcp_rpc(
            app.clone(),
            &game_id,
            &token,
            &session_id,
            38,
            "resources/read",
            json!({"uri":format!("word-arena://games/{other_game}/public")}),
        )
        .await;
        assert_eq!(forbidden["error"]["code"], -32602);
        assert!(
            forbidden["error"]["message"]
                .as_str()
                .unwrap()
                .contains("forbidden")
        );

        let malformed = mcp_rpc(
            app,
            &game_id,
            &token,
            &session_id,
            39,
            "resources/read",
            json!({"uri":format!("{prefix}/public/extra")}),
        )
        .await;
        assert_eq!(malformed["error"]["code"], -32602);
        assert!(
            malformed["error"]["message"]
                .as_str()
                .unwrap()
                .contains("invalid_resource_uri")
        );
    }
}

#[tokio::test]
async fn mcp_resource_subscription_receives_game_updates_and_unsubscribes() {
    let fixture = fixture();
    let game_id = create_game(&fixture, "resource-subscribe").await;
    let token = issue(
        &fixture,
        &game_id,
        CapabilityRole::Seat(Seat::One),
        [CapabilityScope::Act],
        Some("resource-subscriber"),
    )
    .await;
    let app = api_app(Arc::clone(&fixture.state));
    let (session_id, _) = mcp_initialize(app.clone(), &game_id, &token).await;
    let uri = format!("word-arena://games/{game_id}/public");

    let subscribed = mcp_rpc(
        app.clone(),
        &game_id,
        &token,
        &session_id,
        40,
        "resources/subscribe",
        json!({"uri":uri}),
    )
    .await;
    assert!(subscribed.get("error").is_none());

    let stream = app
        .clone()
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
    let mut stream = stream.into_body().into_data_stream();
    let passed = mcp_call(
        app.clone(),
        &game_id,
        &token,
        &session_id,
        41,
        "pass_turn",
        json!({
            "schema_version":1,
            "expected_version":0,
            "turn_id":0,
            "idempotency_key":"resource-subscription-pass"
        }),
    )
    .await;
    assert_eq!(passed["isError"], false);

    let notification = receive_mcp_sse_notification(&mut stream).await;
    assert_eq!(notification["method"], "notifications/resources/updated");
    assert_eq!(notification["params"]["uri"], uri);

    let unsubscribed = mcp_rpc(
        app,
        &game_id,
        &token,
        &session_id,
        42,
        "resources/unsubscribe",
        json!({"uri":uri}),
    )
    .await;
    assert!(unsubscribed.get("error").is_none());
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
    capabilities: Arc<InMemoryCapabilityRepository>,
    games: Arc<InMemoryGameRepository>,
}

fn fixture() -> Fixture {
    fixture_with_preview_policy(PreviewPolicy::default())
}

fn fixture_with_preview_policy(preview_policy: PreviewPolicy) -> Fixture {
    let lexicons = [Language::English, Language::French].map(|language| {
        let ruleset = Ruleset::for_language(language).unwrap();
        let manifest = fixture_manifest(&ruleset.lexicon);
        (Arc::new(AcceptingLexicon(ruleset.lexicon)), manifest)
    });
    let games = Arc::new(InMemoryGameRepository::default());
    let resolver: Arc<dyn LexiconResolver> = Arc::new(FixtureLexicons(lexicons));
    let ids: Arc<dyn GameIdSource> = Arc::new(SequenceGameIds::new("http-game"));
    let seeds: Arc<dyn SeedSource> = Arc::new(SequenceSeeds::new(99));
    let clock: Arc<dyn ApplicationClock> = Arc::new(ManualClock::new(NOW));
    let capabilities = Arc::new(InMemoryCapabilityRepository::default());
    let runtime = Arc::new(
        ApplicationRuntime::new(
            games.clone(),
            resolver,
            ids,
            seeds,
            clock,
            CapabilityAdapters::new(
                capabilities.clone(),
                Arc::new(SequenceCapabilityTokens::new(1)),
                CapabilityDigestKey::new([21; 32]),
            ),
        )
        .with_preview_policy(preview_policy),
    );
    let state = Arc::new(ServerState::new(Arc::clone(&runtime)));
    Fixture {
        runtime,
        state,
        capabilities,
        games,
    }
}

fn fixture_manifest(identity: &PackIdentity) -> PackManifest {
    let manifest = PackManifest {
        format_version: identity.format_version,
        pack_id: identity.pack_id.clone(),
        pack_version: identity.pack_version.clone(),
        locale: identity.locale.clone(),
        word_count: 1,
        content_sha256: identity.content_sha256.clone(),
        normalization: identity.normalization.clone(),
        source: SourceDescriptor {
            id: format!("fixture-{}", identity.locale),
            revision: "1".to_owned(),
            archive_sha256: "11".repeat(32),
            license_id: "CC0-1.0".to_owned(),
        },
        policy: PolicyDescriptor {
            id: "fixture-v1".to_owned(),
            version: 1,
        },
        builder: BuilderDescriptor {
            name: "fixture-builder".to_owned(),
            version: "1.0.0".to_owned(),
        },
        files: REQUIRED_PAYLOAD_FILES
            .map(|path| FileDescriptor {
                path: path.to_owned(),
                size_bytes: 0,
                sha256: "22".repeat(32),
            })
            .into_iter()
            .collect(),
    };
    manifest.validate_schema().unwrap();
    manifest
}

async fn create_game(fixture: &Fixture, key: &str) -> GameId {
    create_game_language(fixture, key, Language::English).await
}

async fn create_game_language(fixture: &Fixture, key: &str, language: Language) -> GameId {
    create_game_mode(fixture, key, language, GameMode::Competitive).await
}

async fn create_game_mode(
    fixture: &Fixture,
    key: &str,
    language: Language,
    mode: GameMode,
) -> GameId {
    let service = fixture.runtime.service();
    service
        .create_game(service.prepare_create_game_with_mode(
            language,
            mode,
            IdempotencyKey::new(key).unwrap(),
        ))
        .await
        .unwrap()
        .game_id
}

struct StdioMcpClient {
    input: DuplexStream,
    output: BufReader<DuplexStream>,
    bridge: tokio::task::JoinHandle<Result<(), CliError>>,
    next_id: u64,
}

impl StdioMcpClient {
    async fn connect(server_url: &str, game_id: &GameId, token: &str) -> Self {
        let config = ResolvedConfig::load_from(
            ConfigOverrides {
                server_url: Some(server_url.to_owned()),
                game_id: Some(game_id.to_string()),
                token: Some(token.to_owned()),
                timeout_ms: Some(2_000),
            },
            None,
            &HashMap::new(),
        )
        .unwrap();
        let client = RemoteClient::new(config).unwrap();
        let (input, bridge_input) = tokio::io::duplex(64 * 1024);
        let (bridge_output, output) = tokio::io::duplex(64 * 1024);
        let bridge = tokio::spawn(run_bridge(
            bridge_input,
            bridge_output,
            client,
            tokio_util::sync::CancellationToken::new(),
        ));
        let mut client = Self {
            input,
            output: BufReader::new(output),
            bridge,
            next_id: 1,
        };
        let initialized = client
            .rpc(
                "initialize",
                json!({
                    "protocolVersion":MCP_PROTOCOL_VERSION,
                    "capabilities":{},
                    "clientInfo":{"name":"word-arena-scripted-stdio","version":"1"}
                }),
            )
            .await;
        assert_eq!(
            initialized["result"]["protocolVersion"],
            MCP_PROTOCOL_VERSION
        );
        client.notify("notifications/initialized", json!({})).await;
        client
    }

    async fn call(&mut self, name: &str, arguments: Value) -> Value {
        let response = self
            .rpc("tools/call", json!({"name":name,"arguments":arguments}))
            .await;
        assert!(
            response.get("error").is_none(),
            "stdio MCP protocol error: {response}"
        );
        response["result"].clone()
    }

    async fn rpc(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.write(json!({
            "jsonrpc":"2.0",
            "id":id,
            "method":method,
            "params":params
        }))
        .await;
        loop {
            let mut line = String::new();
            self.output.read_line(&mut line).await.unwrap();
            assert!(
                !line.is_empty(),
                "stdio MCP bridge closed before response {id}"
            );
            let response: Value = serde_json::from_str(line.trim_end()).unwrap();
            if response["id"] == id {
                return response;
            }
            assert!(
                response.get("method").is_some(),
                "unexpected stdio MCP response: {response}"
            );
        }
    }

    async fn notify(&mut self, method: &str, params: Value) {
        self.write(json!({
            "jsonrpc":"2.0",
            "method":method,
            "params":params
        }))
        .await;
    }

    async fn write(&mut self, message: Value) {
        let bytes = serde_json::to_vec(&message).unwrap();
        self.input.write_all(&bytes).await.unwrap();
        self.input.write_all(b"\n").await.unwrap();
        self.input.flush().await.unwrap();
    }

    async fn close(mut self) {
        self.input.shutdown().await.unwrap();
        self.bridge.await.unwrap().unwrap();
    }
}

async fn assert_persisted_replay(fixture: &Fixture, game_id: &GameId) {
    let stored = fixture.games.load(game_id).await.unwrap();
    let recovery = fixture.games.load_recovery(game_id).await.unwrap();
    let lexicon = Arc::new(AcceptingLexicon(recovery.replay.lexicon.clone()));
    let replayed = Game::replay(&recovery.replay, Some(lexicon)).unwrap();
    assert_eq!(replayed.snapshot(), stored.snapshot);
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
    let response = mcp_rpc(
        app,
        game_id,
        token,
        session_id,
        id,
        "tools/call",
        json!({"name":name,"arguments":arguments}),
    )
    .await;
    assert!(
        response.get("error").is_none(),
        "MCP protocol error: {response}"
    );
    response["result"].clone()
}

async fn mcp_rpc(
    app: axum::Router,
    game_id: &GameId,
    token: &str,
    session_id: &str,
    id: u64,
    method: &str,
    params: Value,
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
                "method":method,
                "params":params
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    mcp_response_json(response).await
}

async fn mcp_read_resource(
    app: axum::Router,
    game_id: &GameId,
    token: &str,
    session_id: &str,
    id: u64,
    uri: &str,
) -> Value {
    let response = mcp_rpc(
        app,
        game_id,
        token,
        session_id,
        id,
        "resources/read",
        json!({"uri":uri}),
    )
    .await;
    assert!(
        response.get("error").is_none(),
        "MCP resource error: {response}"
    );
    let content = &response["result"]["contents"][0];
    assert_eq!(content["uri"], uri);
    assert_eq!(content["mimeType"], "application/json");
    serde_json::from_str(content["text"].as_str().unwrap()).unwrap()
}

async fn receive_mcp_sse_notification(stream: &mut axum::body::BodyDataStream) -> Value {
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut pending = String::new();
        while let Some(chunk) = stream.next().await {
            pending.push_str(std::str::from_utf8(&chunk.unwrap()).unwrap());
            while let Some(end) = pending.find("\n\n") {
                let frame = pending.drain(..end + 2).collect::<String>();
                if let Some(data) = frame
                    .lines()
                    .find_map(|line| line.strip_prefix("data:").map(str::trim))
                    .filter(|data| !data.is_empty())
                {
                    let message: Value = serde_json::from_str(data).unwrap();
                    if message.get("method").is_some() {
                        return message;
                    }
                }
            }
        }
        panic!("MCP SSE stream closed before a notification arrived");
    })
    .await
    .expect("MCP resource notification timed out")
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

async fn connect_browser_websocket(
    url: &str,
    token: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        header::SEC_WEBSOCKET_PROTOCOL,
        format!("word-arena-v1, {token}").parse().unwrap(),
    );
    let (websocket, response) = connect_async(request).await.unwrap();
    assert_eq!(
        response.headers().get(header::SEC_WEBSOCKET_PROTOCOL),
        Some(&"word-arena-v1".parse().unwrap())
    );
    websocket
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
