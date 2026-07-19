//! Deterministic in-memory adapters for application tests and examples.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
};

use word_arena_engine::{GameSeed, WordValidator};
use word_arena_lexicon::PackIdentity;

use crate::{
    ApplicationClock, AuditRecord, BoxFuture, CapabilityId, CapabilityMaterial, CapabilityRecord,
    CapabilityRepository, CapabilityRepositoryError, CapabilityTokenSource, GameId, GameIdSource,
    GameRepository, LexiconResolver, RepositoryError, SeedSource, StoredGame, UnixMillis,
};

#[derive(Debug, Default)]
struct CapabilityState {
    records: BTreeMap<CapabilityId, CapabilityRecord>,
    audits: Vec<AuditRecord>,
}

/// Deterministic, atomic capability/audit repository for tests.
#[derive(Debug, Default)]
pub struct InMemoryCapabilityRepository {
    state: Mutex<CapabilityState>,
}

impl InMemoryCapabilityRepository {
    /// Returns a secret-free snapshot of all audit rows.
    ///
    /// # Panics
    ///
    /// Panics only when another test poisoned the fixture mutex.
    #[must_use]
    pub fn audits(&self) -> Vec<AuditRecord> {
        self.state
            .lock()
            .expect("test capability mutex is not poisoned")
            .audits
            .clone()
    }

    /// Returns one persisted digest-only record.
    ///
    /// # Panics
    ///
    /// Panics only when another test poisoned the fixture mutex.
    #[must_use]
    pub fn record(&self, capability_id: &CapabilityId) -> Option<CapabilityRecord> {
        self.state
            .lock()
            .expect("test capability mutex is not poisoned")
            .records
            .get(capability_id)
            .cloned()
    }
}

impl CapabilityRepository for InMemoryCapabilityRepository {
    fn insert(
        &self,
        capability: CapabilityRecord,
        audit: AuditRecord,
    ) -> BoxFuture<'_, Result<(), CapabilityRepositoryError>> {
        Box::pin(async move {
            let mut state = self
                .state
                .lock()
                .map_err(|_| CapabilityRepositoryError::Unavailable)?;
            if state
                .records
                .contains_key(&capability.descriptor.capability_id)
                || state
                    .records
                    .values()
                    .any(|record| record.token_digest == capability.token_digest)
            {
                return Err(CapabilityRepositoryError::AlreadyExists);
            }
            state
                .records
                .insert(capability.descriptor.capability_id.clone(), capability);
            state.audits.push(audit);
            Ok(())
        })
    }

    fn load(
        &self,
        capability_id: &CapabilityId,
    ) -> BoxFuture<'_, Result<CapabilityRecord, CapabilityRepositoryError>> {
        let capability_id = capability_id.clone();
        Box::pin(async move {
            self.state
                .lock()
                .map_err(|_| CapabilityRepositoryError::Unavailable)?
                .records
                .get(&capability_id)
                .cloned()
                .ok_or(CapabilityRepositoryError::NotFound)
        })
    }

    fn revoke(
        &self,
        capability_id: &CapabilityId,
        revoked_at: UnixMillis,
        audit: AuditRecord,
    ) -> BoxFuture<'_, Result<(), CapabilityRepositoryError>> {
        let capability_id = capability_id.clone();
        Box::pin(async move {
            let mut state = self
                .state
                .lock()
                .map_err(|_| CapabilityRepositoryError::Unavailable)?;
            let record = state
                .records
                .get_mut(&capability_id)
                .ok_or(CapabilityRepositoryError::NotFound)?;
            if record.revoked_at.is_some() {
                return Err(CapabilityRepositoryError::Conflict);
            }
            record.revoked_at = Some(revoked_at);
            state.audits.push(audit);
            Ok(())
        })
    }

    fn rotate(
        &self,
        prior_id: &CapabilityId,
        revoked_at: UnixMillis,
        replacement: CapabilityRecord,
        audits: [AuditRecord; 2],
    ) -> BoxFuture<'_, Result<(), CapabilityRepositoryError>> {
        let prior_id = prior_id.clone();
        Box::pin(async move {
            let mut state = self
                .state
                .lock()
                .map_err(|_| CapabilityRepositoryError::Unavailable)?;
            if state
                .records
                .contains_key(&replacement.descriptor.capability_id)
                || state
                    .records
                    .values()
                    .any(|record| record.token_digest == replacement.token_digest)
            {
                return Err(CapabilityRepositoryError::AlreadyExists);
            }
            let prior = state
                .records
                .get_mut(&prior_id)
                .ok_or(CapabilityRepositoryError::NotFound)?;
            if prior.revoked_at.is_some() {
                return Err(CapabilityRepositoryError::Conflict);
            }
            prior.revoked_at = Some(revoked_at);
            state
                .records
                .insert(replacement.descriptor.capability_id.clone(), replacement);
            state.audits.extend(audits);
            Ok(())
        })
    }

    fn append_audit(
        &self,
        audit: AuditRecord,
    ) -> BoxFuture<'_, Result<(), CapabilityRepositoryError>> {
        Box::pin(async move {
            self.state
                .lock()
                .map_err(|_| CapabilityRepositoryError::Unavailable)?
                .audits
                .push(audit);
            Ok(())
        })
    }
}

