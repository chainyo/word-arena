use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use thiserror::Error;
use word_arena_agent_runtime::{
    AGENT_MANIFEST_SCHEMA_VERSION, AGENT_RUN_RESULT_SCHEMA_VERSION, AgentManifestIdentity,
    MANIFEST_HASH_ALGORITHM, ValidatedAgentManifest,
};
use word_arena_engine::Seat;

/// Run-to-game attribution supplied before process execution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentRunAttribution {
    pub run_id: String,
    pub match_id: Option<String>,
    pub game_id: String,
    pub seat: Seat,
    pub created_at_ms: i64,
}

/// Terminal run outcome persisted independently of later telemetry detail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentRunOutcome {
    Finished,
    Failed,
    Cancelled,
}

impl AgentRunOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Finished => "finished",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Exact run and manifest identity attached to one replay seat.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayAgentAttribution {
    pub seat: Seat,
    pub run_id: String,
    pub manifest: AgentManifestIdentity,
}

/// Persistence error that does not expose SQL, manifest bytes, or credentials.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AgentAttributionError {
    #[error("agent attribution already exists")]
    AlreadyExists,
    #[error("agent attribution input is invalid")]
    Invalid,
    #[error("agent attribution dependency was not found")]
    NotFound,
    #[error("agent attribution conflicts with immutable stored identity")]
    Conflict,
    #[error("stored agent attribution is corrupt or incompatible")]
    Corrupt,
    #[error("agent attribution storage is unavailable")]
    Unavailable,
}

/// `SQLx` adapter that keeps canonical manifests, runs, terminal results, and
/// replay attribution linked by the same immutable digest.
#[derive(Clone, Debug)]
pub struct SqliteAgentAttributionRepository {
    pool: SqlitePool,
}

