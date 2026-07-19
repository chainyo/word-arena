use std::sync::Arc;

use tempfile::TempDir;
use word_arena_application::{GameId, GameRepository, RepositoryError, StoredGame, UnixMillis};
use word_arena_engine::{
    Coordinate, Game, GamePhase, GameSeed, Move, PhysicalTile, Placement, Ruleset, Seat, Tile,
    TileFace, WordValidator,
};
use word_arena_lexicon::{NormalizedKey, PackIdentity};
use word_arena_persistence::{SqliteGameRepository, connect_and_migrate};

#[derive(Debug)]
struct AcceptingLexicon(PackIdentity);

impl WordValidator for AcceptingLexicon {
    fn identity(&self) -> &PackIdentity {
        &self.0
    }

    fn contains(&self, _key: &NormalizedKey) -> bool {
        true
    }
}

#[tokio::test]
async fn insert_load_and_metadata_are_exact() {
    let database = Database::open("insert-load").await;
    let (ruleset, _lexicon, game, record) = fixture("game");
    database.repository.insert(record.clone()).await.unwrap();
    assert_eq!(
        database.repository.load(&record.game_id).await.unwrap(),
        record
    );
    assert_eq!(
        database.repository.insert(record.clone()).await,
        Err(RepositoryError::AlreadyExists)
    );
    assert_eq!(
        database
            .repository
            .load(&GameId::new("missing").unwrap())
            .await,
        Err(RepositoryError::NotFound)
    );

    let ruleset_bytes = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT definition_json FROM rulesets WHERE ruleset_id = ?",
    )
    .bind(ruleset.id.as_str())
    .fetch_one(database.repository.pool())
    .await
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<Ruleset>(&ruleset_bytes).unwrap(),
        ruleset
    );
    let identity_bytes =
        sqlx::query_scalar::<_, Vec<u8>>("SELECT identity_json FROM lexicon_packs")
            .fetch_one(database.repository.pool())
            .await
            .unwrap();
    assert_eq!(
        serde_json::from_slice::<PackIdentity>(&identity_bytes).unwrap(),
        game.public_state().lexicon
    );
}

#[tokio::test]
async fn concurrent_writers_have_one_winner_and_one_conflict() {
    let database = Database::open("concurrency").await;
    let (ruleset, lexicon, game, initial) = fixture("race");
    database.repository.insert(initial.clone()).await.unwrap();
    let mut passed =
        Game::resume(game.snapshot(), ruleset.clone(), Some(Arc::clone(&lexicon))).unwrap();
    passed.pass(Seat::One, 0).unwrap();
    let mut resigned = Game::resume(game.snapshot(), ruleset, Some(lexicon)).unwrap();
    resigned.resign(Seat::One, 0).unwrap();

    let pass_record = record(&passed, &initial.game_id, UnixMillis(1_001));
    let resign_record = record(&resigned, &initial.game_id, UnixMillis(1_002));
    let (pass_result, resign_result) = tokio::join!(
        database.repository.replace(0, pass_record.clone()),
        database.repository.replace(0, resign_record.clone())
    );
    assert!(matches!(
        (&pass_result, &resign_result),
        (Ok(()), Err(RepositoryError::Conflict)) | (Err(RepositoryError::Conflict), Ok(()))
    ));
    let stored = database.repository.load(&initial.game_id).await.unwrap();
    let expected = if pass_result.is_ok() {
        pass_record
    } else {
        resign_record
    };
    assert_eq!(stored, expected);
    assert_eq!(
        row_count(database.repository.pool(), "public_events").await,
        2
    );
    assert_eq!(
        row_count(database.repository.pool(), "game_snapshots").await,
        2
    );
}

