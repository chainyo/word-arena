use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::Arc,
};

use axum::http::request::Parts;
use rmcp::{
    ErrorData, Peer, RoleServer,
    model::{
        ListResourceTemplatesResult, ListResourcesResult, ReadResourceRequestParams,
        ReadResourceResult, Resource, ResourceContents, ResourceTemplate,
        ResourceUpdatedNotificationParam, SubscribeRequestParams, UnsubscribeRequestParams,
    },
    service::RequestContext,
};
use serde::Serialize;
use serde_json::json;
use tokio::sync::RwLock;
use word_arena_application::{
    ApplicationRuntime, CompetitiveSeatCredential, SeatGameQuery, SeatGameView,
};
use word_arena_engine::{Ruleset, RulesetId};

use crate::mcp_tools::McpRequestAuthority;

const RESOURCE_SCHEMA_VERSION: u16 = 1;
const RESOURCE_MIME_TYPE: &str = "application/json";
const SESSION_HEADER: &str = "mcp-session-id";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResourceKind {
    PublicGame,
    PrivateSeat,
    EventHistory,
    Ruleset,
    LexiconManifest,
}

impl ResourceKind {
    const ALL: [Self; 5] = [
        Self::PublicGame,
        Self::PrivateSeat,
        Self::EventHistory,
        Self::Ruleset,
        Self::LexiconManifest,
    ];

    const fn slug(self) -> &'static str {
        match self {
            Self::PublicGame => "public",
            Self::PrivateSeat => "seat",
            Self::EventHistory => "history",
            Self::Ruleset => "ruleset",
            Self::LexiconManifest => "lexicon-manifest",
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::PublicGame => "public-game",
            Self::PrivateSeat => "private-seat",
            Self::EventHistory => "event-history",
            Self::Ruleset => "game-ruleset",
            Self::LexiconManifest => "active-lexicon-manifest",
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Self::PublicGame => "Public game",
            Self::PrivateSeat => "Private seat",
            Self::EventHistory => "Event history",
            Self::Ruleset => "Game ruleset",
            Self::LexiconManifest => "Active lexicon manifest",
        }
    }

    const fn description(self) -> &'static str {
        match self {
            Self::PublicGame => "Current public board, scores, lifecycle, and public events.",
            Self::PrivateSeat => {
                "Authenticated seat projection with only its rack and past private draws."
            }
            Self::EventHistory => {
                "Public events plus only the authenticated seat's private transitions."
            }
            Self::Ruleset => "Exact immutable board, scoring, tile, and pack configuration.",
            Self::LexiconManifest => {
                "Verified manifest for the exact offline lexicon pack active in this game."
            }
        }
    }

    const fn subscribable(self) -> bool {
        matches!(
            self,
            Self::PublicGame | Self::PrivateSeat | Self::EventHistory
        )
    }
}

#[derive(Clone)]
struct SessionSubscription {
    game_id: word_arena_application::GameId,
    peer: Peer<RoleServer>,
    uris: HashSet<String>,
}

/// Shared session subscription registry containing no capability secrets.
#[derive(Clone, Default)]
pub(crate) struct McpResourceSubscriptions {
    sessions: Arc<RwLock<HashMap<String, SessionSubscription>>>,
}

impl fmt::Debug for McpResourceSubscriptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpResourceSubscriptions")
            .field("sessions", &"SESSION-BOUND")
            .finish()
    }
}

impl McpResourceSubscriptions {
    async fn subscribe(
        &self,
        session_id: String,
        credential: &CompetitiveSeatCredential,
        uri: String,
        peer: Peer<RoleServer>,
    ) {
        let mut sessions = self.sessions.write().await;
        let subscription = sessions
            .entry(session_id)
            .or_insert_with(|| SessionSubscription {
                game_id: credential.game_id().clone(),
                peer,
                uris: HashSet::new(),
            });
        subscription.uris.insert(uri);
    }

    async fn unsubscribe(&self, session_id: &str, uri: &str) {
        let mut sessions = self.sessions.write().await;
        if let Some(subscription) = sessions.get_mut(session_id) {
            subscription.uris.remove(uri);
            if subscription.uris.is_empty() {
                sessions.remove(session_id);
            }
        }
    }

    pub(crate) async fn remove_session(&self, session_id: &str) {
        self.sessions.write().await.remove(session_id);
    }

