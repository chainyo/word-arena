use std::{
    collections::HashMap,
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{HeaderMap, Method, Request, Response, StatusCode, header},
    response::IntoResponse,
    routing::{any, get, post},
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use word_arena_cli::{
    args::ConfigOverrides, bridge::run_bridge, client::RemoteClient, config::ResolvedConfig,
    error::CliError,
};

#[tokio::test]
async fn health_auth_action_and_private_replay_have_stable_json() {
    let fixture = MockServer::start(false).await;
    let client = fixture.client();
    assert_eq!(
        client.health().await.unwrap(),
        json!({"status":"ok","schema_version":1})
    );
    let auth = client.authenticate().await.unwrap();
    assert_eq!(
        serde_json::to_string_pretty(&auth).unwrap(),
        include_str!("snapshots/auth.json").trim()
    );
    let action = client
        .action(0, 0, "pass-0".to_owned(), json!({"type":"pass"}))
        .await
        .unwrap();
    assert_eq!(action["data"]["event"]["kind"]["type"], "passed");
    let replay = client.replay_export().await.unwrap();
    assert_eq!(
        serde_json::to_string_pretty(&replay).unwrap(),
        include_str!("snapshots/replay.json").trim()
    );
    let replay_text = replay.to_string();
    assert!(!replay_text.contains("opponent_rack"));
    assert!(!replay_text.contains("future_bag"));
    assert!(!replay_text.contains("test-token"));
}

#[tokio::test]
async fn stdio_bridge_preserves_frames_and_streamable_http_session() {
    let fixture = MockServer::start(false).await;
    let client = fixture.client();
    let (mut input_writer, input_reader) = tokio::io::duplex(16 * 1024);
    let (output_writer, mut output_reader) = tokio::io::duplex(16 * 1024);
    let bridge = tokio::spawn(run_bridge(
        input_reader,
        output_writer,
        client,
        CancellationToken::new(),
    ));
    input_writer
        .write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"pass_turn\",\"arguments\":{\"schema_version\":1,\"expected_version\":0,\"turn_id\":0,\"idempotency_key\":\"bridge-pass\"}}}\n{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/list\",\"params\":{}}\n",
        )
        .await
        .unwrap();
    input_writer.shutdown().await.unwrap();
    bridge.await.unwrap().unwrap();
    let mut output = String::new();
    output_reader.read_to_string(&mut output).await.unwrap();
    let frames = output
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0]["id"], 1);
    assert_eq!(frames[1]["id"], 2);
    assert_eq!(frames[1]["result"]["isError"], false);
    assert_eq!(frames[2]["id"], 3);
    let records = fixture.state.records.lock().unwrap().clone();
    assert!(records.iter().any(|record| record.method == "initialize"));
    assert!(
        records
            .iter()
            .any(|record| record.method == "tools/call" && record.has_session)
    );
    assert!(
        records
            .iter()
            .any(|record| record.method == "tools/list" && record.has_session)
    );
    assert!(
        records
            .iter()
            .any(|record| record.method == "DELETE" && record.has_session)
    );
}

