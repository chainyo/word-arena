use std::{fs, path::PathBuf, sync::Arc};

use serde_json::Value;
use tempfile::TempDir;
use word_arena_agent_runtime::{
    BudgetController, DiagnosticRecord, DiagnosticStream, DriverClock, DriverLifecycleState,
    DriverTelemetry, LifecycleTransition, NetworkPolicy, PlatformBudgetCapabilities,
    RunTelemetryArchive, RunTelemetryCorrelation, RunUsageTelemetry, TelemetryRetentionPolicy,
    TelemetrySanitizer, TurnTelemetry, UnenforcedBudgetPolicy, ValidatedAgentManifest,
    VisibleToolCall,
};
use word_arena_engine::Seat;
use word_arena_persistence::{
    AgentAttributionError, AgentRunAttribution, AgentRunOutcome, SqliteAgentAttributionRepository,
    connect_and_migrate,
};

struct Database {
    _directory: TempDir,
    pool: sqlx::SqlitePool,
    repository: SqliteAgentAttributionRepository,
}

#[derive(Debug)]
struct BudgetClock;

impl DriverClock for BudgetClock {
    fn now_unix_ms(&self) -> i64 {
        19
    }
}

impl Database {
    async fn open(name: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("{name}.sqlite3"));
        let pool = connect_and_migrate(&format!("sqlite://{}", path.display()))
            .await
            .unwrap();
        seed_game(&pool, "game-one").await;
        Self {
            _directory: directory,
            repository: SqliteAgentAttributionRepository::new(pool.clone()),
            pool,
        }
    }
}

fn manifest(name: &str) -> ValidatedAgentManifest {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/agents/codex-v1.json");
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    value["name"] = name.into();
    ValidatedAgentManifest::from_json(&serde_json::to_vec(&value).unwrap()).unwrap()
}

fn run(run_id: &str, seat: Seat) -> AgentRunAttribution {
    AgentRunAttribution {
        run_id: run_id.into(),
        match_id: None,
        game_id: "game-one".into(),
        seat,
        created_at_ms: 10,
    }
}

#[tokio::test]
async fn canonical_manifest_is_registered_once_and_loaded_exactly() {
    let database = Database::open("manifest-round-trip").await;
    let manifest = manifest("Codex one");
    database
        .repository
        .create_run(&manifest, &run("run-one", Seat::One))
        .await
        .unwrap();

    assert_eq!(
        database
            .repository
            .load_run_manifest("run-one")
            .await
            .unwrap(),
        manifest
    );
    let bytes = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT manifest_json FROM agent_manifests WHERE manifest_sha256 = ?",
    )
    .bind(&manifest.identity().manifest_sha256)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(bytes, manifest.canonical_json());
    assert!(!String::from_utf8(bytes).unwrap().contains("secret"));
}

