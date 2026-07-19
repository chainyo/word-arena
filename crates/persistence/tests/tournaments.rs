use std::collections::BTreeSet;

use tempfile::TempDir;
use word_arena_application::{
    RepositoryError, StoredTournament, SwissProgress, SwissRematchPolicy, SwissStanding,
    TOURNAMENT_FORMAT_SCHEMA_VERSION, TOURNAMENT_LIFECYCLE_SCHEMA_VERSION, TournamentEntrant,
    TournamentFormat, TournamentGameProfile, TournamentLifecycleEvent, TournamentLifecycleState,
    TournamentRepository, TournamentSpec, UnixMillis,
};
use word_arena_lexicon::{NormalizationDescriptor, PackIdentity};
use word_arena_persistence::{SqliteTournamentRepository, connect_and_migrate};

#[tokio::test]
async fn static_schedule_round_trips_and_transitions_atomically() {
    let database = Database::open("round-trip").await;
    let tournament = static_tournament("round-trip");
    database.seed_profiles(&tournament).await;

    database
        .repository
        .insert(tournament.clone())
        .await
        .unwrap();
    let restarted = SqliteTournamentRepository::new(database.pool.clone());
    assert_eq!(restarted.load("round-trip").await.unwrap(), tournament);
    assert_eq!(
        restarted.insert(tournament).await,
        Err(RepositoryError::AlreadyExists)
    );

    let event = restarted
        .transition(
            "round-trip",
            0,
            TournamentLifecycleState::Running,
            UnixMillis(20),
        )
        .await
        .unwrap();
    assert_eq!(event.sequence, 1);
    assert_eq!(event.state, TournamentLifecycleState::Running);
    assert_eq!(
        restarted
            .transition(
                "round-trip",
                0,
                TournamentLifecycleState::Running,
                UnixMillis(21),
            )
            .await,
        Err(RepositoryError::Conflict)
    );
    let loaded = restarted.load("round-trip").await.unwrap();
    assert_eq!(loaded.lifecycle, [scheduled_event(), event]);
}

#[tokio::test]
async fn normalized_schedule_tampering_is_detected_on_load() {
    let database = Database::open("tamper").await;
    let tournament = static_tournament("tamper");
    database.seed_profiles(&tournament).await;
    database
        .repository
        .insert(tournament.clone())
        .await
        .unwrap();
    let game = &tournament.schedule.matches[0];
    let replacement = tournament
        .spec
        .entrants
        .iter()
        .map(|entrant| entrant.entrant_id.as_str())
        .find(|entrant| {
            *entrant != game.seat_one_entrant_id && *entrant != game.seat_two_entrant_id
        })
        .unwrap();
    sqlx::query(
        "UPDATE tournament_match_seats SET entrant_id = ?
         WHERE match_id = ? AND seat_number = 1",
    )
    .bind(replacement)
    .bind(&game.match_id)
    .execute(&database.pool)
    .await
    .unwrap();

    assert_eq!(
        database.repository.load("tamper").await,
        Err(RepositoryError::Corrupt)
    );
}