#[tokio::test]
async fn failed_event_append_rolls_back_version_and_snapshot() {
    let database = Database::open("rollback").await;
    let (ruleset, lexicon, game, initial) = fixture("rollback");
    database.repository.insert(initial.clone()).await.unwrap();
    let mut candidate = Game::resume(game.snapshot(), ruleset, Some(lexicon)).unwrap();
    candidate.pass(Seat::One, 0).unwrap();
    let next = record(&candidate, &initial.game_id, UnixMillis(1_001));
    let event_bytes = serde_json::to_vec(&next.snapshot.events[1]).unwrap();
    sqlx::query(
        "INSERT INTO public_events (
            game_id, sequence, event_schema_version, payload_json, committed_at_ms
         ) VALUES (?, 1, 1, ?, 1001)",
    )
    .bind(initial.game_id.as_str())
    .bind(event_bytes)
    .execute(database.repository.pool())
    .await
    .unwrap();

    assert_eq!(
        database.repository.replace(0, next).await,
        Err(RepositoryError::Corrupt)
    );
    let version = sqlx::query_scalar::<_, i64>("SELECT version FROM games WHERE game_id = ?")
        .bind(initial.game_id.as_str())
        .fetch_one(database.repository.pool())
        .await
        .unwrap();
    assert_eq!(version, 0);
    let future_snapshots = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM game_snapshots WHERE game_id = ? AND version = 1",
    )
    .bind(initial.game_id.as_str())
    .fetch_one(database.repository.pool())
    .await
    .unwrap();
    assert_eq!(future_snapshots, 0);
}

#[tokio::test]
async fn restart_resumes_and_replays_complete_public_and_private_history() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("restart.sqlite3");
    let url = database_url(&path);
    let pool = connect_and_migrate(&url).await.unwrap();
    let repository = SqliteGameRepository::new(pool.clone());
    let (ruleset, lexicon, mut game, initial) = fixture("restart");
    repository.insert(initial.clone()).await.unwrap();

    let opening = game
        .rack(Seat::One)
        .tiles()
        .iter()
        .take(2)
        .enumerate()
        .map(|(index, tile)| {
            Placement::new(
                tile.id,
                Coordinate::new(7, 6 + u8::try_from(index).unwrap()),
                assignment(tile, index),
            )
        })
        .collect();
    game.apply_move(
        Seat::One,
        0,
        Move::Place {
            placements: opening,
        },
    )
    .unwrap();
    persist_version(&repository, &game, &initial.game_id).await;
    let exchange_id = game.rack(Seat::Two).tiles()[0].id;
    game.apply_move(
        Seat::Two,
        1,
        Move::Exchange {
            tile_ids: vec![exchange_id],
        },
    )
    .unwrap();
    persist_version(&repository, &game, &initial.game_id).await;
    while game.public_state().phase == GamePhase::Active {
        let version = game.public_state().version;
        let seat = game.public_state().current_player;
        game.pass(seat, version).unwrap();
        persist_version(&repository, &game, &initial.game_id).await;
    }
    let expected = game.snapshot();
    let replay = game.replay_bundle().unwrap();
    pool.close().await;

    let restarted_pool = connect_and_migrate(&url).await.unwrap();
    let restarted = SqliteGameRepository::new(restarted_pool);
    let stored = restarted.load(&initial.game_id).await.unwrap();
    assert_eq!(stored.snapshot, expected);
    let resumed =
        Game::resume(stored.snapshot, ruleset.clone(), Some(Arc::clone(&lexicon))).unwrap();
    assert_eq!(resumed.snapshot(), expected);
    assert_eq!(
        Game::replay(&replay, Some(lexicon)).unwrap().snapshot(),
        expected
    );
    assert_eq!(
        row_count(restarted.pool(), "public_events").await,
        i64::try_from(expected.state.version).unwrap() + 1
    );
    assert_eq!(row_count(restarted.pool(), "private_events").await, 2);
}