#[tokio::test]
async fn run_result_and_replay_repeat_the_exact_manifest_identity() {
    let database = Database::open("result-replay").await;
    let manifest = manifest("Codex one");
    database
        .repository
        .create_run(&manifest, &run("run-one", Seat::One))
        .await
        .unwrap();
    database
        .repository
        .record_result(
            "run-one",
            manifest.identity(),
            AgentRunOutcome::Finished,
            20,
        )
        .await
        .unwrap();
    insert_replay(&database.pool, "game-one", 1).await;
    database
        .repository
        .attach_replay("game-one", 1, Seat::One, "run-one", manifest.identity())
        .await
        .unwrap();

    let result_sha = sqlx::query_scalar::<_, String>(
        "SELECT manifest_sha256 FROM agent_run_results WHERE run_id = 'run-one'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(result_sha, manifest.identity().manifest_sha256);
    assert_eq!(
        database
            .repository
            .replay_attribution("game-one", 1)
            .await
            .unwrap(),
        [word_arena_persistence::ReplayAgentAttribution {
            seat: Seat::One,
            run_id: "run-one".into(),
            manifest: manifest.identity().clone(),
        }]
    );
}

#[tokio::test]
async fn terminal_budget_telemetry_round_trips_with_exact_run_identity() {
    let database = Database::open("budget-telemetry").await;
    let manifest = manifest("Codex budget");
    database
        .repository
        .create_run(&manifest, &run("run-one", Seat::One))
        .await
        .unwrap();
    let controller = BudgetController::new(
        manifest.manifest().budgets.clone(),
        PlatformBudgetCapabilities::detect(&NetworkPolicy::Deny),
        UnenforcedBudgetPolicy::AllowReported,
        Arc::new(BudgetClock),
    )
    .unwrap();
    controller.begin_attempt().unwrap();
    controller.record_tool_calls(1).unwrap();
    let telemetry = controller.snapshot().unwrap();
    assert_eq!(
        database
            .repository
            .record_budget_telemetry("run-one", manifest.identity(), &telemetry, 20,)
            .await,
        Err(AgentAttributionError::Conflict),
        "a nonterminal run cannot accept final budget telemetry"
    );
    database
        .repository
        .record_result(
            "run-one",
            manifest.identity(),
            AgentRunOutcome::Finished,
            20,
        )
        .await
        .unwrap();
    database
        .repository
        .record_budget_telemetry("run-one", manifest.identity(), &telemetry, 20)
        .await
        .unwrap();
    assert_eq!(
        database
            .repository
            .load_budget_telemetry("run-one")
            .await
            .unwrap(),
        telemetry
    );
    assert_eq!(
        database
            .repository
            .record_budget_telemetry("run-one", manifest.identity(), &telemetry, 21)
            .await,
        Err(AgentAttributionError::AlreadyExists)
    );

    sqlx::query(
        "UPDATE agent_run_budget_telemetry SET telemetry_schema_version = 99
         WHERE run_id = 'run-one'",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    assert_eq!(
        database.repository.load_budget_telemetry("run-one").await,
        Err(AgentAttributionError::Corrupt)
    );
}

#[tokio::test]
async fn sanitized_run_telemetry_round_trips_restarts_exports_and_expires() {
    let database = Database::open("run-telemetry").await;
    seed_match(&database.pool, "tournament-one", "match-one", "game-one").await;
    let manifest = manifest("Codex telemetry");
    let mut attribution = run("run-one", Seat::One);
    attribution.match_id = Some("match-one".to_owned());
    database
        .repository
        .create_run(&manifest, &attribution)
        .await
        .unwrap();
    let secret = "synthetic-seat-secret";
    let archive = run_telemetry(manifest.identity(), secret);
    assert_eq!(
        database.repository.record_run_telemetry(&archive, 21).await,
        Err(AgentAttributionError::Conflict),
        "nonterminal runs cannot accept final telemetry"
    );
    database
        .repository
        .record_result(
            "run-one",
            manifest.identity(),
            AgentRunOutcome::Finished,
            20,
        )
        .await
        .unwrap();

    let budget_controller = BudgetController::new(
        manifest.manifest().budgets.clone(),
        PlatformBudgetCapabilities::detect(&NetworkPolicy::Deny),
        UnenforcedBudgetPolicy::AllowReported,
        Arc::new(BudgetClock),
    )
    .unwrap();
    database
        .repository
        .record_budget_telemetry(
            "run-one",
            manifest.identity(),
            &budget_controller.snapshot().unwrap(),
            20,
        )
        .await
        .unwrap();

    database
        .repository
        .record_run_telemetry(&archive, 21)
        .await
        .unwrap();
    let stored = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT telemetry_json FROM agent_run_telemetry WHERE run_id = 'run-one'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert!(!String::from_utf8(stored).unwrap().contains(secret));
    assert_eq!(
        database.repository.record_run_telemetry(&archive, 21).await,
        Err(AgentAttributionError::AlreadyExists)
    );

    let restarted = SqliteAgentAttributionRepository::new(database.pool.clone());
    let loaded = restarted.load_run_telemetry("run-one").await.unwrap();
    assert_eq!(loaded, archive);
    assert_telemetry_column_drift_is_rejected(&database.pool, &restarted).await;

    let public_json =
        serde_json::to_string(&restarted.public_run_telemetry("run-one").await.unwrap()).unwrap();
    assert_private_and_public_telemetry_privacy(&loaded, &public_json, secret);

    let mut substituted = archive.clone();
    substituted.correlation.game_id = "other-game".to_owned();
    assert_eq!(
        database
            .repository
            .record_run_telemetry(&substituted, 21)
            .await,
        Err(AgentAttributionError::Conflict)
    );

    assert_eq!(restarted.purge_expired_run_telemetry(49).await.unwrap(), 0);
    assert_eq!(restarted.purge_expired_run_telemetry(50).await.unwrap(), 1);
    assert_eq!(
        restarted.load_run_telemetry("run-one").await,
        Err(AgentAttributionError::NotFound)
    );
    assert_eq!(
        restarted.load_budget_telemetry("run-one").await,
        Err(AgentAttributionError::NotFound)
    );
}

#[tokio::test]
async fn identity_cross_run_seat_and_replay_substitution_fail_closed() {
    let database = Database::open("substitution").await;
    let first = manifest("Codex one");
    let second = manifest("Codex two");
    database
        .repository
        .create_run(&first, &run("run-one", Seat::One))
        .await
        .unwrap();
    database
        .repository
        .create_run(&second, &run("run-two", Seat::Two))
        .await
        .unwrap();
    assert_eq!(
        database
            .repository
            .record_result("run-one", second.identity(), AgentRunOutcome::Finished, 20)
            .await,
        Err(AgentAttributionError::Conflict)
    );
    let mut malformed_identity = first.identity().clone();
    malformed_identity.hash_algorithm = "sha512".into();
    assert_eq!(
        database
            .repository
            .record_result(
                "run-one",
                &malformed_identity,
                AgentRunOutcome::Finished,
                20
            )
            .await,
        Err(AgentAttributionError::Invalid)
    );
    insert_replay(&database.pool, "game-one", 1).await;
    assert_eq!(
        database
            .repository
            .attach_replay("game-one", 1, Seat::Two, "run-one", first.identity())
            .await,
        Err(AgentAttributionError::Conflict)
    );
    assert_eq!(
        database
            .repository
            .attach_replay("game-one", 1, Seat::One, "run-one", second.identity())
            .await,
        Err(AgentAttributionError::Conflict)
    );
}

#[tokio::test]
async fn failed_run_creation_rolls_back_the_seat_assignment() {
    let database = Database::open("create-rollback").await;
    let manifest = manifest("Codex one");
    let mut invalid = run("run-one", Seat::One);
    invalid.match_id = Some("missing-match".into());
    assert_eq!(
        database.repository.create_run(&manifest, &invalid).await,
        Err(AgentAttributionError::Conflict)
    );
    let seat = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT participant_kind, participant_id
         FROM seats WHERE game_id = 'game-one' AND seat_number = 1",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(seat, ("unassigned".into(), None));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM agent_manifests")
            .fetch_one(&database.pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn stored_manifest_drift_is_detected_on_load() {
    let database = Database::open("stored-drift").await;
    let manifest = manifest("Codex one");
    database
        .repository
        .create_run(&manifest, &run("run-one", Seat::One))
        .await
        .unwrap();
    sqlx::query("UPDATE agent_manifests SET manifest_json = ? WHERE manifest_sha256 = ?")
        .bind(b"{}".as_slice())
        .bind(&manifest.identity().manifest_sha256)
        .execute(&database.pool)
        .await
        .unwrap();
    assert_eq!(
        database.repository.load_run_manifest("run-one").await,
        Err(AgentAttributionError::Corrupt)
    );
}

async fn assert_telemetry_column_drift_is_rejected(
    pool: &sqlx::SqlitePool,
    repository: &SqliteAgentAttributionRepository,
) {
    sqlx::query(
        "UPDATE agent_run_telemetry SET redaction_policy_version = 99
         WHERE run_id = 'run-one'",
    )
    .execute(pool)
    .await
    .unwrap();
    assert_eq!(
        repository.load_run_telemetry("run-one").await,
        Err(AgentAttributionError::Corrupt)
    );
    sqlx::query(
        "UPDATE agent_run_telemetry SET redaction_policy_version = 1
         WHERE run_id = 'run-one'",
    )
    .execute(pool)
    .await
    .unwrap();
}

fn assert_private_and_public_telemetry_privacy(
    private: &RunTelemetryArchive,
    public_json: &str,
    secret: &str,
) {
    let private_json = serde_json::to_string(private).unwrap();
    assert!(!private_json.contains(secret));
    assert!(private_json.contains("Private rack ETE"));
    for forbidden in [
        secret,
        "Private rack ETE",
        "visible_input",
        "visible_output",
        "arguments",
        "result",
        "visible_text",
    ] {
        assert!(!public_json.contains(forbidden));
    }
    for correlation in [
        "tournament-one",
        "match-one",
        "game-one",
        "run-one",
        "turn-one",
    ] {
        assert!(public_json.contains(correlation));
    }
}

async fn seed_game(pool: &sqlx::SqlitePool, game_id: &str) {
    let ruleset_sha = "c".repeat(64);
    let lexicon_sha = "d".repeat(64);
    sqlx::query(
        "INSERT INTO rulesets (
            ruleset_id, schema_version, content_sha256, definition_json, created_at_ms
         ) VALUES ('english-v1', 1, ?, x'7b7d', 1)",
    )
    .bind(&ruleset_sha)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO lexicon_packs (
            pack_id, pack_version, content_sha256, format_version,
            normalization_version, locale, identity_json, installed_at_ms
         ) VALUES ('pack', '1.0.0', ?, 1, 1, 'en', x'7b7d', 1)",
    )
    .bind(&lexicon_sha)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO games (
            game_id, status, version, ruleset_id, ruleset_sha256,
            lexicon_pack_id, lexicon_pack_version, lexicon_content_sha256,
            rng_algorithm, seed_commitment_sha256, created_at_ms, updated_at_ms
         ) VALUES (?, 'finished', 1, 'english-v1', ?, 'pack', '1.0.0', ?,
                   'xoshiro256-star-star-v1', ?, 1, 2)",
    )
    .bind(game_id)
    .bind(ruleset_sha)
    .bind(lexicon_sha)
    .bind("e".repeat(64))
    .execute(pool)
    .await
    .unwrap();
    for seat in [1_i64, 2] {
        sqlx::query(
            "INSERT INTO seats (
                game_id, seat_number, participant_kind, created_at_ms
             ) VALUES (?, ?, 'unassigned', 1)",
        )
        .bind(game_id)
        .bind(seat)
        .execute(pool)
        .await
        .unwrap();
    }
}