#[tokio::test]
async fn missing_profile_dependencies_roll_back_the_whole_insert() {
    let database = Database::open("rollback").await;
    let tournament = static_tournament("rollback");

    assert_eq!(
        database.repository.insert(tournament).await,
        Err(RepositoryError::Conflict)
    );
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM tournaments WHERE tournament_id = 'rollback'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn swiss_progress_and_next_round_are_persisted_exactly() {
    let database = Database::open("swiss").await;
    let spec = spec(
        "swiss",
        TournamentFormat::Swiss {
            rounds: 3,
            games_per_series: 1,
            rematches: SwissRematchPolicy::Avoid,
        },
        6,
    );
    let progress = SwissProgress {
        completed_rounds: 0,
        standings: spec
            .entrants
            .iter()
            .map(|entrant| SwissStanding {
                entrant_id: entrant.entrant_id.clone(),
                match_points: 0,
                spread: 0,
                wins: 0,
            })
            .collect(),
        prior_pairings: BTreeSet::new(),
        prior_byes: BTreeSet::new(),
        seat_balance: Vec::new(),
        next_seed_index: 0,
        next_match_sequence: 0,
    };
    let tournament = StoredTournament {
        schedule: spec.schedule_swiss_round(&progress).unwrap(),
        spec,
        swiss_progress: Some(progress),
        lifecycle: vec![scheduled_event()],
    };
    database.seed_profiles(&tournament).await;

    database
        .repository
        .insert(tournament.clone())
        .await
        .unwrap();
    assert_eq!(database.repository.load("swiss").await.unwrap(), tournament);
}

struct Database {
    _directory: TempDir,
    pool: sqlx::SqlitePool,
    repository: SqliteTournamentRepository,
}

impl Database {
    async fn open(label: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("{label}.sqlite3"));
        let pool = connect_and_migrate(&format!("sqlite://{}", path.display()))
            .await
            .unwrap();
        let repository = SqliteTournamentRepository::new(pool.clone());
        Self {
            _directory: directory,
            pool,
            repository,
        }
    }

    async fn seed_profiles(&self, tournament: &StoredTournament) {
        for profile in &tournament.spec.profiles {
            sqlx::query(
                "INSERT INTO rulesets (
                    ruleset_id, schema_version, content_sha256, definition_json, created_at_ms
                 ) VALUES (?, 1, ?, x'7b7d', 1)",
            )
            .bind(&profile.ruleset_id)
            .bind(&profile.ruleset_sha256)
            .execute(&self.pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO lexicon_packs (
                    pack_id, pack_version, content_sha256, format_version,
                    normalization_version, locale, identity_json, installed_at_ms
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, 1)",
            )
            .bind(&profile.lexicon.pack_id)
            .bind(&profile.lexicon.pack_version)
            .bind(&profile.lexicon.content_sha256)
            .bind(i64::from(profile.lexicon.format_version))
            .bind(i64::from(profile.lexicon.normalization.version))
            .bind(&profile.lexicon.locale)
            .bind(serde_json::to_vec(&profile.lexicon).unwrap())
            .execute(&self.pool)
            .await
            .unwrap();
        }
    }
}

fn static_tournament(tournament_id: &str) -> StoredTournament {
    let spec = spec(tournament_id, TournamentFormat::RoundRobin { cycles: 1 }, 6);
    StoredTournament {
        schedule: spec.schedule().unwrap(),
        spec,
        swiss_progress: None,
        lifecycle: vec![scheduled_event()],
    }
}

fn spec(tournament_id: &str, format: TournamentFormat, seed_count: usize) -> TournamentSpec {
    TournamentSpec {
        schema_version: TOURNAMENT_FORMAT_SCHEMA_VERSION,
        tournament_id: tournament_id.to_owned(),
        format,
        entrants: (1..=4)
            .map(|seed| TournamentEntrant {
                entrant_id: format!("agent-{seed}"),
                seed_number: u32::try_from(seed).unwrap(),
                manifest_sha256: None,
            })
            .collect(),
        profiles: vec![profile("en", 'a'), profile("fr", 'b')],
        game_seed_commitments: (1..=seed_count)
            .map(|value| format!("{value:064x}"))
            .collect(),
    }
}

fn profile(language: &str, marker: char) -> TournamentGameProfile {
    TournamentGameProfile {
        language: language.to_owned(),
        ruleset_id: format!("{language}-v1"),
        ruleset_sha256: marker.to_string().repeat(64),
        lexicon: PackIdentity {
            pack_id: format!("word-arena-{language}-v1"),
            pack_version: "1.0.0".to_owned(),
            format_version: 1,
            locale: language.to_owned(),
            normalization: NormalizationDescriptor {
                algorithm: "word-arena-board-key".to_owned(),
                version: 1,
                profile: format!("{language}-v1"),
            },
            content_sha256: marker.to_string().repeat(64),
        },
    }
}

fn scheduled_event() -> TournamentLifecycleEvent {
    TournamentLifecycleEvent {
        schema_version: TOURNAMENT_LIFECYCLE_SCHEMA_VERSION,
        sequence: 0,
        state: TournamentLifecycleState::Scheduled,
        occurred_at: UnixMillis(10),
    }
}
