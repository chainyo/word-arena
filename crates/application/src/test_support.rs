//! Deterministic in-memory adapters for application tests and examples.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
};

use word_arena_engine::{GameSeed, WordValidator};
use word_arena_lexicon::PackIdentity;

use crate::{
    ApplicationClock, BoxFuture, GameId, GameIdSource, GameRepository, LexiconResolver,
    RepositoryError, SeedSource, StoredGame, UnixMillis,
};

/// Thread-safe optimistic in-memory game repository.
#[derive(Debug, Default)]
pub struct InMemoryGameRepository {
    games: Mutex<BTreeMap<GameId, StoredGame>>,
}

impl GameRepository for InMemoryGameRepository {
    fn insert(&self, game: StoredGame) -> BoxFuture<'_, Result<(), RepositoryError>> {
        Box::pin(async move {
            let mut games = self
                .games
                .lock()
                .map_err(|_| RepositoryError::Unavailable)?;
            if games.contains_key(&game.game_id) {
                return Err(RepositoryError::AlreadyExists);
            }
            games.insert(game.game_id.clone(), game);
            Ok(())
        })
    }

    fn load(&self, game_id: &GameId) -> BoxFuture<'_, Result<StoredGame, RepositoryError>> {
        let game_id = game_id.clone();
        Box::pin(async move {
            self.games
                .lock()
                .map_err(|_| RepositoryError::Unavailable)?
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
            let mut games = self
                .games
                .lock()
                .map_err(|_| RepositoryError::Unavailable)?;
            let current = games.get(&game.game_id).ok_or(RepositoryError::NotFound)?;
            if current.snapshot.state.version != expected_version {
                return Err(RepositoryError::Conflict);
            }
            games.insert(game.game_id.clone(), game);
            Ok(())
        })
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