fn run_telemetry(
    manifest: &word_arena_agent_runtime::AgentManifestIdentity,
    secret: &str,
) -> RunTelemetryArchive {
    let driver = DriverTelemetry {
        schema_version: 1,
        run_id: "run-one".to_owned(),
        manifest: manifest.clone(),
        restarts: 1,
        lifecycle: vec![LifecycleTransition {
            sequence: 0,
            at_unix_ms: 10,
            state: DriverLifecycleState::Ready,
        }],
        turns: vec![TurnTelemetry {
            turn_id: "turn-one".to_owned(),
            started_at_unix_ms: 11,
            completed_at_unix_ms: 15,
            visible_input: format!("Private rack ETE {secret}"),
            visible_output: "Placed ETE".to_owned(),
            tool_calls: vec![VisibleToolCall {
                tool: "word_arena.place_tiles".to_owned(),
                arguments: serde_json::json!({"token": secret, "tiles": "ETE"}),
                result: serde_json::json!({"accepted": true}),
            }],
        }],
        diagnostics: vec![DiagnosticRecord {
            sequence: 0,
            at_unix_ms: 16,
            stream: DiagnosticStream::Driver,
            code: "retry".to_owned(),
            visible_text: secret.to_owned(),
        }],
    };
    RunTelemetryArchive::capture(
        RunTelemetryCorrelation {
            tournament_id: Some("tournament-one".to_owned()),
            match_id: Some("match-one".to_owned()),
            game_id: "game-one".to_owned(),
            run_id: "run-one".to_owned(),
            seat_number: 1,
        },
        manifest.clone(),
        &driver,
        RunUsageTelemetry::unavailable("provider_omitted").unwrap(),
        TelemetryRetentionPolicy::expire_at(50),
        20,
        &TelemetrySanitizer::new([secret.as_bytes().to_vec()]),
    )
    .unwrap()
}

