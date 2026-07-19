use std::{sync::Arc, time::Duration};

use futures_util::StreamExt;
use reqwest::{Method, StatusCode, header};
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{Mutex, mpsc},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::{
    client::{RemoteClient, bounded_body, send_mcp},
    error::{CliError, RemoteError},
};

const MAX_FRAME_BYTES: usize = 1024 * 1024;
const INITIAL_BACKOFF: Duration = Duration::from_millis(100);
const MAX_BACKOFF: Duration = Duration::from_secs(2);

/// Bridges process stdin/stdout to one authenticated Streamable HTTP session.
///
/// # Errors
///
/// Returns stable authentication, remote, protocol, I/O, or interruption errors.
pub async fn run_stdio(client: RemoteClient) -> Result<(), CliError> {
    let cancellation = CancellationToken::new();
    let signal_cancellation = cancellation.clone();
    let signal = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal_cancellation.cancel();
        }
    });
    let result = run_bridge(
        tokio::io::stdin(),
        tokio::io::stdout(),
        client,
        cancellation,
    )
    .await;
    signal.abort();
    result
}

/// Runs the bridge over injected asynchronous streams for local clients and tests.
///
/// # Errors
///
/// Returns when framing, transport, authentication, output, or cancellation fails.
pub async fn run_bridge<R, W>(
    input: R,
    output: W,
    client: RemoteClient,
    cancellation: CancellationToken,
) -> Result<(), CliError>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (frames, mut frame_receiver) = mpsc::channel::<String>(32);
    let writer = tokio::spawn(async move {
        let mut output = output;
        while let Some(frame) = frame_receiver.recv().await {
            output.write_all(frame.as_bytes()).await?;
            output.write_all(b"\n").await?;
            output.flush().await?;
        }
        Ok::<(), std::io::Error>(())
    });
    let session = Arc::new(Mutex::new(None::<String>));
    let mut sse_task: Option<JoinHandle<()>> = None;
    let mut input = BufReader::new(input);
    let mut line = String::new();

    let loop_result = loop {
        line.clear();
        let read = tokio::select! {
            () = cancellation.cancelled() => break Err(CliError::Interrupted),
            read = input.read_line(&mut line) => read?,
        };
        if read == 0 {
            break Ok(());
        }
        if line.len() > MAX_FRAME_BYTES {
            break Err(CliError::Protocol("stdin frame exceeded 1 MiB".to_owned()));
        }
        let frame = line.trim_end_matches(['\r', '\n']);
        if frame.is_empty() {
            continue;
        }
        validate_json_rpc(frame)?;
        let current_session = session.lock().await.clone();
        let response = tokio::select! {
            () = cancellation.cancelled() => break Err(CliError::Interrupted),
            response = send_mcp(
                client.http(),
                Method::POST,
                client.mcp_url()?,
                client.token()?,
                current_session.as_deref(),
                Some(frame),
            ) => response?,
        };
        let response_session = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let response_frames = response_frames(response).await?;
        for response_frame in response_frames {
            frames
                .send(response_frame)
                .await
                .map_err(|_| CliError::Io("stdout closed".to_owned()))?;
        }
        if current_session.is_none()
            && let Some(response_session) = response_session
        {
            *session.lock().await = Some(response_session.clone());
            sse_task = Some(spawn_sse_forwarder(
                client.clone(),
                response_session,
                frames.clone(),
                cancellation.clone(),
            ));
        }
    };

    cancellation.cancel();
    if let Some(task) = sse_task {
        let _ = task.await;
    }
    if let Some(session_id) = session.lock().await.clone() {
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            send_mcp(
                client.http(),
                Method::DELETE,
                client.mcp_url()?,
                client.token()?,
                Some(&session_id),
                None,
            ),
        )
        .await;
    }
    drop(frames);
    let writer_result = writer
        .await
        .map_err(|_| CliError::Io("stdout writer stopped".to_owned()))?;
    writer_result.map_err(|error| CliError::Io(error.to_string()))?;
    loop_result
}

fn spawn_sse_forwarder(
    client: RemoteClient,
    session_id: String,
    frames: mpsc::Sender<String>,
    cancellation: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut backoff = INITIAL_BACKOFF;
        loop {
            if cancellation.is_cancelled() {
                return;
            }
            let response = tokio::select! {
                () = cancellation.cancelled() => return,
                response = send_mcp(
                    client.http(),
                    Method::GET,
                    match client.mcp_url() {
                        Ok(url) => url,
                        Err(_) => return,
                    },
                    match client.token() {
                        Ok(token) => token,
                        Err(_) => return,
                    },
                    Some(&session_id),
                    None,
                ) => response,
            };
            match response {
                Ok(response) if response.status().is_success() => {
                    backoff = INITIAL_BACKOFF;
                    if forward_sse(response, &frames, &cancellation).await.is_err()
                        && !cancellation.is_cancelled()
                    {
                        eprintln!("word-arena-cli: MCP notification stream disconnected; retrying");
                    }
                }
                Ok(response)
                    if matches!(
                        response.status(),
                        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
                    ) =>
                {
                    return;
                }
                Ok(_) | Err(_) => {
                    eprintln!("word-arena-cli: MCP notification stream unavailable; retrying");
                }
            }
            tokio::select! {
                () = cancellation.cancelled() => return,
                () = tokio::time::sleep(backoff) => {}
            }
            backoff = backoff.saturating_mul(2).min(MAX_BACKOFF);
        }
    })
}