/// Deterministic sequence-backed capability material source.
#[derive(Debug)]
pub struct SequenceCapabilityTokens {
    next: Mutex<u8>,
}

impl SequenceCapabilityTokens {
    /// Starts at one explicit non-secret fixture byte.
    #[must_use]
    pub const fn new(first: u8) -> Self {
        Self {
            next: Mutex::new(first),
        }
    }
}

impl CapabilityTokenSource for SequenceCapabilityTokens {
    fn next_material(&self) -> Result<CapabilityMaterial, crate::CapabilityError> {
        let mut next = self.next.lock().expect("test token mutex is not poisoned");
        let value = *next;
        *next = next.wrapping_add(1);
        Ok(CapabilityMaterial::new(
            [value; 16],
            [value.wrapping_add(128); 32],
        ))
    }
}

/// Thread-safe optimistic in-memory game repository.
#[derive(Debug, Default)]
struct InMemoryGameState {
    games: BTreeMap<GameId, StoredGame>,
    creations: BTreeMap<[u8; 32], crate::CreationIdempotencyRecord>,
    idempotency: BTreeMap<(GameId, [u8; 32]), crate::IdempotencyRecord>,
    invalid_attempts: BTreeMap<(GameId, u64, u8), crate::InvalidAttemptState>,
    recoveries: BTreeMap<GameId, crate::RecoveryRecord>,
}

#[derive(Debug, Default)]
pub struct InMemoryGameRepository {
    state: Mutex<InMemoryGameState>,
}

