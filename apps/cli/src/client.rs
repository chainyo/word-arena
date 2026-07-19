use futures_util::StreamExt;
use reqwest::{Method, Response, StatusCode, header};
use serde_json::{Value, json};

use crate::{
    config::ResolvedConfig,
    error::{CliError, RemoteError},
};

const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct RemoteClient {
    http: reqwest::Client,
    config: ResolvedConfig,
}

impl RemoteClient {
    /// Builds a bounded HTTP client from fully resolved redacted configuration.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the HTTP client cannot initialize.
    pub fn new(config: ResolvedConfig) -> Result<Self, CliError> {
        let http = reqwest::Client::builder()
            .timeout(config.timeout())
            .connect_timeout(config.timeout())
            .user_agent(concat!("word-arena-cli/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| CliError::Config("failed to initialize HTTP client".to_owned()))?;
        Ok(Self { http, config })
    }

    /// Reads server health without sending a capability.
    ///
    /// # Errors
    ///
    /// Returns remote, response-limit, or JSON protocol errors.
    pub async fn health(&self) -> Result<Value, CliError> {
        let url = self.config.endpoint("health")?;
        self.send_json(self.http.get(url)).await
    }

    /// Confirms the configured credential by returning its server-bound seat.
    ///
    /// # Errors
    ///
    /// Returns configuration, authentication, remote, or response protocol errors.
    pub async fn authenticate(&self) -> Result<Value, CliError> {
        let observed = self.observe().await?;
        let seat = observed
            .pointer("/data/game/seat")
            .and_then(Value::as_str)
            .ok_or_else(|| CliError::Protocol("seat observation omitted its seat".to_owned()))?;
        Ok(json!({
            "schema_version":1,
            "authenticated":true,
            "game_id":self.config.game_id()?,
            "seat":seat
        }))
    }

    /// Returns exactly the configured seat's REST projection.
    ///
    /// # Errors
    ///
    /// Returns configuration, authentication, remote, or response protocol errors.
    pub async fn observe(&self) -> Result<Value, CliError> {
        let game_id = self.config.game_id()?;
        let url = self
            .config
            .endpoint(&format!("api/v1/games/{game_id}/seat"))?;
        self.send_json(self.authorized(self.http.get(url))?).await
    }

    /// Sends one caller-supplied authoritative action to the bound game.
    ///
    /// # Errors
    ///
    /// Returns configuration, authentication, remote, or response protocol errors.
    pub async fn action(
        &self,
        expected_version: u64,
        turn_id: u64,
        idempotency_key: String,
        action: Value,
    ) -> Result<Value, CliError> {
        let game_id = self.config.game_id()?;
        let url = self
            .config
            .endpoint(&format!("api/v1/games/{game_id}/actions"))?;
        let request = self.authorized(self.http.post(url))?.json(&json!({
            "expected_version":expected_version,
            "turn_number":turn_id,
            "idempotency_key":idempotency_key,
            "action":action
        }));
        self.send_json(request).await
    }

    /// Exports public history plus only the configured seat's private history.
    ///
    /// # Errors
    ///
    /// Returns observation errors or a protocol error for an incomplete projection.
    pub async fn replay_export(&self) -> Result<Value, CliError> {
        let observed = self.observe().await?;
        let game = observed
            .pointer("/data/game")
            .ok_or_else(|| CliError::Protocol("seat observation omitted game data".to_owned()))?;
        let seat = game
            .get("seat")
            .cloned()
            .ok_or_else(|| CliError::Protocol("seat observation omitted seat".to_owned()))?;
        let public = game.get("public").cloned().ok_or_else(|| {
            CliError::Protocol("seat observation omitted public history".to_owned())
        })?;
        let private_events = game.get("private_events").cloned().ok_or_else(|| {
            CliError::Protocol("seat observation omitted private history".to_owned())
        })?;
        Ok(json!({
            "schema_version":1,
            "kind":"seat_replay_export",
            "game_id":self.config.game_id()?,
            "seat":seat,
            "public":public,
            "private_events":private_events
        }))
    }

    #[must_use]
    pub const fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// Resolves the configured game-scoped MCP endpoint.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when game identity or URL resolution fails.
    pub fn mcp_url(&self) -> Result<reqwest::Url, CliError> {
        self.config
            .endpoint(&format!("api/v1/games/{}/mcp", self.config.game_id()?))
    }

    /// Returns the required capability only for constructing an authorization header.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when no token was configured.
    pub fn token(&self) -> Result<&str, CliError> {
        self.config.token()
    }

    fn authorized(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, CliError> {
        Ok(request.bearer_auth(self.config.token()?))
    }

    async fn send_json(&self, request: reqwest::RequestBuilder) -> Result<Value, CliError> {
        let response = request
            .send()
            .await
            .map_err(|_| remote_transport("server is unavailable"))?;
        let status = response.status();
        let bytes = bounded_body(response).await?;
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(CliError::Authentication);
        }
        if !status.is_success() {
            return Err(remote_status(status, &bytes));
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| CliError::Protocol("server returned invalid JSON".to_owned()))
    }
}

/// Collects one response while enforcing the CLI-wide response byte limit.
///
/// # Errors
///
/// Returns a remote stream or response-limit protocol error.
pub async fn bounded_body(response: Response) -> Result<Vec<u8>, CliError> {
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| remote_transport("response stream failed"))?;
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(CliError::Protocol("response exceeded 4 MiB".to_owned()));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub fn mcp_headers(
    request: reqwest::RequestBuilder,
    token: &str,
    session_id: Option<&str>,
) -> reqwest::RequestBuilder {
    let mut request = request
        .bearer_auth(token)
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-11-25");
    if let Some(session_id) = session_id {
        request = request.header("mcp-session-id", session_id);
    }
    request
}

/// Sends one authenticated MCP HTTP request without logging bearer material.
///
/// # Errors
///
/// Returns authentication or sanitized transport errors.
pub async fn send_mcp(
    client: &reqwest::Client,
    method: Method,
    url: reqwest::Url,
    token: &str,
    session_id: Option<&str>,
    body: Option<&str>,
) -> Result<Response, CliError> {
    let mut request = mcp_headers(client.request(method, url), token, session_id);
    if let Some(body) = body {
        request = request
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.to_owned());
    }
    let response = request
        .send()
        .await
        .map_err(|_| remote_transport("MCP server is unavailable"))?;
    if response.status() == StatusCode::UNAUTHORIZED || response.status() == StatusCode::FORBIDDEN {
        return Err(CliError::Authentication);
    }
    Ok(response)
}

fn remote_status(status: StatusCode, bytes: &[u8]) -> CliError {
    let payload = serde_json::from_slice::<Value>(bytes).ok();
    let code = payload
        .as_ref()
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    CliError::Remote(RemoteError {
        status: Some(status.as_u16()),
        code,
        message: "server rejected the request".to_owned(),
    })
}

fn remote_transport(message: &str) -> CliError {
    CliError::Remote(RemoteError {
        status: None,
        code: None,
        message: message.to_owned(),
    })
}
