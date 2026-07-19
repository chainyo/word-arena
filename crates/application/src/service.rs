use std::sync::Arc;

use word_arena_engine::{Game, Ruleset, RulesetId};

use crate::{
    AdministratorCredential, AdministratorGameQuery, AdministratorGameView, ApplicationClock,
    ApplicationError, CompetitiveSeatCredential, CreateGameCommand, CreatedGame, CreatedGameAccess,
    GameActionCommand, GameActionResult, GameId, GameIdSource, GameRepository,
    HumanSpectatorCredential, HumanSpectatorGameQuery, HumanSpectatorGameView, IdempotencyKey,
    LexiconResolver, PublicGameQuery, PublicGameView, PublicViewerCredential, RepositoryError,
    SeatGameQuery, SeatGameView, SeedSource, StoredGame,
};

/// Process-bootstrap boundary that owns operator-only credential issuance.
///
/// Agent drivers and transport handlers should receive only
/// [`ApplicationService`] plus their authenticated competitive credentials.
/// Keeping this runtime out of agent processes makes human-spectator and
/// administrator issuance unavailable to agent configuration and commands.
#[derive(Debug)]
pub struct ApplicationRuntime {
    service: ApplicationService,
}

impl ApplicationRuntime {
    /// Builds the application process from explicit adapters.
    #[must_use]
    pub fn new(
        repository: Arc<dyn GameRepository>,
        lexicons: Arc<dyn LexiconResolver>,
        ids: Arc<dyn GameIdSource>,
        seeds: Arc<dyn SeedSource>,
        clock: Arc<dyn ApplicationClock>,
    ) -> Self {
        Self {
            service: ApplicationService::new(repository, lexicons, ids, seeds, clock),
        }
    }

    /// Non-operator game use cases safe to give to transport and agent adapters.
    #[must_use]
    pub const fn service(&self) -> &ApplicationService {
        &self.service
    }

    /// Issues a public-view credential after confirming that the game exists.
    ///
    /// # Errors
    ///
    /// Returns an application error when the game cannot be loaded exactly.
    pub async fn issue_public_viewer(
        &self,
        game_id: &GameId,
    ) -> Result<PublicViewerCredential, ApplicationError> {
        self.service.load_game(game_id).await?;
        Ok(PublicViewerCredential::new(game_id))
    }

    /// Issues a human-only spectator credential from the operator boundary.
    ///
    /// # Errors
    ///
    /// Returns an application error when the game cannot be loaded exactly.
    pub async fn issue_human_spectator(
        &self,
        game_id: &GameId,
    ) -> Result<HumanSpectatorCredential, ApplicationError> {
        self.service.load_game(game_id).await?;
        Ok(HumanSpectatorCredential::new(game_id))
    }

    /// Issues an administrator credential from the operator boundary.
    ///
    /// # Errors
    ///
    /// Returns an application error when the game cannot be loaded exactly.
    pub async fn issue_administrator(
        &self,
        game_id: &GameId,
    ) -> Result<AdministratorCredential, ApplicationError> {
        self.service.load_game(game_id).await?;
        Ok(AdministratorCredential::new(game_id))
    }
}

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
    fn new(
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
    /// credential bindings when creation cannot commit.
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
            updated_at: created_at,
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

    /// Loads the public-only projection for a game-bound observer.
    ///
    /// # Errors
    ///
    /// Returns repository, compatibility, or deterministic resume errors.
    pub async fn public_game(
        &self,
        credential: &PublicViewerCredential,
        query: PublicGameQuery,
    ) -> Result<PublicGameView, ApplicationError> {
        ensure_game(credential.game_id(), &query.game_id)?;
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
        credential: &CompetitiveSeatCredential,
        query: SeatGameQuery,
    ) -> Result<SeatGameView, ApplicationError> {
        ensure_game(credential.game_id(), &query.game_id)?;
        let game = self.load_game(&query.game_id).await?;
        Ok(SeatGameView {
            observed_at: self.clock.now(),
            game: game.seat_projection(credential.seat()),
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
        credential: &HumanSpectatorCredential,
        query: HumanSpectatorGameQuery,
    ) -> Result<HumanSpectatorGameView, ApplicationError> {
        ensure_game(credential.game_id(), &query.game_id)?;
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
        credential: &AdministratorCredential,
        query: AdministratorGameQuery,
    ) -> Result<AdministratorGameView, ApplicationError> {
        ensure_game(credential.game_id(), &query.game_id)?;
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
        credential: &CompetitiveSeatCredential,
        command: GameActionCommand,
    ) -> Result<GameActionResult, ApplicationError> {
        ensure_game(credential.game_id(), &command.game_id)?;
        if command.turn.seat != credential.seat() {
            return Err(ApplicationError::WrongSeatAuthority {
                actual: credential.seat(),
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
        let event = game.apply_move(credential.seat(), command.expected_version, command.action)?;
        let committed_at = self.clock.now();
        let updated = StoredGame {
            game_id: record.game_id,
            created_at: record.created_at,
            updated_at: committed_at,
            snapshot: game.snapshot(),
        };
        self.repository
            .replace(command.expected_version, updated)
            .await?;
        Ok(GameActionResult {
            committed_at,
            event,
            game: game.seat_projection(credential.seat()),
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