impl GameRepository for InMemoryGameRepository {
    fn insert(&self, game: StoredGame) -> BoxFuture<'_, Result<(), RepositoryError>> {
        Box::pin(async move {
            let mut state = self
                .state
                .lock()
                .map_err(|_| RepositoryError::Unavailable)?;
            if state.games.contains_key(&game.game_id) {
                return Err(RepositoryError::AlreadyExists);
            }
            state.games.insert(game.game_id.clone(), game);
            Ok(())
        })
    }

    fn insert_idempotent(
        &self,
        game: StoredGame,
        idempotency: crate::CreationIdempotencyRecord,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        Box::pin(async move {
            let mut state = self
                .state
                .lock()
                .map_err(|_| RepositoryError::Unavailable)?;
            if state.games.contains_key(&game.game_id)
                || state.creations.contains_key(&idempotency.key_digest)
            {
                return Err(RepositoryError::Conflict);
            }
            state.games.insert(game.game_id.clone(), game);
            state.creations.insert(idempotency.key_digest, idempotency);
            Ok(())
        })
    }

    fn load_creation_idempotency(
        &self,
        key_digest: [u8; 32],
        payload_sha256: &str,
    ) -> BoxFuture<'_, Result<crate::CreationIdempotencyLookup, RepositoryError>> {
        let payload_sha256 = payload_sha256.to_owned();
        Box::pin(async move {
            let state = self
                .state
                .lock()
                .map_err(|_| RepositoryError::Unavailable)?;
            Ok(match state.creations.get(&key_digest) {
                None => crate::CreationIdempotencyLookup::Missing,
                Some(record) if record.payload_sha256 == payload_sha256 => {
                    crate::CreationIdempotencyLookup::Match(Box::new(record.result.clone()))
                }
                Some(_) => crate::CreationIdempotencyLookup::PayloadConflict,
            })
        })
    }

    fn load(&self, game_id: &GameId) -> BoxFuture<'_, Result<StoredGame, RepositoryError>> {
        let game_id = game_id.clone();
        Box::pin(async move {
            self.state
                .lock()
                .map_err(|_| RepositoryError::Unavailable)?
                .games
                .get(&game_id)
                .cloned()
                .ok_or(RepositoryError::NotFound)
        })
    }

    fn replace(
        &self,
        expected_version: u64,
        game: StoredGame,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        Box::pin(async move {
            let mut state = self
                .state
                .lock()
                .map_err(|_| RepositoryError::Unavailable)?;
            let current = state
                .games
                .get(&game.game_id)
                .ok_or(RepositoryError::NotFound)?;
            if current.snapshot.state.version != expected_version {
                return Err(RepositoryError::Conflict);
            }
            state.games.insert(game.game_id.clone(), game);
            Ok(())
        })
    }

    fn load_idempotency(
        &self,
        game_id: &GameId,
        key_digest: [u8; 32],
        payload_sha256: &str,
    ) -> BoxFuture<'_, Result<crate::IdempotencyLookup, RepositoryError>> {
        let game_id = game_id.clone();
        let payload_sha256 = payload_sha256.to_owned();
        Box::pin(async move {
            let state = self
                .state
                .lock()
                .map_err(|_| RepositoryError::Unavailable)?;
            Ok(match state.idempotency.get(&(game_id, key_digest)) {
                None => crate::IdempotencyLookup::Missing,
                Some(record) if record.payload_sha256 == payload_sha256 => {
                    crate::IdempotencyLookup::Match(record.outcome.clone())
                }
                Some(_) => crate::IdempotencyLookup::PayloadConflict,
            })
        })
    }

    fn load_invalid_attempt(
        &self,
        game_id: &GameId,
        turn: u64,
        seat: word_arena_engine::Seat,
    ) -> BoxFuture<'_, Result<Option<crate::InvalidAttemptState>, RepositoryError>> {
        let game_id = game_id.clone();
        Box::pin(async move {
            Ok(self
                .state
                .lock()
                .map_err(|_| RepositoryError::Unavailable)?
                .invalid_attempts
                .get(&(game_id, turn, test_seat_number(seat)))
                .copied())
        })
    }

    fn commit_action(
        &self,
        commit: crate::ActionCommit,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        Box::pin(async move {
            let mut state = self
                .state
                .lock()
                .map_err(|_| RepositoryError::Unavailable)?;
            let key = (
                commit.idempotency.game_id.clone(),
                commit.idempotency.key_digest,
            );
            if state.idempotency.contains_key(&key) {
                return Err(RepositoryError::Conflict);
            }
            let current = state
                .games
                .get(&commit.idempotency.game_id)
                .ok_or(RepositoryError::NotFound)?;
            if let Some(attempt) = commit.invalid_attempt {
                let key = (
                    commit.idempotency.game_id.clone(),
                    attempt.turn,
                    test_seat_number(attempt.seat),
                );
                let prior = state
                    .invalid_attempts
                    .get(&key)
                    .map_or(0, |value| value.count);
                if attempt.count != prior.saturating_add(1) {
                    return Err(RepositoryError::Conflict);
                }
            }
            if let Some(successor) = &commit.successor {
                if current.snapshot.state.version != commit.expected_version {
                    return Err(RepositoryError::Conflict);
                }
                if let Some(replay) = commit.replay.clone() {
                    state.recoveries.insert(
                        successor.game_id.clone(),
                        crate::RecoveryRecord {
                            game_id: successor.game_id.clone(),
                            created_at: successor.created_at,
                            updated_at: successor.updated_at,
                            phase: successor.snapshot.state.phase,
                            replay,
                        },
                    );
                }
                state
                    .games
                    .insert(successor.game_id.clone(), successor.clone());
            }
            if let Some(attempt) = commit.invalid_attempt {
                state.invalid_attempts.insert(
                    (
                        commit.idempotency.game_id.clone(),
                        attempt.turn,
                        test_seat_number(attempt.seat),
                    ),
                    attempt,
                );
            }
            state.idempotency.insert(key, commit.idempotency);
            Ok(())
        })
    }

    fn load_recovery(
        &self,
        game_id: &GameId,
    ) -> BoxFuture<'_, Result<crate::RecoveryRecord, RepositoryError>> {
        let game_id = game_id.clone();
        Box::pin(async move {
            self.state
                .lock()
                .map_err(|_| RepositoryError::Unavailable)?
                .recoveries
                .get(&game_id)
                .cloned()
                .ok_or(RepositoryError::Corrupt)
        })
    }

    fn due_timeouts(
        &self,
        now: UnixMillis,
        limit: u32,
    ) -> BoxFuture<'_, Result<Vec<crate::TimeoutCommand>, RepositoryError>> {
        Box::pin(async move {
            let state = self
                .state
                .lock()
                .map_err(|_| RepositoryError::Unavailable)?;
            Ok(state
                .games
                .values()
                .filter_map(|game| {
                    game.turn_deadline
                        .filter(|deadline| deadline.deadline_at <= now)
                        .map(|deadline| crate::TimeoutCommand {
                            game_id: game.game_id.clone(),
                            expected_version: deadline.turn,
                        })
                })
                .take(limit as usize)
                .collect())
        })
    }
}