async fn response_frames(response: reqwest::Response) -> Result<Vec<String>, CliError> {
    let status = response.status();
    if status == StatusCode::ACCEPTED || status == StatusCode::NO_CONTENT {
        return Ok(Vec::new());
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = bounded_body(response).await?;
    if !status.is_success() {
        return Err(remote_mcp_error(status, &bytes));
    }
    if content_type.starts_with("text/event-stream") {
        parse_sse_bytes(&bytes)
    } else if bytes.is_empty() {
        Ok(Vec::new())
    } else {
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| CliError::Protocol("MCP server returned invalid JSON".to_owned()))?;
        Ok(vec![compact(&value)?])
    }
}

async fn forward_sse(
    response: reqwest::Response,
    frames: &mpsc::Sender<String>,
    cancellation: &CancellationToken,
) -> Result<(), CliError> {
    let mut stream = response.bytes_stream();
    let mut pending = String::new();
    loop {
        let chunk = tokio::select! {
            () = cancellation.cancelled() => return Ok(()),
            chunk = stream.next() => chunk,
        };
        let Some(chunk) = chunk else {
            return Ok(());
        };
        let chunk = chunk.map_err(|_| {
            CliError::Remote(RemoteError {
                status: None,
                code: None,
                message: "MCP notification stream failed".to_owned(),
            })
        })?;
        let text = std::str::from_utf8(&chunk)
            .map_err(|_| CliError::Protocol("MCP SSE was not UTF-8".to_owned()))?;
        pending.push_str(text);
        if pending.len() > MAX_FRAME_BYTES {
            return Err(CliError::Protocol(
                "MCP SSE frame exceeded 1 MiB".to_owned(),
            ));
        }
        for event in drain_sse_events(&mut pending)? {
            frames
                .send(event)
                .await
                .map_err(|_| CliError::Io("stdout closed".to_owned()))?;
        }
    }
}

fn parse_sse_bytes(bytes: &[u8]) -> Result<Vec<String>, CliError> {
    let mut pending = std::str::from_utf8(bytes)
        .map_err(|_| CliError::Protocol("MCP SSE was not UTF-8".to_owned()))?
        .to_owned();
    let mut events = drain_sse_events(&mut pending)?;
    if !pending.trim().is_empty() {
        pending.push_str("\n\n");
        events.extend(drain_sse_events(&mut pending)?);
    }
    Ok(events)
}

fn drain_sse_events(pending: &mut String) -> Result<Vec<String>, CliError> {
    let mut events = Vec::new();
    while let Some((end, delimiter_len)) = next_event_boundary(pending) {
        let event = pending.drain(..end + delimiter_len).collect::<String>();
        let data = event
            .lines()
            .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
            .collect::<Vec<_>>()
            .join("\n");
        if !data.is_empty() {
            let value: Value = serde_json::from_str(&data)
                .map_err(|_| CliError::Protocol("MCP SSE data was not JSON".to_owned()))?;
            events.push(compact(&value)?);
        }
    }
    Ok(events)
}

fn next_event_boundary(value: &str) -> Option<(usize, usize)> {
    let lf = value.find("\n\n").map(|index| (index, 2));
    let crlf = value.find("\r\n\r\n").map(|index| (index, 4));
    match (lf, crlf) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(boundary), None) | (None, Some(boundary)) => Some(boundary),
        (None, None) => None,
    }
}

fn validate_json_rpc(frame: &str) -> Result<(), CliError> {
    let value: Value = serde_json::from_str(frame)
        .map_err(|_| CliError::Protocol("stdin frame is not valid JSON".to_owned()))?;
    let object = value
        .as_object()
        .ok_or_else(|| CliError::Protocol("stdin frame must be a JSON object".to_owned()))?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || object.get("method").and_then(Value::as_str).is_none()
    {
        return Err(CliError::Protocol(
            "stdin frame must be a JSON-RPC 2.0 request or notification".to_owned(),
        ));
    }
    Ok(())
}

fn compact(value: &Value) -> Result<String, CliError> {
    serde_json::to_string(value)
        .map_err(|_| CliError::Protocol("failed to encode JSON-RPC frame".to_owned()))
}

fn remote_mcp_error(status: StatusCode, bytes: &[u8]) -> CliError {
    let payload = serde_json::from_slice::<Value>(bytes).ok();
    let code = payload
        .as_ref()
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    CliError::Remote(RemoteError {
        status: Some(status.as_u16()),
        code,
        message: "MCP server rejected the request".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{parse_sse_bytes, validate_json_rpc};

    #[test]
    fn stdio_and_sse_framing_accept_only_complete_json_rpc_messages() {
        assert!(validate_json_rpc(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#).is_ok());
        assert!(validate_json_rpc("[]").is_err());
        assert!(validate_json_rpc(r#"{"jsonrpc":"2.0","id":1}"#).is_err());
        assert_eq!(
            parse_sse_bytes(
                b"event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\r\n\r\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/test\"}\n\n"
            )
            .unwrap(),
            [
                json!({"jsonrpc":"2.0","id":1,"result":{}}).to_string(),
                json!({"jsonrpc":"2.0","method":"notifications/test"}).to_string()
            ]
        );
    }
}
