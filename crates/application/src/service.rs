use std::sync::Arc;

use word_arena_engine::{Game, Ruleset, RulesetId};

use crate::{
    AdministratorAuthority, AdministratorGameQuery, AdministratorGameView, ApplicationClock,
    ApplicationError, CreateGameCommand, CreatedGame, CreatedGameAccess, GameActionCommand,
    GameActionResult, GameIdSource, GameRepository, HumanSpectatorAuthority,
    HumanSpectatorGameQuery, HumanSpectatorGameView, IdempotencyKey, LexiconResolver,
    PublicGameQuery, PublicGameView, RepositoryError, SeatAuthority, SeatGameQuery, SeatGameView,
    SeedSource, StoredGame,
};

/// Transport-independent application coordinator.
#[derive(Debug)]
pub struct ApplicationService {
    repository: Arc<dyn GameRepository>,
    lexicons: Arc<dyn LexiconResolver>,
    ids: Arc<dyn GameIdSource>,
    seeds: Arc<dyn SeedSource>,
    clock: Arc<dyn ApplicationClock>,
}

impl ApplicationService {
    /// Creates a service from explicit application-boundary adapters.
    #[must_use]
    pub fn new(
        repository: Arc<dyn GameRepository>,
        lexicons: Arc<dyn LexiconResolver>,
        ids: Arc<dyn GameIdSource>,
        seeds: Arc<dyn SeedSource>,
        clock: Arc<dyn ApplicationClock>,
    ) -> Self {
        Self {
            repository,
            lexicons,
            ids,
            seeds,
            clock,
        }
    }

    /// Allocates a fully identified create command from the injected ID source.
    #[must_use]
    pub fn prepare_create_game(
        &self,
        language: word_arena_engine::Language,
        idempotency_key: IdempotencyKey,
    ) -> CreateGameCommand {
        CreateGameCommand {
            game_id: self.ids.next_game_id(),
            language,
            idempotency_key,
        }
    }

    /// Creates, deals, and persists one game with exact immutable inputs.
    ///
    /// # Errors
    ///
    /// Returns a ruleset/pack, engine, or repository error before exposing
    /// authority bindings when creation cannot commit.
    pub async fn create_game(
        &self,
        command: CreateGameCommand,
    ) -> Result<CreatedGame, ApplicationError> {
        let ruleset = Ruleset::for_language(command.language)?;
        let lexicon = self.lexicons.resolve(&ruleset.lexicon).ok_or_else(|| {
            ApplicationError::MissingLexicon {
                game_id: command.game_id.clone(),
            }
        })?;
        let created_at = self.clock.now();
        let game = Game::create(
            command.game_id.as_str(),
            ruleset,
            Some(lexicon),
            self.seeds.next_seed(),
        )?;
        let record = StoredGame {
            game_id: command.game_id.clone(),
            created_at,
            snapshot: game.snapshot(),
        };
        self.repository.insert(record).await?;
        Ok(CreatedGame {
            access: CreatedGameAccess::new(&command.game_id),
            game_id: command.game_id,
            created_at,
            public: game.public_projection(),
        })
    }

    /// Loads the public-only projection for any observer.
    ///
    /// # Errors
    ///
    /// Returns repository, compatibility, or deterministic resume errors.
    pub async fn public_game(
        &self,
        query: PublicGameQuery,
    ) -> Result<PublicGameView, ApplicationError> {
        let game = self.load_game(&query.game_id).await?;
        Ok(PublicGameView {
            observed_at: self.clock.now(),
            game: game.public_projection(),
        })
    }

    /// Loads exactly the competitive seat projection bound to `authority`.
    ///
    /// # Errors
    ///
    /// Rejects cross-game authority before loading, then returns repository,
    /// compatibility, or deterministic resume errors.
    pub async fn seat_game(
        &self,
        authority: &SeatAuthority,
        query: SeatGameQuery,
    ) -> Result<SeatGameView, ApplicationError> {
        ensure_game(authority.game_id(), &query.game_id)?;
        let game = self.load_game(&query.game_id).await?;
        Ok(SeatGameView {
            observed_at: self.clock.now(),
            game: game.seat_projection(authority.seat()),
        })
    }

