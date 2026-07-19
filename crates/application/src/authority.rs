use word_arena_engine::Seat;

use crate::{
    AdministratorGameQuery, GameId, HumanSpectatorGameQuery, PublicGameQuery, SeatGameQuery,
};

mod private {
    pub trait Sealed {}
}

/// Sealed compile-time mapping from a credential to its one allowed query.
///
/// External crates can use this bound but cannot add role mappings.
pub trait Authorizes<Query>: private::Sealed {}

/// Public-view credential bound to exactly one game.
///
/// Credentials are intentionally not serializable and their fields are
/// private. Transport adapters authenticate opaque capabilities in APP-005 and
/// then map them to these application-only types.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicViewerCredential {
    game_id: GameId,
}

impl PublicViewerCredential {
    pub(crate) fn new(game_id: &GameId) -> Self {
        Self {
            game_id: game_id.clone(),
        }
    }

    /// Bound game identifier.
    #[must_use]
    pub const fn game_id(&self) -> &GameId {
        &self.game_id
    }
}

impl private::Sealed for PublicViewerCredential {}
impl Authorizes<PublicGameQuery> for PublicViewerCredential {}

/// Competitive credential bound to exactly one game and one seat.
///
/// A competitive credential is accepted by seat reads and actions only. Its
/// type cannot be passed to the human-spectator or administrator use cases.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompetitiveSeatCredential {
    game_id: GameId,
    seat: Seat,
}

impl CompetitiveSeatCredential {
    pub(crate) fn new(game_id: &GameId, seat: Seat) -> Self {
        Self {
            game_id: game_id.clone(),
            seat,
        }
    }

    /// Bound game identifier.
    #[must_use]
    pub const fn game_id(&self) -> &GameId {
        &self.game_id
    }

    /// Bound competitive seat.
    #[must_use]
    pub const fn seat(&self) -> Seat {
        self.seat
    }
}

impl private::Sealed for CompetitiveSeatCredential {}
impl Authorizes<SeatGameQuery> for CompetitiveSeatCredential {}

/// Human-operator spectator credential with access to both current racks.
///
/// Only [`crate::ApplicationRuntime`] can issue this type. Agent integrations
/// receive [`CompetitiveGameCredentials`], which cannot represent it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HumanSpectatorCredential {
    game_id: GameId,
}

impl HumanSpectatorCredential {
    pub(crate) fn new(game_id: &GameId) -> Self {
        Self {
            game_id: game_id.clone(),
        }
    }

    /// Bound game identifier.
    #[must_use]
    pub const fn game_id(&self) -> &GameId {
        &self.game_id
    }
}

impl private::Sealed for HumanSpectatorCredential {}
impl Authorizes<HumanSpectatorGameQuery> for HumanSpectatorCredential {}

/// Trusted operator credential for the authoritative administrator view.
///
/// Only [`crate::ApplicationRuntime`] can issue this type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdministratorCredential {
    game_id: GameId,
}

impl AdministratorCredential {
    pub(crate) fn new(game_id: &GameId) -> Self {
        Self {
            game_id: game_id.clone(),
        }
    }

    /// Bound game identifier.
    #[must_use]
    pub const fn game_id(&self) -> &GameId {
        &self.game_id
    }
}

impl private::Sealed for AdministratorCredential {}
impl Authorizes<AdministratorGameQuery> for AdministratorCredential {}

/// The complete credential shape safe to hand to one competitive agent.
///
/// It deliberately has no spectator or administrator variant and is not
/// serializable. Future agent-run configuration can identify capabilities, but
/// cannot ask the application layer to mint a privileged credential.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompetitiveGameCredentials {
    /// Public-view credential for the same game.
    pub public: PublicViewerCredential,
    /// Exactly one private seat credential.
    pub seat: CompetitiveSeatCredential,
}

/// Initial non-operator credentials returned after game creation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatedGameAccess {
    /// Public observer binding.
    pub public: PublicViewerCredential,
    /// Seat-one binding.
    pub seat_one: CompetitiveSeatCredential,
    /// Seat-two binding.
    pub seat_two: CompetitiveSeatCredential,
}

impl CreatedGameAccess {
    pub(crate) fn new(game_id: &GameId) -> Self {
        Self {
            public: PublicViewerCredential::new(game_id),
            seat_one: CompetitiveSeatCredential::new(game_id, Seat::One),
            seat_two: CompetitiveSeatCredential::new(game_id, Seat::Two),
        }
    }

    /// Selects the bounded credential shape safe for one competitive process.
    #[must_use]
    pub fn competitive(&self, seat: Seat) -> CompetitiveGameCredentials {
        CompetitiveGameCredentials {
            public: self.public.clone(),
            seat: match seat {
                Seat::One => self.seat_one.clone(),
                Seat::Two => self.seat_two.clone(),
            },
        }
    }
}