impl SqliteAgentAttributionRepository {
    #[must_use]
    pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Atomically registers canonical manifest bytes, assigns the game seat,
    /// and creates a pending run carrying the exact manifest identity.
    ///
    /// # Errors
    ///
    /// Fails on invalid IDs/timestamps, missing game/match/seat records, an
    /// occupied seat, or any content-address collision.
    pub async fn create_run(
        &self,
        manifest: &ValidatedAgentManifest,
        run: &AgentRunAttribution,
    ) -> Result<(), AgentAttributionError> {
        validate_run(run)?;
        let mut transaction = self.pool.begin().await.map_err(map_storage)?;
        register_manifest(&mut transaction, manifest, run.created_at_ms).await?;
        let assigned = sqlx::query(
            "UPDATE seats
             SET participant_kind = 'agent', participant_id = ?
             WHERE game_id = ? AND seat_number = ?
               AND (participant_kind = 'unassigned'
                    OR (participant_kind = 'agent' AND participant_id = ?))",
        )
        .bind(&run.run_id)
        .bind(&run.game_id)
        .bind(seat_number(run.seat))
        .bind(&run.run_id)
        .execute(&mut *transaction)
        .await
        .map_err(map_storage)?;
        if assigned.rows_affected() != 1 {
            return Err(AgentAttributionError::Conflict);
        }
        let inserted = sqlx::query(
            "INSERT INTO agent_runs (
                run_id, match_id, game_id, seat_number, manifest_sha256,
                status, started_at_ms, finished_at_ms, created_at_ms
             ) VALUES (?, ?, ?, ?, ?, 'pending', NULL, NULL, ?)",
        )
        .bind(&run.run_id)
        .bind(&run.match_id)
        .bind(&run.game_id)
        .bind(seat_number(run.seat))
        .bind(&manifest.identity().manifest_sha256)
        .bind(run.created_at_ms)
        .execute(&mut *transaction)
        .await;
        if let Err(error) = inserted {
            return Err(map_insert(error));
        }
        transaction.commit().await.map_err(map_storage)
    }

    /// Loads and revalidates the canonical manifest stored for a run.
    ///
    /// # Errors
    ///
    /// Fails closed when the run is missing or stored bytes/hash/schema drift.
    pub async fn load_run_manifest(
        &self,
        run_id: &str,
    ) -> Result<ValidatedAgentManifest, AgentAttributionError> {
        validate_text(run_id)?;
        let row = sqlx::query(
            "SELECT m.schema_version, m.manifest_sha256, m.manifest_json
             FROM agent_runs AS r
             JOIN agent_manifests AS m
               ON m.manifest_sha256 = r.manifest_sha256
             WHERE r.run_id = ?",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_storage)?
        .ok_or(AgentAttributionError::NotFound)?;
        let schema_version: i64 = row
            .try_get("schema_version")
            .map_err(|_| AgentAttributionError::Corrupt)?;
        let stored_sha: String = row
            .try_get("manifest_sha256")
            .map_err(|_| AgentAttributionError::Corrupt)?;
        let bytes: Vec<u8> = row
            .try_get("manifest_json")
            .map_err(|_| AgentAttributionError::Corrupt)?;
        let manifest = ValidatedAgentManifest::from_json(&bytes)
            .map_err(|_| AgentAttributionError::Corrupt)?;
        if schema_version != i64::from(AGENT_MANIFEST_SCHEMA_VERSION)
            || manifest.identity().manifest_sha256 != stored_sha
            || manifest.canonical_json() != bytes
        {
            return Err(AgentAttributionError::Corrupt);
        }
        Ok(manifest)
    }

    /// Atomically records the terminal result with the run's repeated exact
    /// manifest identity.
    ///
    /// # Errors
    ///
    /// Fails when the supplied manifest differs, the run is already terminal,
    /// or the timestamp predates run creation.
    pub async fn record_result(
        &self,
        run_id: &str,
        manifest: &AgentManifestIdentity,
        outcome: AgentRunOutcome,
        completed_at_ms: i64,
    ) -> Result<(), AgentAttributionError> {
        validate_text(run_id)?;
        validate_identity(manifest)?;
        if completed_at_ms < 0 {
            return Err(AgentAttributionError::Invalid);
        }
        let mut transaction = self.pool.begin().await.map_err(map_storage)?;
        let updated = sqlx::query(
            "UPDATE agent_runs
             SET status = ?, started_at_ms = COALESCE(started_at_ms, created_at_ms),
                 finished_at_ms = ?
             WHERE run_id = ? AND manifest_sha256 = ?
               AND status IN ('pending', 'starting', 'running')
               AND created_at_ms <= ?",
        )
        .bind(outcome.as_str())
        .bind(completed_at_ms)
        .bind(run_id)
        .bind(&manifest.manifest_sha256)
        .bind(completed_at_ms)
        .execute(&mut *transaction)
        .await
        .map_err(map_storage)?;
        if updated.rows_affected() != 1 {
            return Err(AgentAttributionError::Conflict);
        }
        sqlx::query(
            "INSERT INTO agent_run_results (
                run_id, manifest_sha256, result_schema_version,
                outcome_kind, completed_at_ms
             ) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(run_id)
        .bind(&manifest.manifest_sha256)
        .bind(i64::from(AGENT_RUN_RESULT_SCHEMA_VERSION))
        .bind(outcome.as_str())
        .bind(completed_at_ms)
        .execute(&mut *transaction)
        .await
        .map_err(map_insert)?;
        transaction.commit().await.map_err(map_storage)
    }

    /// Attaches a terminal replay to the exact run and manifest for one seat.
    ///
    /// # Errors
    ///
    /// Database constraints reject cross-game, cross-seat, version, run, or
    /// manifest substitutions.
    pub async fn attach_replay(
        &self,
        game_id: &str,
        version: u64,
        seat: Seat,
        run_id: &str,
        manifest: &AgentManifestIdentity,
    ) -> Result<(), AgentAttributionError> {
        validate_text(game_id)?;
        validate_text(run_id)?;
        validate_identity(manifest)?;
        let version = i64::try_from(version).map_err(|_| AgentAttributionError::Invalid)?;
        sqlx::query(
            "INSERT INTO game_replay_agents (
                game_id, version, seat_number, run_id, manifest_sha256
             ) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(game_id)
        .bind(version)
        .bind(seat_number(seat))
        .bind(run_id)
        .bind(&manifest.manifest_sha256)
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(map_insert)
    }

    /// Returns stable seat-ordered manifest attribution for a replay.
    ///
    /// # Errors
    ///
    /// Fails on invalid input or corrupt stored manifest identity.
    pub async fn replay_attribution(
        &self,
        game_id: &str,
        version: u64,
    ) -> Result<Vec<ReplayAgentAttribution>, AgentAttributionError> {
        validate_text(game_id)?;
        let version = i64::try_from(version).map_err(|_| AgentAttributionError::Invalid)?;
        let rows = sqlx::query(
            "SELECT seat_number, run_id, manifest_sha256
             FROM game_replay_agents
             WHERE game_id = ? AND version = ?
             ORDER BY seat_number",
        )
        .bind(game_id)
        .bind(version)
        .fetch_all(&self.pool)
        .await
        .map_err(map_storage)?;
        rows.into_iter()
            .map(|row| {
                let seat = match row.try_get::<i64, _>("seat_number") {
                    Ok(1) => Seat::One,
                    Ok(2) => Seat::Two,
                    _ => return Err(AgentAttributionError::Corrupt),
                };
                let manifest_sha256: String = row
                    .try_get("manifest_sha256")
                    .map_err(|_| AgentAttributionError::Corrupt)?;
                if manifest_sha256.len() != 64 {
                    return Err(AgentAttributionError::Corrupt);
                }
                Ok(ReplayAgentAttribution {
                    seat,
                    run_id: row
                        .try_get("run_id")
                        .map_err(|_| AgentAttributionError::Corrupt)?,
                    manifest: AgentManifestIdentity {
                        schema_version: AGENT_MANIFEST_SCHEMA_VERSION,
                        hash_algorithm: MANIFEST_HASH_ALGORITHM.to_owned(),
                        manifest_sha256,
                    },
                })
            })
            .collect()
    }
}

