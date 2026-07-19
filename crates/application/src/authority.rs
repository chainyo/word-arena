use word_arena_engine::Seat;

use crate::GameId;

/// Application-issued seat binding used before transport capabilities exist.
///
/// It is not serializable and has no public constructor. APP-004 replaces this
/// trusted in-process binding with authenticated credential variants.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeatAuthority {
    pub(crate) game_id: GameId,
    pub(crate) seat: Seat,
}

impl SeatAuthority {
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

/// Trusted human-only spectator binding with no seat representation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HumanSpectatorAuthority {
    pub(crate) game_id: GameId,
}

impl HumanSpectatorAuthority {
    /// Bound game identifier.
    #[must_use]
    pub const fn game_id(&self) -> &GameId {
        &self.game_id
    }
}

/// Trusted administrator binding for authoritative persistence views.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdministratorAuthority {
    pub(crate) game_id: GameId,
}

impl AdministratorAuthority {
    /// Bound game identifier.
    #[must_use]
    pub const fn game_id(&self) -> &GameId {
        &self.game_id
    }
}

/// Initial in-process authority bindings returned only to the game creator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatedGameAccess {
    /// Seat-one binding.
    pub seat_one: SeatAuthority,
    /// Seat-two binding.
    pub seat_two: SeatAuthority,
    /// Human-only full-rack spectator binding.
    pub human_spectator: HumanSpectatorAuthority,
    /// Trusted administrator binding.
    pub administrator: AdministratorAuthority,
}

impl CreatedGameAccess {
    pub(crate) fn new(game_id: &GameId) -> Self {
        Self {
            seat_one: SeatAuthority {
                game_id: game_id.clone(),
                seat: Seat::One,
            },
            seat_two: SeatAuthority {
                game_id: game_id.clone(),
                seat: Seat::Two,
            },
            human_spectator: HumanSpectatorAuthority {
                game_id: game_id.clone(),
            },
            administrator: AdministratorAuthority {
                game_id: game_id.clone(),
            },
        }
    }
}