#[tokio::test]
async fn schema_pack_history_corruption_and_closed_pool_are_distinct() {
    let schema_database = Database::open("schema-corrupt").await;
    let (_, _, _, schema_record) = fixture("schema-corrupt");
    schema_database
        .repository
        .insert(schema_record.clone())
        .await
        .unwrap();
    let mut snapshot_json = serde_json::to_value(&schema_record.snapshot).unwrap();
    snapshot_json["schema_version"] = serde_json::json!(999);
    sqlx::query("UPDATE game_snapshots SET payload_json = ? WHERE game_id = ?")
        .bind(serde_json::to_vec(&snapshot_json).unwrap())
        .bind(schema_record.game_id.as_str())
        .execute(schema_database.repository.pool())
        .await
        .unwrap();
    assert_eq!(
        schema_database
            .repository
            .load(&schema_record.game_id)
            .await,
        Err(RepositoryError::IncompatibleSchema)
    );

    let pack_database = Database::open("pack-corrupt").await;
    let (_, _, _, pack_record) = fixture("pack-corrupt");
    pack_database
        .repository
        .insert(pack_record.clone())
        .await
        .unwrap();
    let mut other_identity = pack_record.snapshot.state.lexicon.clone();
    other_identity.pack_version = "999.0.0".to_owned();
    sqlx::query("UPDATE lexicon_packs SET identity_json = ?")
        .bind(serde_json::to_vec(&other_identity).unwrap())
        .execute(pack_database.repository.pool())
        .await
        .unwrap();
    assert_eq!(
        pack_database.repository.load(&pack_record.game_id).await,
        Err(RepositoryError::IncompatiblePack)
    );

    let history_database = Database::open("history-corrupt").await;
    let (_, _, _, history_record) = fixture("history-corrupt");
    history_database
        .repository
        .insert(history_record.clone())
        .await
        .unwrap();
    sqlx::query("DELETE FROM public_events WHERE game_id = ?")
        .bind(history_record.game_id.as_str())
        .execute(history_database.repository.pool())
        .await
        .unwrap();
    assert_eq!(
        history_database
            .repository
            .load(&history_record.game_id)
            .await,
        Err(RepositoryError::Corrupt)
    );
    history_database.repository.pool().close().await;
    assert_eq!(
        history_database
            .repository
            .load(&history_record.game_id)
            .await,
        Err(RepositoryError::Unavailable)
    );
}

struct Database {
    _directory: TempDir,
    repository: SqliteGameRepository,
}

impl Database {
    async fn open(name: &str) -> Self {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join(format!("{name}.sqlite3"));
        let pool = connect_and_migrate(&database_url(&path)).await.unwrap();
        Self {
            _directory: directory,
            repository: SqliteGameRepository::new(pool),
        }
    }
}

fn fixture(id: &str) -> (Ruleset, Arc<dyn WordValidator>, Game, StoredGame) {
    let ruleset = Ruleset::english_v1();
    let lexicon: Arc<dyn WordValidator> = Arc::new(AcceptingLexicon(ruleset.lexicon.clone()));
    let game_id = GameId::new(id).unwrap();
    let game = Game::create(
        game_id.as_str(),
        ruleset.clone(),
        Some(Arc::clone(&lexicon)),
        numbered_seed(7),
    )
    .unwrap();
    let record = StoredGame {
        game_id,
        created_at: UnixMillis(1_000),
        updated_at: UnixMillis(1_000),
        snapshot: game.snapshot(),
    };
    (ruleset, lexicon, game, record)
}

fn record(game: &Game, game_id: &GameId, updated_at: UnixMillis) -> StoredGame {
    StoredGame {
        game_id: game_id.clone(),
        created_at: UnixMillis(1_000),
        updated_at,
        snapshot: game.snapshot(),
    }
}

async fn persist_version(repository: &SqliteGameRepository, game: &Game, game_id: &GameId) {
    let expected = game.public_state().version - 1;
    repository
        .replace(
            expected,
            record(
                game,
                game_id,
                UnixMillis(1_000 + i64::try_from(game.public_state().version).unwrap()),
            ),
        )
        .await
        .unwrap();
}

fn assignment(tile: &PhysicalTile, index: usize) -> Tile {
    match &tile.face {
        TileFace::Letter(token) => Tile::letter(token.as_str()),
        TileFace::Blank => Tile::blank(if index == 0 { "A" } else { "B" }),
    }
}

fn numbered_seed(number: u64) -> GameSeed {
    let mut seed = [0_u8; 32];
    for (index, chunk) in seed.chunks_exact_mut(8).enumerate() {
        chunk.copy_from_slice(&number.wrapping_add(index as u64).to_be_bytes());
    }
    GameSeed::from_bytes(seed)
}

fn database_url(path: &std::path::Path) -> String {
    format!("sqlite://{}", path.display())
}

async fn row_count(pool: &sqlx::SqlitePool, table: &str) -> i64 {
    let query = match table {
        "game_snapshots" => "SELECT COUNT(*) FROM game_snapshots",
        "private_events" => "SELECT COUNT(*) FROM private_events",
        "public_events" => "SELECT COUNT(*) FROM public_events",
        _ => panic!("unsupported test table"),
    };
    sqlx::query_scalar::<_, i64>(query)
        .fetch_one(pool)
        .await
        .unwrap()
}
