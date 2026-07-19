use std::{str::FromStr, sync::LazyLock, time::Duration};

use sqlx::{
    SqlSafeStr, SqlitePool,
    migrate::{MigrateError, Migration, MigrationType, Migrator},
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use thiserror::Error;

/// Compile-time embedded, forward-only application migrations.
pub static MIGRATOR: LazyLock<Migrator> = LazyLock::new(|| {
    Migrator::with_migrations(vec![
        Migration::new(
            1,
            "core".into(),
            MigrationType::Simple,
            include_str!("../migrations/0001_core.sql").into_sql_str(),
            false,
        ),
        Migration::new(
            2,
            "operations".into(),
            MigrationType::Simple,
            include_str!("../migrations/0002_operations.sql").into_sql_str(),
            false,
        ),
        Migration::new(
            3,
            "capability agent runs".into(),
            MigrationType::Simple,
            include_str!("../migrations/0003_capability_agent_runs.sql").into_sql_str(),
            false,
        ),
        Migration::new(
            4,
            "reliability".into(),
            MigrationType::Simple,
            include_str!("../migrations/0004_reliability.sql").into_sql_str(),
            false,
        ),
        Migration::new(
            5,
            "agent manifest attribution".into(),
            MigrationType::Simple,
            include_str!("../migrations/0005_agent_manifest_attribution.sql").into_sql_str(),
            false,
        ),
        Migration::new(
            6,
            "agent budget telemetry".into(),
            MigrationType::Simple,
            include_str!("../migrations/0006_agent_budget_telemetry.sql").into_sql_str(),
            false,
        ),
    ])
});

/// `SQLite` connection or migration failure.
#[derive(Debug, Error)]
pub enum MigrationError {
    /// Database URL cannot be parsed as `SQLite` options.
    #[error("invalid SQLite database URL: {0}")]
    InvalidUrl(#[from] sqlx::Error),
    /// Embedded migration validation or execution failed.
    #[error("SQLite migration failed: {0}")]
    Migrate(#[from] MigrateError),
}

/// Opens a bounded `SQLite` pool, enables foreign keys, and applies every
/// embedded migration before returning it.
///
/// # Errors
///
/// Returns a URL, connection, or migration error without exposing a partially
/// initialized pool.
pub async fn connect_and_migrate(database_url: &str) -> Result<SqlitePool, MigrationError> {
    let options = SqliteConnectOptions::from_str(database_url)?
        .create_if_missing(true)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5))
        .journal_mode(SqliteJournalMode::Wal);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;
    migrate(&pool).await?;
    Ok(pool)
}

/// Applies and validates the embedded migrations idempotently.
///
/// # Errors
///
/// Returns when an applied migration checksum changed or a pending migration
/// cannot commit.
pub async fn migrate(pool: &SqlitePool) -> Result<(), MigrateError> {
    MIGRATOR.run(pool).await
}

/// Opens a single-connection temporary database for integration tests.
#[cfg(test)]
pub(crate) async fn temporary_pool(path: &std::path::Path) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sqlx::migrate::Migrator;
    use tempfile::tempdir;

    use super::{connect_and_migrate, migrate, temporary_pool};

    const RULESET_SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const LEXICON_SHA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[tokio::test]
    async fn clean_install_is_idempotent_and_applies_migrations_in_order() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("word-arena.sqlite3");
        let url = format!("sqlite://{}", database.display());
        let pool = connect_and_migrate(&url).await.unwrap();
        migrate(&pool).await.unwrap();

        let versions = sqlx::query_scalar::<_, i64>(
            "SELECT version FROM _sqlx_migrations WHERE success = 1 ORDER BY version",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(versions, [1, 2, 3, 4, 5, 6]);
        let schema_version = sqlx::query_scalar::<_, String>(
            "SELECT value FROM schema_metadata WHERE key = 'application_schema_version'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(schema_version, "6");

        let tables = sqlx::query_scalar::<_, String>(
            "SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        for required in [
            "agent_manifests",
            "agent_run_results",
            "agent_run_budget_telemetry",
            "agent_runs",
            "audit_records",
            "capabilities",
            "creation_idempotency_records",
            "game_snapshots",
            "game_replays",
            "game_replay_agents",
            "games",
            "idempotency_records",
            "invalid_attempt_counters",
            "lexicon_packs",
            "matches",
            "private_events",
            "public_events",
            "rulesets",
            "schema_metadata",
            "seats",
            "tournament_entries",
            "tournaments",
            "turn_deadlines",
        ] {
            assert!(
                tables.iter().any(|table| table == required),
                "missing {required}"
            );
        }
    }

    #[tokio::test]
    async fn relationships_checks_and_secret_digest_shape_reject_corrupt_rows() {
        let directory = tempdir().unwrap();
        let pool = temporary_pool(&directory.path().join("constraints.sqlite3"))
            .await
            .unwrap();
        migrate(&pool).await.unwrap();

        assert_missing_inputs_rejected(&pool).await;
        seed_core_game(&pool).await;

        assert!(
            sqlx::query(
                "INSERT INTO seats (
                game_id, seat_number, participant_kind, created_at_ms
             ) VALUES ('game', 3, 'unassigned', 1)",
            )
            .execute(&pool)
            .await
            .is_err()
        );
        insert_seats(&pool).await;
        assert!(
            sqlx::query(
                "INSERT INTO capabilities (
                capability_id, game_id, seat_number, authority_kind, scopes,
                token_digest, digest_version, issued_at_ms, expires_at_ms
             ) VALUES ('bad-digest', 'game', 1, 'seat', 'play', zeroblob(31), 1, 1, 2)",
            )
            .execute(&pool)
            .await
            .is_err()
        );
        assert!(
            sqlx::query(
                "INSERT INTO capabilities (
                capability_id, game_id, seat_number, authority_kind, scopes,
                token_digest, digest_version, issued_at_ms, expires_at_ms
             ) VALUES (
                'bad-role', 'game', 1, 'administrator', 'admin', zeroblob(32), 1, 1, 2
             )",
            )
            .execute(&pool)
            .await
            .is_err()
        );

        let capability_columns = sqlx::query_scalar::<_, String>(
            "SELECT name FROM pragma_table_info('capabilities') ORDER BY cid",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(
            capability_columns
                .iter()
                .any(|column| column == "token_digest")
        );
        assert!(
            capability_columns
                .iter()
                .any(|column| column == "agent_run_id")
        );
        assert!(!capability_columns.iter().any(|column| column == "token"));
        assert!(
            sqlx::query("DELETE FROM rulesets WHERE ruleset_id = 'english-v1'")
                .execute(&pool)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn failed_migration_rolls_back_its_partial_schema() {
        let directory = tempdir().unwrap();
        let migration_root = directory.path().join("broken-migrations");
        fs::create_dir(&migration_root).unwrap();
        fs::write(
            migration_root.join("0001_broken.sql"),
            "CREATE TABLE should_rollback (id INTEGER PRIMARY KEY) STRICT;\n\
             INSERT INTO table_that_does_not_exist (id) VALUES (1);\n",
        )
        .unwrap();
        let migrator = Migrator::new(migration_root).await.unwrap();
        let pool = temporary_pool(&directory.path().join("rollback.sqlite3"))
            .await
            .unwrap();
        assert!(migrator.run(&pool).await.is_err());
        let table_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'table' AND name = 'should_rollback'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(table_count, 0);
    }

    async fn insert_game(pool: &sqlx::SqlitePool, game_id: &str) {
        sqlx::query(
            "INSERT INTO games (
                game_id, status, version, ruleset_id, ruleset_sha256,
                lexicon_pack_id, lexicon_pack_version, lexicon_content_sha256,
                rng_algorithm, seed_commitment_sha256, created_at_ms, updated_at_ms
             ) VALUES (?, 'active', 0, 'english-v1', ?, 'pack', '1.0.0', ?,
                'rng-v1', ?, 1, 1)",
        )
        .bind(game_id)
        .bind(RULESET_SHA)
        .bind(LEXICON_SHA)
        .bind(RULESET_SHA)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn assert_missing_inputs_rejected(pool: &sqlx::SqlitePool) {
        let result = sqlx::query(
            "INSERT INTO games (
                game_id, status, version, ruleset_id, ruleset_sha256,
                lexicon_pack_id, lexicon_pack_version, lexicon_content_sha256,
                rng_algorithm, seed_commitment_sha256, created_at_ms, updated_at_ms
             ) VALUES (?, 'active', 0, 'english-v1', ?, 'pack', '1.0.0', ?,
                'rng-v1', ?, 1, 1)",
        )
        .bind("missing-inputs")
        .bind(RULESET_SHA)
        .bind(LEXICON_SHA)
        .bind(RULESET_SHA)
        .execute(pool)
        .await;
        assert!(result.is_err());
    }

    async fn seed_core_game(pool: &sqlx::SqlitePool) {
        sqlx::query(
            "INSERT INTO rulesets (
                ruleset_id, schema_version, content_sha256, definition_json, created_at_ms
             ) VALUES ('english-v1', 1, ?, x'7b7d', 1)",
        )
        .bind(RULESET_SHA)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO lexicon_packs (
                pack_id, pack_version, content_sha256, format_version,
                normalization_version, locale, identity_json, installed_at_ms
             ) VALUES ('pack', '1.0.0', ?, 1, 1, 'en', x'7b7d', 1)",
        )
        .bind(LEXICON_SHA)
        .execute(pool)
        .await
        .unwrap();
        insert_game(pool, "game").await;
    }

    async fn insert_seats(pool: &sqlx::SqlitePool) {
        for seat in [1_i64, 2] {
            sqlx::query(
                "INSERT INTO seats (
                    game_id, seat_number, participant_kind, created_at_ms
                 ) VALUES ('game', ?, 'unassigned', 1)",
            )
            .bind(seat)
            .execute(pool)
            .await
            .unwrap();
        }
    }
}