async fn register_manifest(
    transaction: &mut Transaction<'_, Sqlite>,
    manifest: &ValidatedAgentManifest,
    created_at_ms: i64,
) -> Result<(), AgentAttributionError> {
    sqlx::query(
        "INSERT INTO agent_manifests (
            manifest_sha256, schema_version, manifest_json, created_at_ms
         ) VALUES (?, ?, ?, ?)
         ON CONFLICT (manifest_sha256) DO NOTHING",
    )
    .bind(&manifest.identity().manifest_sha256)
    .bind(i64::from(manifest.identity().schema_version))
    .bind(manifest.canonical_json())
    .bind(created_at_ms)
    .execute(&mut **transaction)
    .await
    .map_err(map_storage)?;
    let row = sqlx::query(
        "SELECT schema_version, manifest_json FROM agent_manifests
         WHERE manifest_sha256 = ?",
    )
    .bind(&manifest.identity().manifest_sha256)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_storage)?;
    let version: i64 = row
        .try_get("schema_version")
        .map_err(|_| AgentAttributionError::Corrupt)?;
    let bytes: Vec<u8> = row
        .try_get("manifest_json")
        .map_err(|_| AgentAttributionError::Corrupt)?;
    if version != i64::from(manifest.identity().schema_version)
        || bytes != manifest.canonical_json()
    {
        return Err(AgentAttributionError::Conflict);
    }
    Ok(())
}

fn validate_run(run: &AgentRunAttribution) -> Result<(), AgentAttributionError> {
    validate_text(&run.run_id)?;
    validate_text(&run.game_id)?;
    if let Some(match_id) = &run.match_id {
        validate_text(match_id)?;
    }
    if run.created_at_ms < 0 {
        return Err(AgentAttributionError::Invalid);
    }
    Ok(())
}

fn validate_identity(identity: &AgentManifestIdentity) -> Result<(), AgentAttributionError> {
    if identity.schema_version != AGENT_MANIFEST_SCHEMA_VERSION
        || identity.hash_algorithm != MANIFEST_HASH_ALGORITHM
        || identity.manifest_sha256.len() != 64
        || !identity
            .manifest_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Err(AgentAttributionError::Invalid)
    } else {
        Ok(())
    }
}

fn validate_text(value: &str) -> Result<(), AgentAttributionError> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        Err(AgentAttributionError::Invalid)
    } else {
        Ok(())
    }
}

const fn seat_number(seat: Seat) -> i64 {
    match seat {
        Seat::One => 1,
        Seat::Two => 2,
    }
}

fn map_insert(error: sqlx::Error) -> AgentAttributionError {
    if let sqlx::Error::Database(database) = &error {
        if database.is_unique_violation() {
            return AgentAttributionError::AlreadyExists;
        }
        if database.is_foreign_key_violation() {
            return AgentAttributionError::Conflict;
        }
    }
    map_storage(error)
}

fn map_storage(_error: sqlx::Error) -> AgentAttributionError {
    AgentAttributionError::Unavailable
}