#[tokio::test]
async fn bridge_maps_remote_failure_broken_pipe_and_cancellation_deterministically() {
    let remote = MockServer::start(true).await;
    let (mut input_writer, input_reader) = tokio::io::duplex(4096);
    input_writer
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n")
        .await
        .unwrap();
    input_writer.shutdown().await.unwrap();
    let remote_error = run_bridge(
        input_reader,
        tokio::io::sink(),
        remote.client(),
        CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(matches!(remote_error, CliError::Remote(_)));
    assert_eq!(remote_error.exit_code(), std::process::ExitCode::from(4));
    assert!(!remote_error.to_string().contains("test-token"));

    let healthy = MockServer::start(false).await;
    let (mut input_writer, input_reader) = tokio::io::duplex(4096);
    input_writer
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n")
        .await
        .unwrap();
    input_writer.shutdown().await.unwrap();
    let pipe_error = run_bridge(
        input_reader,
        BrokenPipe,
        healthy.client(),
        CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(matches!(pipe_error, CliError::Io(_)));
    assert_eq!(pipe_error.exit_code(), std::process::ExitCode::from(6));

    let (_input_writer, input_reader) = tokio::io::duplex(128);
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let interrupted = run_bridge(
        input_reader,
        tokio::io::sink(),
        healthy.client(),
        cancellation,
    )
    .await
    .unwrap_err();
    assert!(matches!(interrupted, CliError::Interrupted));
    assert_eq!(interrupted.exit_code(), std::process::ExitCode::from(130));
}

#[derive(Clone, Debug)]
struct McpRecord {
    method: String,
    has_session: bool,
}

#[derive(Debug)]
struct MockState {
    fail_mcp: bool,
    records: Mutex<Vec<McpRecord>>,
}

struct MockServer {
    base_url: String,
    state: Arc<MockState>,
    task: tokio::task::JoinHandle<()>,
}

impl MockServer {
    async fn start(fail_mcp: bool) -> Self {
        let state = Arc::new(MockState {
            fail_mcp,
            records: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .route("/health", get(health))
            .route("/api/v1/games/test-game/seat", get(seat))
            .route("/api/v1/games/test-game/actions", post(action))
            .route("/api/v1/games/test-game/mcp", any(mcp))
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            base_url: format!("http://{address}"),
            state,
            task,
        }
    }

    fn client(&self) -> RemoteClient {
        let config = ResolvedConfig::load_from(
            ConfigOverrides {
                server_url: Some(self.base_url.clone()),
                game_id: Some("test-game".to_owned()),
                token: Some("test-token".to_owned()),
                timeout_ms: Some(2_000),
            },
            None,
            &HashMap::new(),
        )
        .unwrap();
        RemoteClient::new(config).unwrap()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn health() -> Json<Value> {
    Json(json!({"status":"ok","schema_version":1}))
}

async fn seat(headers: HeaderMap) -> Response<Body> {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(json!({
        "schema_version":1,
        "data":{
            "observed_at":1_700_000_000_000_i64,
            "game":{
                "schema_version":1,
                "seat":"one",
                "public":{
                    "schema_version":1,
                    "state":{"game_id":"test-game","version":0},
                    "events":[{"sequence":0,"kind":{"type":"created"}}]
                },
                "rack":[{"id":1,"face":{"kind":"letter","token":"A"}}],
                "private_events":[{"sequence":0,"seat":"one","drawn":[1]}],
                "opponent_rack":["SECRET"]
            }
        }
    }))
    .into_response()
}

async fn action(headers: HeaderMap, Json(body): Json<Value>) -> Response<Body> {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    assert_eq!(body["action"], json!({"type":"pass"}));
    Json(json!({
        "schema_version":1,
        "data":{"event":{"sequence":1,"kind":{"type":"passed"}}}
    }))
    .into_response()
}

async fn mcp(State(state): State<Arc<MockState>>, request: Request<Body>) -> Response<Body> {
    let method = request.method().clone();
    let has_session = request.headers().contains_key("mcp-session-id");
    if !authorized(request.headers()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if method == Method::GET {
        return StatusCode::NOT_FOUND.into_response();
    }
    if method == Method::DELETE {
        state.records.lock().unwrap().push(McpRecord {
            method: "DELETE".to_owned(),
            has_session,
        });
        return StatusCode::NO_CONTENT.into_response();
    }
    if state.fail_mcp {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"code":"offline","message":"unavailable"})),
        )
            .into_response();
    }
    let bytes = axum::body::to_bytes(request.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let rpc_method = body["method"].as_str().unwrap().to_owned();
    state.records.lock().unwrap().push(McpRecord {
        method: rpc_method.clone(),
        has_session,
    });
    if rpc_method == "notifications/initialized" {
        return StatusCode::ACCEPTED.into_response();
    }
    let id = body.get("id").cloned();
    let payload = if rpc_method == "initialize" {
        json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2025-11-25","capabilities":{},"serverInfo":{"name":"mock","version":"1"}}})
    } else if rpc_method == "tools/call" {
        json!({"jsonrpc":"2.0","id":id,"result":{"isError":false,"structuredContent":{"event":{"kind":{"type":"passed"}}}}})
    } else {
        json!({"jsonrpc":"2.0","id":id,"result":{"tools":[]}})
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header("mcp-session-id", "test-session")
        .body(Body::from(payload.to_string()))
        .unwrap()
}

fn authorized(headers: &HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        == Some("Bearer test-token")
}

#[derive(Debug)]
struct BrokenPipe;

impl AsyncWrite for BrokenPipe {
    fn poll_write(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        _buffer: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed")))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Poll::Ready(Ok(()))
    }
}