const fn test_seat_number(seat: word_arena_engine::Seat) -> u8 {
    match seat {
        word_arena_engine::Seat::One => 1,
        word_arena_engine::Seat::Two => 2,
    }
}

/// Exact-identity fixture lexicon resolver.
#[derive(Debug, Default)]
pub struct InMemoryLexiconResolver {
    lexicons: HashMap<PackIdentity, Arc<dyn WordValidator>>,
}

impl InMemoryLexiconResolver {
    /// Creates a resolver from immutable validators keyed by full identity.
    #[must_use]
    pub fn new(lexicons: impl IntoIterator<Item = Arc<dyn WordValidator>>) -> Self {
        Self {
            lexicons: lexicons
                .into_iter()
                .map(|lexicon| (lexicon.identity().clone(), lexicon))
                .collect(),
        }
    }
}

impl LexiconResolver for InMemoryLexiconResolver {
    fn resolve(&self, identity: &PackIdentity) -> Option<Arc<dyn WordValidator>> {
        self.lexicons.get(identity).cloned()
    }
}

/// Deterministic sequence-backed game ID source.
#[derive(Debug)]
pub struct SequenceGameIds {
    prefix: String,
    next: Mutex<u64>,
}

impl SequenceGameIds {
    /// Creates IDs as `<prefix>-<zero-based sequence>`.
    #[must_use]
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            next: Mutex::new(0),
        }
    }
}

impl GameIdSource for SequenceGameIds {
    fn next_game_id(&self) -> GameId {
        let mut next = self.next.lock().expect("test ID mutex is not poisoned");
        let game_id = GameId::new(format!("{}-{next}", self.prefix))
            .expect("test prefix produces a valid game ID");
        *next = next
            .checked_add(1)
            .expect("test ID sequence does not overflow");
        game_id
    }
}

/// Deterministic sequence-backed seed source.
#[derive(Debug)]
pub struct SequenceSeeds {
    next: Mutex<u64>,
}

impl SequenceSeeds {
    /// Starts at one explicit sequence value.
    #[must_use]
    pub const fn new(first: u64) -> Self {
        Self {
            next: Mutex::new(first),
        }
    }
}

impl SeedSource for SequenceSeeds {
    fn next_seed(&self) -> GameSeed {
        let mut next = self.next.lock().expect("test seed mutex is not poisoned");
        let value = *next;
        *next = next
            .checked_add(1)
            .expect("test seed sequence does not overflow");
        let mut bytes = [0_u8; 32];
        for (index, chunk) in bytes.chunks_exact_mut(8).enumerate() {
            chunk.copy_from_slice(&value.wrapping_add(index as u64).to_be_bytes());
        }
        GameSeed::from_bytes(bytes)
    }
}

/// Fixed application clock.
#[derive(Debug)]
pub struct FixedClock(pub UnixMillis);

impl ApplicationClock for FixedClock {
    fn now(&self) -> UnixMillis {
        self.0
    }
}

/// Mutable atomic application clock for expiry/deadline tests.
#[derive(Debug)]
pub struct ManualClock(AtomicI64);

impl ManualClock {
    /// Starts at one explicit Unix-millisecond value.
    #[must_use]
    pub const fn new(now: UnixMillis) -> Self {
        Self(AtomicI64::new(now.0))
    }

    /// Advances or rewinds the test clock explicitly.
    pub fn set(&self, now: UnixMillis) {
        self.0.store(now.0, Ordering::SeqCst);
    }
}

impl ApplicationClock for ManualClock {
    fn now(&self) -> UnixMillis {
        UnixMillis(self.0.load(Ordering::SeqCst))
    }
}
