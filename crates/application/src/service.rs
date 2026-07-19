use std::{fmt::Write as _, sync::Arc};

use serde::Serialize;
use sha2::{Digest, Sha256};
use word_arena_engine::{Game, GameError, GamePhase, Move, Ruleset, RulesetId, Seat, Turn};

use crate::capability::{actor, credential, validate_issue};
use crate::{
    ActionCommit, ActionOutcome, ActionRejection, AdministratorCredential, AdministratorGameQuery,
    AdministratorGameView, ApplicationClock, ApplicationError, AuditAction, AuditActor,
    AuditOutcome, AuditRecord, AuthenticatedCredential, CAPABILITY_DIGEST_VERSION,
    CapabilityDescriptor, CapabilityDigestKey, CapabilityError, CapabilityId, CapabilityRecord,
    CapabilityRepository, CapabilityRepositoryError, CapabilityScope, CapabilityToken,
    CapabilityTokenSource, CompetitiveSeatCredential, CreateGameCommand, CreatedGame,
    CreatedGameAccess, CreationIdempotencyLookup, CreationIdempotencyRecord, GameActionCommand,
    GameActionResult, GameId, GameIdSource, GameRepository, HumanSpectatorCredential,
    HumanSpectatorGameQuery, HumanSpectatorGameView, IDEMPOTENCY_DIGEST_VERSION, IdempotencyKey,
    IdempotencyLookup, IdempotencyRecord, InvalidAttemptResponse, InvalidAttemptState,
    IssueCapabilityRequest, IssuedCapability, LexiconResolver, OperationalPolicy,
    PersistedActionResult, PersistedCreateResult, PublicGameQuery, PublicGameView,
    PublicViewerCredential, RepositoryError, SeatGameQuery, SeatGameView, SeedSource, StoredGame,
    TimeoutCommand, TimeoutResponse, TurnDeadline,
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
    capabilities: Arc<dyn CapabilityRepository>,
    capability_tokens: Arc<dyn CapabilityTokenSource>,
    capability_digest_key: CapabilityDigestKey,
}

/// Injected capability security and persistence adapters.
#[derive(Debug)]
pub struct CapabilityAdapters {
    repository: Arc<dyn CapabilityRepository>,
    tokens: Arc<dyn CapabilityTokenSource>,
    digest_key: CapabilityDigestKey,
}