    /// Loads the human-only spectator projection bound to `authority`.
    ///
    /// # Errors
    ///
    /// Rejects cross-game authority before loading, then returns repository,
    /// compatibility, or deterministic resume errors.
    pub async fn human_spectator_game(
        &self,
        authority: &HumanSpectatorAuthority,
        query: HumanSpectatorGameQuery,
    ) -> Result<HumanSpectatorGameView, ApplicationError> {
        ensure_game(authority.game_id(), &query.game_id)?;
        let game = self.load_game(&query.game_id).await?;
        Ok(HumanSpectatorGameView {
            observed_at: self.clock.now(),
            game: game.human_spectator_projection(),
        })
    }

    /// Loads the complete administrator projection bound to `authority`.
    ///
    /// # Errors
    ///
    /// Rejects cross-game authority before loading, then returns repository,
    /// compatibility, or deterministic resume errors.
    pub async fn administrator_game(
        &self,
        authority: &AdministratorAuthority,
        query: AdministratorGameQuery,
    ) -> Result<AdministratorGameView, ApplicationError> {
        ensure_game(authority.game_id(), &query.game_id)?;
        let game = self.load_game(&query.game_id).await?;
        Ok(AdministratorGameView {
            observed_at: self.clock.now(),
            game: game.administrator_projection(),
        })
    }

    /// Authorizes, executes, and persists one engine action for a bound seat.
    ///
    /// # Errors
    ///
    /// Rejects cross-game/seat authority and mismatched turn/version before
    /// loading. Engine and optimistic repository errors preserve prior state.
    pub async fn act(
        &self,
        authority: &SeatAuthority,
        command: GameActionCommand,
    ) -> Result<GameActionResult, ApplicationError> {
        ensure_game(authority.game_id(), &command.game_id)?;
        if command.turn.seat != authority.seat() {
            return Err(ApplicationError::WrongSeatAuthority {
                actual: authority.seat(),
                claimed: command.turn.seat,
            });
        }
        if command.turn.number != command.expected_version {
            return Err(ApplicationError::TurnVersionMismatch {
                turn: command.turn.number,
                expected_version: command.expected_version,
            });
        }
        let record = self.repository.load(&command.game_id).await?;
        validate_record(&record, &command.game_id)?;
        let mut game = self.resume(&record)?;
        let event = game.apply_move(authority.seat(), command.expected_version, command.action)?;
        let updated = StoredGame {
            game_id: record.game_id,
            created_at: record.created_at,
            snapshot: game.snapshot(),
        };
        self.repository
            .replace(command.expected_version, updated)
            .await?;
        Ok(GameActionResult {
            committed_at: self.clock.now(),
            event,
            game: game.seat_projection(authority.seat()),
        })
    }

    async fn load_game(&self, game_id: &crate::GameId) -> Result<Game, ApplicationError> {
        let record = self.repository.load(game_id).await?;
        validate_record(&record, game_id)?;
        self.resume(&record)
    }

    fn resume(&self, record: &StoredGame) -> Result<Game, ApplicationError> {
        let ruleset = match record.snapshot.state.ruleset_id {
            RulesetId::EnglishV1 => Ruleset::english_v1(),
            RulesetId::FrenchV1 => Ruleset::french_v1(),
        };
        let lexicon = self
            .lexicons
            .resolve(&record.snapshot.state.lexicon)
            .ok_or_else(|| ApplicationError::MissingLexicon {
                game_id: record.game_id.clone(),
            })?;
        Game::resume(record.snapshot.clone(), ruleset, Some(lexicon)).map_err(Into::into)
    }
}

fn ensure_game(
    authority: &crate::GameId,
    requested: &crate::GameId,
) -> Result<(), ApplicationError> {
    if authority == requested {
        Ok(())
    } else {
        Err(ApplicationError::WrongGameAuthority {
            game_id: requested.clone(),
        })
    }
}

fn validate_record(record: &StoredGame, requested: &crate::GameId) -> Result<(), ApplicationError> {
    if &record.game_id == requested && record.snapshot.state.game_id == requested.as_str() {
        Ok(())
    } else {
        Err(RepositoryError::Corrupt.into())
    }
}