    pub(crate) async fn retain_sessions(&self, live: &HashSet<String>) {
        self.sessions
            .write()
            .await
            .retain(|session_id, _| live.contains(session_id));
    }

    pub(crate) async fn notify_game_updated(&self, game_id: &word_arena_application::GameId) {
        let targets = self
            .sessions
            .read()
            .await
            .values()
            .filter(|subscription| &subscription.game_id == game_id)
            .map(|subscription| (subscription.peer.clone(), subscription.uris.clone()))
            .collect::<Vec<_>>();
        for (peer, uris) in targets {
            for uri in uris {
                if let Err(error) = peer
                    .notify_resource_updated(ResourceUpdatedNotificationParam::new(uri))
                    .await
                {
                    tracing::debug!(%error, %game_id, "MCP resource update delivery failed");
                }
            }
        }
    }
}

pub(crate) fn list_resources(
    context: &RequestContext<RoleServer>,
) -> Result<ListResourcesResult, ErrorData> {
    let credential = request_credential(context)?;
    let resources = ResourceKind::ALL
        .map(|kind| {
            Resource::new(resource_uri(credential.game_id(), kind), kind.name())
                .with_title(kind.title())
                .with_description(kind.description())
                .with_mime_type(RESOURCE_MIME_TYPE)
        })
        .into_iter()
        .collect();
    Ok(ListResourcesResult::with_all_items(resources))
}

pub(crate) fn list_resource_templates(
    context: &RequestContext<RoleServer>,
) -> Result<ListResourceTemplatesResult, ErrorData> {
    request_credential(context)?;
    let templates = ResourceKind::ALL
        .map(|kind| {
            ResourceTemplate::new(resource_template(kind), kind.name())
                .with_title(kind.title())
                .with_description(kind.description())
                .with_mime_type(RESOURCE_MIME_TYPE)
        })
        .into_iter()
        .collect();
    Ok(ListResourceTemplatesResult::with_all_items(templates))
}

pub(crate) async fn read_resource(
    runtime: &ApplicationRuntime,
    request: ReadResourceRequestParams,
    context: &RequestContext<RoleServer>,
) -> Result<ReadResourceResult, ErrorData> {
    let credential = request_credential(context)?.clone();
    let kind = authorize_uri(&request.uri, &credential)?;
    let view = runtime
        .service()
        .seat_game(
            &credential,
            SeatGameQuery {
                game_id: credential.game_id().clone(),
            },
        )
        .await
        .map_err(|error| resource_application_error(&error))?;
    let text = encode_resource(runtime, kind, &view)?;
    Ok(ReadResourceResult::new(vec![
        ResourceContents::text(text, request.uri).with_mime_type(RESOURCE_MIME_TYPE),
    ]))
}

pub(crate) async fn subscribe(
    subscriptions: &McpResourceSubscriptions,
    request: SubscribeRequestParams,
    context: &RequestContext<RoleServer>,
) -> Result<(), ErrorData> {
    let credential = request_credential(context)?.clone();
    let kind = authorize_uri(&request.uri, &credential)?;
    if !kind.subscribable() {
        return Err(ErrorData::invalid_params(
            "resource_not_subscribable: ruleset and lexicon manifest resources are immutable",
            None,
        ));
    }
    let session_id = request_session_id(context)?;
    subscriptions
        .subscribe(session_id, &credential, request.uri, context.peer.clone())
        .await;
    Ok(())
}

pub(crate) async fn unsubscribe(
    subscriptions: &McpResourceSubscriptions,
    request: UnsubscribeRequestParams,
    context: &RequestContext<RoleServer>,
) -> Result<(), ErrorData> {
    let credential = request_credential(context)?;
    let kind = authorize_uri(&request.uri, credential)?;
    if !kind.subscribable() {
        return Err(ErrorData::invalid_params(
            "resource_not_subscribable: ruleset and lexicon manifest resources are immutable",
            None,
        ));
    }
    let session_id = request_session_id(context)?;
    subscriptions.unsubscribe(&session_id, &request.uri).await;
    Ok(())
}