impl CapabilityAdapters {
    /// Groups the capability adapters and secret digest key for process setup.
    #[must_use]
    pub fn new(
        repository: Arc<dyn CapabilityRepository>,
        tokens: Arc<dyn CapabilityTokenSource>,
        digest_key: CapabilityDigestKey,
    ) -> Self {
        Self {
            repository,
            tokens,
            digest_key,
        }
    }
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
        capability_adapters: CapabilityAdapters,
    ) -> Self {
        Self {
            service: ApplicationService::new(repository, lexicons, ids, seeds, clock),
            capabilities: capability_adapters.repository,
            capability_tokens: capability_adapters.tokens,
            capability_digest_key: capability_adapters.digest_key,
        }
    }

    /// Applies one validated versioned reliability policy at bootstrap.
    #[must_use]
    pub fn with_operational_policy(mut self, policy: OperationalPolicy) -> Self {
        self.service = self.service.with_operational_policy(policy);
        self
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

    /// Issues one scoped opaque capability from the operator boundary.
    ///
    /// Only the returned value contains the raw token. Persistence receives a
    /// versioned keyed digest and privacy-safe audit metadata.
    ///
    /// # Errors
    ///
    /// Rejects invalid game, time, role, agent-run, or scope bindings and fails
    /// closed when entropy or persistence is unavailable.
    pub async fn issue_capability(
        &self,
        request: IssueCapabilityRequest,
    ) -> Result<IssuedCapability, CapabilityError> {
        self.service
            .load_game(&request.game_id)
            .await
            .map_err(|error| match error {
                ApplicationError::Repository(error) => CapabilityError::Game(error),
                _ => CapabilityError::InvalidRequest,
            })?;
        let issued_at = self.service.clock.now();
        validate_issue(&request, issued_at)?;
        let material = self.capability_tokens.next_material()?;
        let capability_id = CapabilityId::new(encode_id(material.capability_id()))?;
        let token = CapabilityToken::from_material(&material);
        let descriptor = CapabilityDescriptor {
            capability_id: capability_id.clone(),
            game_id: request.game_id,
            role: request.role,
            scopes: request.scopes,
            issued_at,
            expires_at: request.expires_at,
            agent_run_id: request.agent_run_id,
        };
        let record = CapabilityRecord {
            descriptor: descriptor.clone(),
            token_digest: self.capability_digest_key.digest(token.secret()),
            digest_version: CAPABILITY_DIGEST_VERSION,
            revoked_at: None,
        };
        let audit = capability_audit(
            Some(&descriptor),
            AuditActor::System,
            AuditAction::Issue,
            AuditOutcome::Success,
            None,
            issued_at,
        );
        self.capabilities.insert(record, audit).await?;
        Ok(IssuedCapability { descriptor, token })
    }

    /// Authenticates one token for one game and scope.
    ///
    /// Digest verification uses `HMAC-SHA-256`'s constant-time verifier. Every
    /// result is audited without storing the token or any game payload.
    ///
    /// # Errors
    ///
    /// Returns one deliberately generic unauthorized error for malformed,
    /// unknown, expired, revoked, cross-game, or wrong-scope credentials.
    pub async fn authenticate_capability(
        &self,
        token: &str,
        game_id: &GameId,
        scope: CapabilityScope,
    ) -> Result<AuthenticatedCredential, CapabilityError> {
        let now = self.service.clock.now();
        let Ok(capability_id) = CapabilityToken::parse(token) else {
            self.audit_denial(
                None,
                Some(game_id.clone()),
                scope,
                AuditOutcome::DeniedMalformed,
                now,
            )
            .await?;
            return Err(CapabilityError::Unauthorized);
        };
        let record = match self.capabilities.load(&capability_id).await {
            Ok(record) => record,
            Err(CapabilityRepositoryError::NotFound) => {
                self.audit_denial(
                    Some(capability_id),
                    Some(game_id.clone()),
                    scope,
                    AuditOutcome::DeniedUnknown,
                    now,
                )
                .await?;
                return Err(CapabilityError::Unauthorized);
            }
            Err(error) => return Err(error.into()),
        };
        let denied = if record.digest_version != CAPABILITY_DIGEST_VERSION
            || !self
                .capability_digest_key
                .verifies(token, &record.token_digest)
        {
            Some(AuditOutcome::DeniedUnknown)
        } else if record.revoked_at.is_some() {
            Some(AuditOutcome::DeniedRevoked)
        } else if now >= record.descriptor.expires_at {
            Some(AuditOutcome::DeniedExpired)
        } else if &record.descriptor.game_id != game_id {
            Some(AuditOutcome::DeniedGame)
        } else if !record.descriptor.scopes.contains(&scope) {
            Some(AuditOutcome::DeniedScope)
        } else {
            None
        };
        if let Some(outcome) = denied {
            self.capabilities
                .append_audit(capability_audit(
                    Some(&record.descriptor),
                    AuditActor::System,
                    AuditAction::Authenticate,
                    outcome,
                    Some(scope),
                    now,
                ))
                .await?;
            return Err(CapabilityError::Unauthorized);
        }

        self.capabilities
            .append_audit(capability_audit(
                Some(&record.descriptor),
                actor(record.descriptor.role),
                AuditAction::Authenticate,
                AuditOutcome::Success,
                Some(scope),
                now,
            ))
            .await?;
        if matches!(
            scope,
            CapabilityScope::ObserveHumanSpectator | CapabilityScope::ObserveAdministrator
        ) {
            self.capabilities
                .append_audit(capability_audit(
                    Some(&record.descriptor),
                    actor(record.descriptor.role),
                    AuditAction::PrivilegedAccess,
                    AuditOutcome::Success,
                    Some(scope),
                    now,
                ))
                .await?;
        }
        Ok(credential(&record))
    }

    /// Immediately revokes exactly one capability.
    ///
    /// # Errors
    ///
    /// Returns a repository error for missing, already-revoked, corrupt, or
    /// unavailable records.
    pub async fn revoke_capability(
        &self,
        capability_id: &CapabilityId,
    ) -> Result<(), CapabilityError> {
        let record = self.capabilities.load(capability_id).await?;
        let revoked_at = self.service.clock.now();
        let audit = capability_audit(
            Some(&record.descriptor),
            AuditActor::System,
            AuditAction::Revoke,
            AuditOutcome::Success,
            None,
            revoked_at,
        );
        self.capabilities
            .revoke(capability_id, revoked_at, audit)
            .await?;
        Ok(())
    }

    /// Atomically revokes one capability and returns a same-binding replacement.
    ///
    /// # Errors
    ///
    /// Rejects expired replacement time and fails on missing, revoked,
    /// concurrent, entropy, or storage errors.
    pub async fn rotate_capability(
        &self,
        capability_id: &CapabilityId,
        expires_at: crate::UnixMillis,
    ) -> Result<IssuedCapability, CapabilityError> {
        let prior = self.capabilities.load(capability_id).await?;
        let now = self.service.clock.now();
        let request = IssueCapabilityRequest {
            game_id: prior.descriptor.game_id.clone(),
            role: prior.descriptor.role,
            scopes: prior.descriptor.scopes.clone(),
            expires_at,
            agent_run_id: prior.descriptor.agent_run_id.clone(),
        };
        validate_issue(&request, now)?;
        if prior.revoked_at.is_some() || now >= prior.descriptor.expires_at {
            return Err(CapabilityError::Unauthorized);
        }
        let material = self.capability_tokens.next_material()?;
        let replacement_id = CapabilityId::new(encode_id(material.capability_id()))?;
        let token = CapabilityToken::from_material(&material);
        let descriptor = CapabilityDescriptor {
            capability_id: replacement_id,
            game_id: request.game_id,
            role: request.role,
            scopes: request.scopes,
            issued_at: now,
            expires_at: request.expires_at,
            agent_run_id: request.agent_run_id,
        };
        let replacement = CapabilityRecord {
            descriptor: descriptor.clone(),
            token_digest: self.capability_digest_key.digest(token.secret()),
            digest_version: CAPABILITY_DIGEST_VERSION,
            revoked_at: None,
        };
        let audits = [
            capability_audit(
                Some(&prior.descriptor),
                AuditActor::System,
                AuditAction::Rotate,
                AuditOutcome::Success,
                None,
                now,
            ),
            capability_audit(
                Some(&descriptor),
                AuditActor::System,
                AuditAction::Issue,
                AuditOutcome::Success,
                None,
                now,
            ),
        ];
        self.capabilities
            .rotate(capability_id, now, replacement, audits)
            .await?;
        Ok(IssuedCapability { descriptor, token })
    }

    async fn audit_denial(
        &self,
        capability_id: Option<CapabilityId>,
        game_id: Option<GameId>,
        scope: CapabilityScope,
        outcome: AuditOutcome,
        occurred_at: crate::UnixMillis,
    ) -> Result<(), CapabilityError> {
        self.capabilities
            .append_audit(AuditRecord {
                game_id,
                actor: AuditActor::System,
                action: AuditAction::Authenticate,
                outcome,
                capability_id,
                scope: Some(scope),
                occurred_at,
            })
            .await?;
        Ok(())
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
    policy: OperationalPolicy,
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
            policy: OperationalPolicy::default(),
        }
    }

    /// Overrides the versioned reliability policy during process bootstrap.
    ///
    /// # Panics
    ///
    /// Panics when a version, duration, or attempt limit is zero or negative.
    #[must_use]
    pub fn with_operational_policy(mut self, policy: OperationalPolicy) -> Self {
        assert!(policy.version > 0, "policy version must be positive");
        assert!(
            policy.turn_duration_ms > 0,
            "turn duration must be positive"
        );
        assert!(
            policy.invalid_attempt_limit > 0,
            "attempt limit must be positive"
        );
        self.policy = policy;
        self
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
        let key_digest = digest_key(&command.idempotency_key);
        let payload_sha256 = payload_sha256(&CreatePayload {
            language: command.language,
        })?;
        match self
            .repository
            .load_creation_idempotency(key_digest, &payload_sha256)
            .await?
        {
            CreationIdempotencyLookup::Match(result) => return Ok(created_game(*result)),
            CreationIdempotencyLookup::PayloadConflict => {
                return Err(ApplicationError::ActionRejected(
                    ActionRejection::IdempotencyConflict,
                ));
            }
            CreationIdempotencyLookup::Missing => {}
        }
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
            turn_deadline: self.next_deadline(&game, created_at),
        };
        let result = PersistedCreateResult {
            game_id: command.game_id,
            created_at,
            public: game.public_projection(),
        };
        let creation = CreationIdempotencyRecord {
            key_digest,
            digest_version: IDEMPOTENCY_DIGEST_VERSION,
            payload_sha256: payload_sha256.clone(),
            result: result.clone(),
        };
        if let Err(error) = self.repository.insert_idempotent(record, creation).await {
            if matches!(
                error,
                RepositoryError::Conflict | RepositoryError::AlreadyExists
            ) {
                match self
                    .repository
                    .load_creation_idempotency(key_digest, &payload_sha256)
                    .await?
                {
                    CreationIdempotencyLookup::Match(cached) => return Ok(created_game(*cached)),
                    CreationIdempotencyLookup::PayloadConflict => {
                        return Err(ApplicationError::ActionRejected(
                            ActionRejection::IdempotencyConflict,
                        ));
                    }
                    CreationIdempotencyLookup::Missing => {}
                }
            }
            return Err(error.into());
        }
        Ok(created_game(result))
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
        let payload_sha256 = payload_sha256(&ActionPayload {
            game_id: &command.game_id,
            expected_version: command.expected_version,
            turn: command.turn,
            action: &command.action,
        })?;
        let key_digest = digest_key(&command.idempotency_key);
        if let Some(result) = self
            .cached_outcome(&command.game_id, key_digest, &payload_sha256)
            .await?
        {
            return result;
        }
        self.execute_action(
            &command.game_id,
            command.expected_version,
            credential.seat(),
            command.action,
            "game_action",
            key_digest,
            payload_sha256,
            true,
        )
        .await
    }

    /// Resolves one due turn using the injected timeout policy.
    ///
    /// Concurrent player actions and timeout workers share the same optimistic
    /// version, so exactly one transition can commit.
    ///
    /// # Errors
    ///
    /// Returns a stable deadline rejection, engine rejection, or repository
    /// failure without partially committing a transition.
    pub async fn resolve_timeout(
        &self,
        command: TimeoutCommand,
    ) -> Result<GameActionResult, ApplicationError> {
        let key = crate::IdempotencyKey::new(format!(
            "timeout:{}:{}:{}",
            command.game_id, command.expected_version, self.policy.version
        ))?;
        let payload_sha256 = payload_sha256(&TimeoutPayload {
            game_id: &command.game_id,
            expected_version: command.expected_version,
            policy_version: self.policy.version,
        })?;
        let key_digest = digest_key(&key);
        if let Some(result) = self
            .cached_outcome(&command.game_id, key_digest, &payload_sha256)
            .await?
        {
            return result;
        }
        let record = self.load_record(&command.game_id).await?;
        let deadline = record
            .turn_deadline
            .ok_or(ApplicationError::ActionRejected(
                ActionRejection::DeadlineNotReached,
            ))?;
        if deadline.turn != command.expected_version || self.clock.now() < deadline.deadline_at {
            return Err(ApplicationError::ActionRejected(
                ActionRejection::DeadlineNotReached,
            ));
        }
        let move_to_apply = match self.policy.timeout_response {
            TimeoutResponse::Pass => Move::Pass,
            TimeoutResponse::Resign => Move::Resign,
        };
        self.execute_action(
            &command.game_id,
            command.expected_version,
            deadline.seat,
            move_to_apply,
            "turn_timeout",
            key_digest,
            payload_sha256,
            false,
        )
        .await
    }

    /// Resolves a bounded batch of persisted deadlines for a restart-safe worker.
    ///
    /// # Errors
    ///
    /// Returns the first non-race repository or deterministic application
    /// failure. Expected timeout/action races are ignored.
    pub async fn resolve_due_timeouts(
        &self,
        limit: u32,
    ) -> Result<Vec<TimeoutCommand>, ApplicationError> {
        let commands = self
            .repository
            .due_timeouts(self.clock.now(), limit)
            .await?;
        let mut committed = Vec::new();
        for command in commands {
            match self.resolve_timeout(command.clone()).await {
                Ok(_) => committed.push(command),
                Err(
                    ApplicationError::Repository(RepositoryError::Conflict)
                    | ApplicationError::ActionRejected(
                        ActionRejection::VersionConflict | ActionRejection::DeadlineNotReached,
                    ),
                ) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(committed)
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn execute_action(
        &self,
        game_id: &GameId,
        expected_version: u64,
        seat: Seat,
        action: Move,
        command_kind: &str,
        key_digest: [u8; 32],
        payload_sha256: String,
        count_invalid: bool,
    ) -> Result<GameActionResult, ApplicationError> {
        let record = self.load_record(game_id).await?;
        let mut game = self.resume(&record)?;
        let committed_at = self.clock.now();
        let attempted = game.apply_move(seat, expected_version, action);
        let (outcome, successor, invalid_attempt, replay) = match attempted {
            Ok(event) => {
                let result = PersistedActionResult {
                    committed_at,
                    event,
                    game: game.seat_projection(seat),
                };
                let replay = game.replay_bundle();
                let successor = StoredGame {
                    game_id: record.game_id.clone(),
                    created_at: record.created_at,
                    updated_at: committed_at,
                    snapshot: game.snapshot(),
                    turn_deadline: self.next_deadline(&game, committed_at),
                };
                (
                    ActionOutcome::Accepted(Box::new(result)),
                    Some(successor),
                    None,
                    replay,
                )
            }
            Err(error) => {
                let rejection = rejection_from_engine(&error);
                let mut successor = None;
                let mut replay = None;
                let invalid_attempt =
                    if count_invalid && !matches!(rejection, ActionRejection::VersionConflict) {
                        let prior = self
                            .repository
                            .load_invalid_attempt(game_id, expected_version, seat)
                            .await?
                            .map_or(0, |state| state.count);
                        let count = prior.saturating_add(1);
                        if count >= self.policy.invalid_attempt_limit
                            && self.policy.invalid_attempt_response
                                != InvalidAttemptResponse::RejectOnly
                        {
                            let mut policy_game = self.resume(&record)?;
                            let response = match self.policy.invalid_attempt_response {
                                InvalidAttemptResponse::Pass => Move::Pass,
                                InvalidAttemptResponse::Resign => Move::Resign,
                                InvalidAttemptResponse::RejectOnly => unreachable!(),
                            };
                            policy_game.apply_move(seat, expected_version, response)?;
                            replay = policy_game.replay_bundle();
                            successor = Some(StoredGame {
                                game_id: record.game_id.clone(),
                                created_at: record.created_at,
                                updated_at: committed_at,
                                snapshot: policy_game.snapshot(),
                                turn_deadline: self.next_deadline(&policy_game, committed_at),
                            });
                        }
                        Some(InvalidAttemptState {
                            turn: expected_version,
                            seat,
                            policy_version: self.policy.version,
                            count,
                        })
                    } else {
                        None
                    };
                (
                    ActionOutcome::Rejected(rejection),
                    successor,
                    invalid_attempt,
                    replay,
                )
            }
        };
        let record = IdempotencyRecord {
            game_id: game_id.clone(),
            key_digest,
            digest_version: IDEMPOTENCY_DIGEST_VERSION,
            command_kind: command_kind.to_owned(),
            payload_sha256: payload_sha256.clone(),
            outcome: outcome.clone(),
            created_at: committed_at,
        };
        let commit = ActionCommit {
            expected_version,
            successor,
            idempotency: record,
            invalid_attempt,
            replay,
        };
        if let Err(error) = self.repository.commit_action(commit).await {
            if error == RepositoryError::Conflict
                && let Some(result) = self
                    .cached_outcome(game_id, key_digest, &payload_sha256)
                    .await?
            {
                return result;
            }
            return Err(error.into());
        }
        outcome_result(outcome)
    }

    async fn cached_outcome(
        &self,
        game_id: &GameId,
        key_digest: [u8; 32],
        payload_sha256: &str,
    ) -> Result<Option<Result<GameActionResult, ApplicationError>>, ApplicationError> {
        Ok(
            match self
                .repository
                .load_idempotency(game_id, key_digest, payload_sha256)
                .await?
            {
                IdempotencyLookup::Missing => None,
                IdempotencyLookup::PayloadConflict => Some(Err(ApplicationError::ActionRejected(
                    ActionRejection::IdempotencyConflict,
                ))),
                IdempotencyLookup::Match(outcome) => Some(outcome_result(outcome)),
            },
        )
    }

    fn next_deadline(&self, game: &Game, from: crate::UnixMillis) -> Option<TurnDeadline> {
        (game.public_projection().state.phase == GamePhase::Active).then(|| TurnDeadline {
            turn: game.public_projection().state.version,
            seat: game.public_projection().state.current_player,
            deadline_at: crate::UnixMillis(from.0.saturating_add(self.policy.turn_duration_ms)),
            policy_version: self.policy.version,
        })
    }

    async fn load_record(&self, game_id: &GameId) -> Result<StoredGame, ApplicationError> {
        match self.repository.load(game_id).await {
            Ok(record) => Ok(record),
            Err(RepositoryError::Corrupt) => {
                let recovery = self.repository.load_recovery(game_id).await?;
                let lexicon = self
                    .lexicons
                    .resolve(&recovery.replay.lexicon)
                    .ok_or_else(|| ApplicationError::MissingLexicon {
                        game_id: game_id.clone(),
                    })?;
                let game = Game::replay(&recovery.replay, Some(lexicon))?;
                Ok(StoredGame {
                    game_id: recovery.game_id,
                    created_at: recovery.created_at,
                    updated_at: recovery.updated_at,
                    snapshot: game.snapshot(),
                    turn_deadline: None,
                })
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn load_game(&self, game_id: &crate::GameId) -> Result<Game, ApplicationError> {
        let record = self.load_record(game_id).await?;
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

#[derive(Serialize)]
struct ActionPayload<'a> {
    game_id: &'a GameId,
    expected_version: u64,
    turn: Turn,
    action: &'a Move,
}

#[derive(Serialize)]
struct CreatePayload {
    language: word_arena_engine::Language,
}

#[derive(Serialize)]
struct TimeoutPayload<'a> {
    game_id: &'a GameId,
    expected_version: u64,
    policy_version: u32,
}

fn digest_key(key: &IdempotencyKey) -> [u8; 32] {
    Sha256::digest(key.as_str().as_bytes()).into()
}

fn payload_sha256(payload: &impl Serialize) -> Result<String, ApplicationError> {
    let bytes = serde_json::to_vec(payload).map_err(|_| RepositoryError::Corrupt)?;
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(encoded)
}

fn rejection_from_engine(error: &GameError) -> ActionRejection {
    if matches!(error, GameError::StaleVersion { .. }) {
        ActionRejection::VersionConflict
    } else {
        ActionRejection::IllegalAction {
            message: error.to_string(),
        }
    }
}

fn outcome_result(outcome: ActionOutcome) -> Result<GameActionResult, ApplicationError> {
    match outcome {
        ActionOutcome::Accepted(result) => Ok(GameActionResult {
            committed_at: result.committed_at,
            event: result.event,
            game: result.game,
        }),
        ActionOutcome::Rejected(rejection) => Err(ApplicationError::ActionRejected(rejection)),
    }
}

fn created_game(result: PersistedCreateResult) -> CreatedGame {
    CreatedGame {
        access: CreatedGameAccess::new(&result.game_id),
        game_id: result.game_id,
        created_at: result.created_at,
        public: result.public,
    }
}

fn capability_audit(
    descriptor: Option<&CapabilityDescriptor>,
    actor: AuditActor,
    action: AuditAction,
    outcome: AuditOutcome,
    scope: Option<CapabilityScope>,
    occurred_at: crate::UnixMillis,
) -> AuditRecord {
    AuditRecord {
        game_id: descriptor.map(|value| value.game_id.clone()),
        actor,
        action,
        outcome,
        capability_id: descriptor.map(|value| value.capability_id.clone()),
        scope,
        occurred_at,
    }
}

fn encode_id(bytes: &[u8; 16]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(32);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}
