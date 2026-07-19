use std::{fmt, sync::Arc};

use axum::http::request::Parts;
use rmcp::{
    Json, RoleServer, ServerHandler,
    handler::server::{
        router::tool::{ToolRoute, ToolRouter},
        tool::{ToolCallContext, parse_json_object, schema_for_input},
    },
    model::{
        CallToolRequestParams, CallToolResult, ContentBlock, Implementation,
        ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
        ProtocolVersion, ReadResourceRequestParams, ReadResourceResult, ServerCapabilities,
        ServerInfo, SubscribeRequestParams, Tool, ToolAnnotations, UnsubscribeRequestParams,
    },
    service::RequestContext,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use word_arena_application::{
    ActionRejection, ApplicationError, ApplicationRuntime, CompetitiveSeatCredential,
    GameActionCommand, GameActionResult, IdempotencyKey, SeatGameQuery, SeatGameView,
};
use word_arena_engine::{Coordinate, Move, Placement, Ruleset, RulesetId, Tile, TileId, Turn};

use crate::mcp_resources::{
    McpResourceSubscriptions, list_resource_templates, list_resources, read_resource, subscribe,
    unsubscribe,
};

/// Stable schema shared by the first competitive MCP tool generation.
pub(crate) const MCP_TOOL_SCHEMA_VERSION: u16 = 1;

/// Unforgeable authority injected by the authenticated HTTP gateway.
#[derive(Clone, Debug)]
pub(crate) struct McpRequestAuthority {
    credential: CompetitiveSeatCredential,
}

impl McpRequestAuthority {
    pub(crate) const fn new(credential: CompetitiveSeatCredential) -> Self {
        Self { credential }
    }

    pub(crate) const fn credential(&self) -> &CompetitiveSeatCredential {
        &self.credential
    }
}

#[derive(Clone)]
pub(crate) struct WordArenaMcp {
    runtime: Arc<ApplicationRuntime>,
    subscriptions: McpResourceSubscriptions,
    tool_router: ToolRouter<Self>,
}

impl fmt::Debug for WordArenaMcp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WordArenaMcp")
            .field("authority", &"REQUEST-BOUND")
            .finish_non_exhaustive()
    }
}

impl WordArenaMcp {
    pub(crate) fn new(
        runtime: Arc<ApplicationRuntime>,
        subscriptions: McpResourceSubscriptions,
    ) -> Self {
        Self {
            runtime,
            subscriptions,
            tool_router: Self::competitive_tool_router(),
        }
    }

    fn competitive_tool_router() -> ToolRouter<Self> {
        let mut router = ToolRouter::new();
        router.add_route(ToolRoute::new_dyn(
            tool_definition::<ReadInput, ObserveGameOutput>(
                "observe_game",
                "Observe game",
                "Observe the authenticated seat's current board, public history, own rack, and own draws.",
                true,
                false,
            ),
            |mut context| {
                Box::pin(async move {
                    let input = parse_tool_input::<ReadInput>(&mut context)?;
                    let parts = request_parts(&context)?;
                    into_tool_result(context.service.observe_game(input, &parts).await)
                })
            },
        ));
        router.add_route(ToolRoute::new_dyn(
            tool_definition::<ReadInput, GetRulesetOutput>(
                "get_ruleset",
                "Get ruleset",
                "Return the exact immutable board, scoring, tile, and lexicon configuration for this game.",
                true,
                false,
            ),
            |mut context| {
                Box::pin(async move {
                    let input = parse_tool_input::<ReadInput>(&mut context)?;
                    let parts = request_parts(&context)?;
                    into_tool_result(context.service.get_ruleset(input, &parts).await)
                })
            },
        ));
        router.add_route(ToolRoute::new_dyn(
            tool_definition::<PlayTilesInput, ActionOutput>(
                "play_tiles",
                "Play tiles",
                "Place owned rack tiles on empty board squares and commit all formed words atomically.",
                false,
                false,
            ),
            |mut context| {
                Box::pin(async move {
                    let input = parse_tool_input::<PlayTilesInput>(&mut context)?;
                    let parts = request_parts(&context)?;
                    into_tool_result(context.service.play_tiles(input, &parts).await)
                })
            },
        ));
        router.add_route(ToolRoute::new_dyn(
            tool_definition::<ExchangeTilesInput, ActionOutput>(
                "exchange_tiles",
                "Exchange tiles",
                "Exchange selected owned tiles when the immutable ruleset and bag permit it.",
                false,
                false,
            ),
            |mut context| {
                Box::pin(async move {
                    let input = parse_tool_input::<ExchangeTilesInput>(&mut context)?;
                    let parts = request_parts(&context)?;
                    into_tool_result(context.service.exchange_tiles(input, &parts).await)
                })
            },
        ));
        router.add_route(ToolRoute::new_dyn(
            tool_definition::<MutationInput, ActionOutput>(
                "pass_turn",
                "Pass turn",
                "End the authenticated seat's current turn without scoring or changing its rack.",
                false,
                false,
            ),
            |mut context| {
                Box::pin(async move {
                    let input = parse_tool_input::<MutationInput>(&mut context)?;
                    let parts = request_parts(&context)?;
                    into_tool_result(context.service.pass_turn(input, &parts).await)
                })
            },
        ));
        router.add_route(ToolRoute::new_dyn(
            tool_definition::<MutationInput, ActionOutput>(
                "resign",
                "Resign",
                "Concede immediately; this permanently finishes the game.",
                false,
                true,
            ),
            |mut context| {
                Box::pin(async move {
                    let input = parse_tool_input::<MutationInput>(&mut context)?;
                    let parts = request_parts(&context)?;
                    into_tool_result(context.service.resign(input, &parts).await)
                })
            },
        ));
        router
    }

