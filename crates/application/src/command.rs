use std::fmt;

use serde::{Deserialize, Serialize};
use word_arena_engine::{
    AdministratorProjection, GameEvent, HumanSpectatorProjection, Language, Move, PublicProjection,
    SeatProjection, Turn,
};

use crate::CreatedGameAccess;

const MAX_GAME_ID_BYTES: usize = 128;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 256;

/// Stable application-level game identifier.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct GameId(String);

impl GameId {
    /// Validates one externally recordable game ID.
    ///
    /// # Errors
    ///
    /// Rejects empty, oversized, whitespace, control, or non-ASCII values.
    pub fn new(value: impl Into<String>) -> Result<Self, crate::ApplicationError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_GAME_ID_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_graphic() && !byte.is_ascii_whitespace())
        {
            return Err(crate::ApplicationError::InvalidGameId);
        }
        Ok(Self(value))
    }

    /// String form used by repositories and engine state.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for GameId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for GameId {
    type Error = crate::ApplicationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<GameId> for String {
    fn from(value: GameId) -> Self {
        value.0
    }
}

/// Opaque client retry identity, persisted atomically in APP-007.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// Validates a bounded printable key without interpreting its format.
    ///
    /// # Errors
    ///
    /// Rejects empty, oversized, whitespace, control, or non-ASCII values.
    pub fn new(value: impl Into<String>) -> Result<Self, crate::ApplicationError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_IDEMPOTENCY_KEY_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_graphic() && !byte.is_ascii_whitespace())
        {
            return Err(crate::ApplicationError::InvalidIdempotencyKey);
        }
        Ok(Self(value))
    }

    /// Opaque string form for later persistence adapters.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for IdempotencyKey {
    type Error = crate::ApplicationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<IdempotencyKey> for String {
    fn from(value: IdempotencyKey) -> Self {
        value.0
    }
}

/// UTC Unix timestamp in milliseconds supplied by an application clock.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct UnixMillis(pub i64);

/// Fully identified game creation request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateGameCommand {
    /// ID generated at the application boundary.
    pub game_id: GameId,
    /// Immutable language/ruleset selection.
    pub language: Language,
    /// Retry identity reserved before creation.
    pub idempotency_key: IdempotencyKey,
}

/// One authority-bound engine mutation request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GameActionCommand {
    /// Game the command targets.
    pub game_id: GameId,
    /// Optimistic engine/repository version.
    pub expected_version: u64,
    /// Explicit active turn identity.
    pub turn: Turn,
    /// Retry identity reserved for APP-007 persistence.
    pub idempotency_key: IdempotencyKey,
    /// Typed engine action.
    pub action: Move,
}

/// Role-neutral public game query.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicGameQuery {
    /// Requested game.
    pub game_id: GameId,
}

/// Competitive-seat query. The seat comes from [`crate::SeatAuthority`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeatGameQuery {
    /// Requested game.
    pub game_id: GameId,
}

/// Human-only spectator query with no role/seat input.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HumanSpectatorGameQuery {
    /// Requested game.
    pub game_id: GameId,
}

/// Administrator query with no caller-selectable role input.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdministratorGameQuery {
    /// Requested game.
    pub game_id: GameId,
}

/// Newly persisted game and its trusted initial bindings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatedGame {
    /// Stable generated ID.
    pub game_id: GameId,
    /// Injected creation time.
    pub created_at: UnixMillis,
    /// Safe initial public state.
    pub public: PublicProjection,
    /// In-process bindings supplied only to the trusted creator.
    pub access: CreatedGameAccess,
}

/// Public query result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicGameView {
    /// Injected observation time.
    pub observed_at: UnixMillis,
    /// Public-only projection.
    pub game: PublicProjection,
}

/// One-seat private query result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeatGameView {
    /// Injected observation time.
    pub observed_at: UnixMillis,
    /// Projection bound to the authority's seat.
    pub game: SeatProjection,
}

/// Trusted-human spectator query result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HumanSpectatorGameView {
    /// Injected observation time.
    pub observed_at: UnixMillis,
    /// Both-rack projection with no future bag.
    pub game: HumanSpectatorProjection,
}

/// Administrator query result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdministratorGameView {
    /// Injected observation time.
    pub observed_at: UnixMillis,
    /// Complete authoritative checkpoint.
    pub game: AdministratorProjection,
}

/// Accepted action event and updated acting-seat projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GameActionResult {
    /// Injected commit time.
    pub committed_at: UnixMillis,
    /// Authoritative public event.
    pub event: GameEvent,
    /// Updated projection for the authenticated acting seat.
    pub game: SeatProjection,
}