async fn seed_match(pool: &sqlx::SqlitePool, tournament_id: &str, match_id: &str, game_id: &str) {
    sqlx::query(
        "INSERT INTO tournaments (
            tournament_id, schema_version, format_kind, status, config_json,
            created_at_ms, updated_at_ms
         ) VALUES (?, 1, 'round_robin', 'finished', x'7b7d', 1, 2)",
    )
    .bind(tournament_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO matches (
            match_id, tournament_id, sequence, game_id, language,
            ruleset_id, ruleset_sha256, lexicon_pack_id,
            lexicon_pack_version, lexicon_content_sha256, status,
            scheduled_at_ms, started_at_ms, finished_at_ms, created_at_ms
         )
         SELECT ?, ?, 0, ?, 'en', 'english-v1', ruleset_sha256,
                'pack', '1.0.0', lexicon_content_sha256, 'finished', 1, 1, 2, 1
         FROM games WHERE game_id = ?",
    )
    .bind(match_id)
    .bind(tournament_id)
    .bind(game_id)
    .bind(game_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_replay(pool: &sqlx::SqlitePool, game_id: &str, version: i64) {
    sqlx::query(
        "INSERT INTO game_replays (
            game_id, version, replay_schema_version, payload_json, created_at_ms
         ) VALUES (?, ?, 3, x'7b7d', 20)",
    )
    .bind(game_id)
    .bind(version)
    .execute(pool)
    .await
    .unwrap();
}