    async fn observe_game(
        &self,
        input: ReadInput,
        parts: &Parts,
    ) -> Result<Json<ObserveGameOutput>, String> {
        validate_schema(input.schema_version)?;
        let credential = authority(parts)?;
        let view = self
            .runtime
            .service()
            .seat_game(
                credential,
                SeatGameQuery {
                    game_id: credential.game_id().clone(),
                },
            )
            .await
            .map_err(tool_error)?;
        observe_output(view)
    }

    async fn get_ruleset(
        &self,
        input: ReadInput,
        parts: &Parts,
    ) -> Result<Json<GetRulesetOutput>, String> {
        validate_schema(input.schema_version)?;
        let credential = authority(parts)?;
        let view = self
            .runtime
            .service()
            .seat_game(
                credential,
                SeatGameQuery {
                    game_id: credential.game_id().clone(),
                },
            )
            .await
            .map_err(tool_error)?;
        let ruleset_id = view.game.public.state.ruleset_id;
        let ruleset = match ruleset_id {
            RulesetId::EnglishV1 => Ruleset::english_v1(),
            RulesetId::FrenchV1 => Ruleset::french_v1(),
        };
        Ok(Json(GetRulesetOutput {
            schema_version: MCP_TOOL_SCHEMA_VERSION,
            summary: format!(
                "{} uses {} with a {}x{} board and rack capacity {}",
                credential.game_id(),
                ruleset.id.as_str(),
                ruleset.game.board.width,
                ruleset.game.board.height,
                ruleset.game.rack_capacity
            ),
            ruleset: serde_json::to_value(ruleset).map_err(|error| serialization_error(&error))?,
        }))
    }

    async fn play_tiles(
        &self,
        input: PlayTilesInput,
        parts: &Parts,
    ) -> Result<Json<ActionOutput>, String> {
        validate_schema(input.mutation.schema_version)?;
        let placements = input
            .placements
            .into_iter()
            .map(|placement| {
                let tile = if placement.is_blank {
                    Tile::blank(placement.letter)
                } else {
                    Tile::letter(placement.letter)
                };
                Placement::new(
                    TileId(placement.tile_id),
                    Coordinate::new(placement.row, placement.column),
                    tile,
                )
            })
            .collect();
        self.act(parts, input.mutation, Move::Place { placements })
            .await
    }

    async fn exchange_tiles(
        &self,
        input: ExchangeTilesInput,
        parts: &Parts,
    ) -> Result<Json<ActionOutput>, String> {
        validate_schema(input.mutation.schema_version)?;
        self.act(
            parts,
            input.mutation,
            Move::Exchange {
                tile_ids: input.tile_ids.into_iter().map(TileId).collect(),
            },
        )
        .await
    }

    async fn pass_turn(
        &self,
        input: MutationInput,
        parts: &Parts,
    ) -> Result<Json<ActionOutput>, String> {
        validate_schema(input.schema_version)?;
        self.act(parts, input, Move::Pass).await
    }

    async fn resign(
        &self,
        input: MutationInput,
        parts: &Parts,
    ) -> Result<Json<ActionOutput>, String> {
        validate_schema(input.schema_version)?;
        self.act(parts, input, Move::Resign).await
    }