fn encode_resource(
    runtime: &ApplicationRuntime,
    kind: ResourceKind,
    view: &SeatGameView,
) -> Result<String, ErrorData> {
    let state = &view.game.public.state;
    let data = match kind {
        ResourceKind::PublicGame => serde_json::to_value(&view.game.public),
        ResourceKind::PrivateSeat => serde_json::to_value(&view.game),
        ResourceKind::EventHistory => serde_json::to_value(json!({
            "public_events": &view.game.public.events,
            "private_events": &view.game.private_events,
        })),
        ResourceKind::Ruleset => {
            let ruleset = match state.ruleset_id {
                RulesetId::EnglishV1 => Ruleset::english_v1(),
                RulesetId::FrenchV1 => Ruleset::french_v1(),
            };
            serde_json::to_value(ruleset)
        }
        ResourceKind::LexiconManifest => {
            let manifest = runtime
                .service()
                .lexicon_manifest(&state.lexicon)
                .ok_or_else(|| {
                    ErrorData::invalid_params(
                        "lexicon_manifest_unavailable: exact installed pack metadata is unavailable",
                        None,
                    )
                })?;
            serde_json::to_value(manifest)
        }
    }
    .map_err(|error| resource_serialization_error(&error))?;
    serde_json::to_string(&ResourceEnvelope {
        schema_version: RESOURCE_SCHEMA_VERSION,
        resource: kind.name(),
        game_id: &state.game_id,
        version: state.version,
        data,
    })
    .map_err(|error| resource_serialization_error(&error))
}

#[derive(Serialize)]
struct ResourceEnvelope<'a> {
    schema_version: u16,
    resource: &'static str,
    game_id: &'a str,
    version: u64,
    data: serde_json::Value,
}

fn request_credential(
    context: &RequestContext<RoleServer>,
) -> Result<&CompetitiveSeatCredential, ErrorData> {
    request_parts(context)?
        .extensions
        .get::<McpRequestAuthority>()
        .map(McpRequestAuthority::credential)
        .ok_or_else(|| ErrorData::invalid_params("missing authenticated seat authority", None))
}

fn request_session_id(context: &RequestContext<RoleServer>) -> Result<String, ErrorData> {
    request_parts(context)?
        .headers
        .get(SESSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .ok_or_else(|| ErrorData::invalid_params("missing authenticated MCP session ID", None))
}

fn request_parts(context: &RequestContext<RoleServer>) -> Result<&Parts, ErrorData> {
    context
        .extensions
        .get::<Parts>()
        .ok_or_else(|| ErrorData::internal_error("missing HTTP request context", None))
}

fn authorize_uri(
    uri: &str,
    credential: &CompetitiveSeatCredential,
) -> Result<ResourceKind, ErrorData> {
    let (game_id, kind) = parse_resource_uri(uri)?;
    if game_id != credential.game_id().as_str() {
        return Err(ErrorData::invalid_params(
            "resource_uri_forbidden: URI is not bound to this authenticated game",
            None,
        ));
    }
    Ok(kind)
}

fn parse_resource_uri(uri: &str) -> Result<(&str, ResourceKind), ErrorData> {
    let remainder = uri
        .strip_prefix("word-arena://games/")
        .ok_or_else(invalid_resource_uri)?;
    let mut segments = remainder.split('/');
    let game_id = segments.next().filter(|value| !value.is_empty());
    let kind = match segments.next() {
        Some("public") => Some(ResourceKind::PublicGame),
        Some("seat") => Some(ResourceKind::PrivateSeat),
        Some("history") => Some(ResourceKind::EventHistory),
        Some("ruleset") => Some(ResourceKind::Ruleset),
        Some("lexicon-manifest") => Some(ResourceKind::LexiconManifest),
        _ => None,
    };
    match (game_id, kind, segments.next()) {
        (Some(game_id), Some(kind), None) => Ok((game_id, kind)),
        _ => Err(invalid_resource_uri()),
    }
}

fn resource_uri(game_id: &word_arena_application::GameId, kind: ResourceKind) -> String {
    format!("word-arena://games/{game_id}/{}", kind.slug())
}

fn resource_template(kind: ResourceKind) -> String {
    format!("word-arena://games/{{game_id}}/{}", kind.slug())
}

fn invalid_resource_uri() -> ErrorData {
    ErrorData::invalid_params(
        "invalid_resource_uri: expected word-arena://games/{game_id}/{public|seat|history|ruleset|lexicon-manifest}",
        None,
    )
}

fn resource_application_error(error: &word_arena_application::ApplicationError) -> ErrorData {
    ErrorData::invalid_params(format!("resource_unavailable: {error}"), None)
}

fn resource_serialization_error(error: &serde_json::Error) -> ErrorData {
    ErrorData::internal_error(format!("failed to encode MCP resource: {error}"), None)
}