    async fn act(
        &self,
        parts: &Parts,
        input: MutationInput,
        action: Move,
    ) -> Result<Json<ActionOutput>, String> {
        let credential = authority(parts)?;
        let game_id = credential.game_id().clone();
        let idempotency_key = IdempotencyKey::new(input.idempotency_key).map_err(tool_error)?;
        let result = self
            .runtime
            .service()
            .act(
                credential,
                GameActionCommand {
                    game_id: credential.game_id().clone(),
                    expected_version: input.expected_version,
                    turn: Turn {
                        number: input.turn_id,
                        seat: credential.seat(),
                    },
                    idempotency_key,
                    action,
                },
            )
            .await
            .map_err(tool_error)?;
        self.subscriptions.notify_game_updated(&game_id).await;
        action_output(result)
    }
}

impl ServerHandler for WordArenaMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_resources()
                .enable_resources_subscribe()
                .enable_tools()
                .build(),
        )
            .with_protocol_version(ProtocolVersion::V_2025_11_25)
            .with_server_info(
                Implementation::new("word-arena", env!("CARGO_PKG_VERSION"))
                    .with_title("Word Arena")
                    .with_description("Authenticated competitive word-tile game server"),
            )
            .with_instructions(
                "Use observe_game before acting. Mutations require the observed version as both expected_version and turn_id, plus a unique idempotency_key. Your authenticated seat is fixed by the MCP session and cannot be selected in tool input.",
            )
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_router
            .call(ToolCallContext::new(self, request, context))
            .await
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        Ok(ListToolsResult::with_all_items(self.tool_router.list_all()))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_router.get(name).cloned()
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, rmcp::ErrorData> {
        list_resources(&context)
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, rmcp::ErrorData> {
        list_resource_templates(&context)
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, rmcp::ErrorData> {
        read_resource(&self.runtime, request, &context).await
    }

    async fn subscribe(
        &self,
        request: SubscribeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), rmcp::ErrorData> {
        subscribe(&self.subscriptions, request, &context).await
    }

    async fn unsubscribe(
        &self,
        request: UnsubscribeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), rmcp::ErrorData> {
        unsubscribe(&self.subscriptions, request, &context).await
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadInput {
    /// Competitive tool schema. Must be 1.
    schema_version: u16,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MutationInput {
    /// Competitive tool schema. Must be 1.
    schema_version: u16,
    /// Public game version returned by `observe_game`.
    expected_version: u64,
    /// Active turn identifier returned by `observe_game`; equals `expected_version` in V1.
    turn_id: u64,
    /// Unique printable retry key. Reuse only for an identical request.
    idempotency_key: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PlayTilesInput {
    #[serde(flatten)]
    mutation: MutationInput,
    /// One or more owned rack tiles in a single row or column.
    placements: Vec<PlacementInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PlacementInput {
    /// Physical tile ID from the authenticated seat's rack.
    tile_id: u16,
    /// Zero-indexed board row.
    row: u8,
    /// Zero-indexed board column.
    column: u8,
    /// Uppercase A-Z board letter; for a nonblank it must match the tile face.
    letter: String,
    /// True only when assigning a physical blank.
    is_blank: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExchangeTilesInput {
    #[serde(flatten)]
    mutation: MutationInput,
    /// One or more physical tile IDs from the authenticated seat's rack.
    tile_ids: Vec<u16>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ObserveGameOutput {
    schema_version: u16,
    summary: String,
    observed_at_unix_ms: i64,
    /// Seat-private projection: public state/history, own rack, and own past draws only.
    game: Value,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetRulesetOutput {
    schema_version: u16,
    summary: String,
    /// Complete immutable ruleset and exact offline lexicon identity.
    ruleset: Value,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ActionOutput {
    schema_version: u16,
    summary: String,
    committed_at_unix_ms: i64,
    /// Authoritative event committed by this action.
    event: Value,
    /// Updated projection for only the authenticated acting seat.
    game: Value,
}

fn authority(parts: &Parts) -> Result<&CompetitiveSeatCredential, String> {
    parts
        .extensions
        .get::<McpRequestAuthority>()
        .map(|authority| &authority.credential)
        .ok_or_else(|| "unauthorized: authenticated seat context is missing".to_owned())
}

fn validate_schema(schema_version: u16) -> Result<(), String> {
    if schema_version == MCP_TOOL_SCHEMA_VERSION {
        Ok(())
    } else {
        let expected = MCP_TOOL_SCHEMA_VERSION;
        Err(format!(
            "unsupported_schema: expected {expected}, received {schema_version}"
        ))
    }
}

fn observe_output(view: SeatGameView) -> Result<Json<ObserveGameOutput>, String> {
    let state = &view.game.public.state;
    Ok(Json(ObserveGameOutput {
        schema_version: MCP_TOOL_SCHEMA_VERSION,
        summary: format!(
            "{} version {}: {:?} to act, scores {}-{}, {} tiles in your rack, {} in bag",
            state.game_id,
            state.version,
            state.current_player,
            state.scores[0].value(),
            state.scores[1].value(),
            view.game.rack.len(),
            state.bag_count
        ),
        observed_at_unix_ms: view.observed_at.0,
        game: serde_json::to_value(view.game).map_err(|error| serialization_error(&error))?,
    }))
}

fn action_output(result: GameActionResult) -> Result<Json<ActionOutput>, String> {
    let state = &result.game.public.state;
    Ok(Json(ActionOutput {
        schema_version: MCP_TOOL_SCHEMA_VERSION,
        summary: format!(
            "action committed at version {}; scores {}-{}; phase {:?}",
            state.version,
            state.scores[0].value(),
            state.scores[1].value(),
            state.phase
        ),
        committed_at_unix_ms: result.committed_at.0,
        event: serde_json::to_value(result.event).map_err(|error| serialization_error(&error))?,
        game: serde_json::to_value(result.game).map_err(|error| serialization_error(&error))?,
    }))
}

fn tool_error(error: ApplicationError) -> String {
    match error {
        ApplicationError::ActionRejected(ActionRejection::VersionConflict)
        | ApplicationError::Repository(word_arena_application::RepositoryError::Conflict) => {
            "version_conflict: call observe_game, then retry with the current expected_version and turn_id".to_owned()
        }
        ApplicationError::ActionRejected(ActionRejection::IllegalAction { message }) => {
            format!("illegal_action: {message}")
        }
        ApplicationError::ActionRejected(ActionRejection::IdempotencyConflict) => {
            "idempotency_conflict: this key was already used for a different request".to_owned()
        }
        ApplicationError::ActionRejected(ActionRejection::DeadlineNotReached) => {
            "action_rejected: the turn deadline has not been reached".to_owned()
        }
        ApplicationError::InvalidIdempotencyKey => {
            "invalid_request: idempotency_key must be 1-256 printable non-whitespace ASCII bytes"
                .to_owned()
        }
        ApplicationError::Repository(word_arena_application::RepositoryError::NotFound) => {
            "not_found: game does not exist".to_owned()
        }
        ApplicationError::Engine(error) => format!("invalid_action: {error}"),
        _ => "request_failed: the game request could not be completed".to_owned(),
    }
}

fn serialization_error(error: &serde_json::Error) -> String {
    format!("internal_error: failed to encode tool result: {error}")
}

fn tool_definition<I, O>(
    name: &'static str,
    title: &'static str,
    description: &'static str,
    read_only: bool,
    destructive: bool,
) -> Tool
where
    I: JsonSchema + 'static,
    O: JsonSchema + 'static,
{
    Tool::new(
        name,
        description,
        schema_for_input::<I>().expect("competitive tool input schema must be an object"),
    )
    .with_title(title)
    .with_output_schema::<O>()
    .with_annotations(
        ToolAnnotations::with_title(title)
            .read_only(read_only)
            .destructive(destructive)
            .idempotent(true)
            .open_world(false),
    )
}

fn parse_tool_input<T>(
    context: &mut ToolCallContext<'_, WordArenaMcp>,
) -> Result<T, rmcp::ErrorData>
where
    T: DeserializeOwned,
{
    parse_json_object(context.arguments.take().unwrap_or_default())
}

fn request_parts(context: &ToolCallContext<'_, WordArenaMcp>) -> Result<Parts, rmcp::ErrorData> {
    context
        .request_context
        .extensions
        .get::<Parts>()
        .cloned()
        .ok_or_else(|| rmcp::ErrorData::internal_error("missing HTTP request context", None))
}

fn into_tool_result<T>(result: Result<Json<T>, String>) -> Result<CallToolResult, rmcp::ErrorData>
where
    T: Serialize,
{
    match result {
        Ok(Json(output)) => serde_json::to_value(output)
            .map(CallToolResult::structured)
            .map_err(|error| {
                rmcp::ErrorData::internal_error(
                    format!("failed to serialize competitive tool output: {error}"),
                    None,
                )
            }),
        Err(message) => Ok(CallToolResult::error(vec![ContentBlock::text(message)])),
    }
}
