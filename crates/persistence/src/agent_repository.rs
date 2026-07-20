use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use thiserror::Error;
use word_arena_agent_runtime::{
    AGENT_MANIFEST_SCHEMA_VERSION, AGENT_RUN_RESULT_SCHEMA_VERSION, AgentManifestIdentity,
    BUDGET_CAPABILITY_SCHEMA_VERSION, BUDGET_TELEMETRY_SCHEMA_VERSION, BudgetTelemetry,
    MANIFEST_HASH_ALGORITHM, PublicRunTelemetry, RUN_TELEMETRY_SCHEMA_VERSION, RunTelemetryArchive,
    TELEMETRY_REDACTION_POLICY_VERSION, TelemetryRetentionKind, ValidatedAgentManifest,
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

    /// Persists one final normalized budget snapshot for a terminal run.
    ///
    /// # Errors
    ///
    /// Rejects incompatible schemas, malformed event ordering, identity drift,
    /// nonterminal/missing runs, duplicate snapshots, or unavailable storage.
    pub async fn record_budget_telemetry(
        &self,
        run_id: &str,
        manifest: &AgentManifestIdentity,
        telemetry: &BudgetTelemetry,
        recorded_at_ms: i64,
    ) -> Result<(), AgentAttributionError> {
        validate_text(run_id)?;
        validate_identity(manifest)?;
        validate_budget_telemetry(telemetry)?;
        if recorded_at_ms < 0 {
            return Err(AgentAttributionError::Invalid);
        }
        let bytes = serde_json::to_vec(telemetry).map_err(|_| AgentAttributionError::Invalid)?;
        let inserted = sqlx::query(
            "INSERT INTO agent_run_budget_telemetry (
                run_id, manifest_sha256, capability_schema_version,
                telemetry_schema_version, telemetry_json, recorded_at_ms
             )
             SELECT ?, ?, ?, ?, ?, ?
             FROM agent_run_results
             WHERE run_id = ? AND manifest_sha256 = ? AND completed_at_ms <= ?",
        )
        .bind(run_id)
        .bind(&manifest.manifest_sha256)
        .bind(i64::from(BUDGET_CAPABILITY_SCHEMA_VERSION))
        .bind(i64::from(BUDGET_TELEMETRY_SCHEMA_VERSION))
        .bind(bytes)
        .bind(recorded_at_ms)
        .bind(run_id)
        .bind(&manifest.manifest_sha256)
        .bind(recorded_at_ms)
        .execute(&self.pool)
        .await
        .map_err(map_insert)?;
        if inserted.rows_affected() == 1 {
            Ok(())
        } else {
            Err(AgentAttributionError::Conflict)
        }
    }

    /// Loads and revalidates a terminal run's normalized budget snapshot.
    ///
    /// # Errors
    ///
    /// Rejects invalid run IDs, missing rows, schema drift, malformed JSON, or
    /// corrupt limit-event ordering.
    pub async fn load_budget_telemetry(
        &self,
        run_id: &str,
    ) -> Result<BudgetTelemetry, AgentAttributionError> {
        validate_text(run_id)?;
        let row = sqlx::query(
            "SELECT capability_schema_version, telemetry_schema_version, telemetry_json
             FROM agent_run_budget_telemetry WHERE run_id = ?",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_storage)?
        .ok_or(AgentAttributionError::NotFound)?;
        let capability_version: i64 = row
            .try_get("capability_schema_version")
            .map_err(|_| AgentAttributionError::Corrupt)?;
        let telemetry_version: i64 = row
            .try_get("telemetry_schema_version")
            .map_err(|_| AgentAttributionError::Corrupt)?;
        let bytes: Vec<u8> = row
            .try_get("telemetry_json")
            .map_err(|_| AgentAttributionError::Corrupt)?;
        let telemetry: BudgetTelemetry =
            serde_json::from_slice(&bytes).map_err(|_| AgentAttributionError::Corrupt)?;
        if capability_version != i64::from(BUDGET_CAPABILITY_SCHEMA_VERSION)
            || telemetry_version != i64::from(BUDGET_TELEMETRY_SCHEMA_VERSION)
        {
            return Err(AgentAttributionError::Corrupt);
        }
        validate_budget_telemetry(&telemetry).map_err(|_| AgentAttributionError::Corrupt)?;
        Ok(telemetry)
    }

    /// Persists one final sanitized, bounded telemetry archive for a terminal run.
    ///
    /// # Errors
    ///
    /// Rejects invalid schemas, retention, ordering, exact run/manifest/game/
    /// seat/match/tournament correlation drift, duplicates, or unavailable storage.
    pub async fn record_run_telemetry(
        &self,
        telemetry: &RunTelemetryArchive,
        recorded_at_ms: i64,
    ) -> Result<(), AgentAttributionError> {
        validate_run_telemetry(telemetry)?;
        if recorded_at_ms < telemetry.captured_at_unix_ms
            || telemetry
                .retention
                .expires_at_unix_ms
                .is_some_and(|expires| expires < recorded_at_ms)
        {
            return Err(AgentAttributionError::Invalid);
        }
        let bytes = serde_json::to_vec(telemetry).map_err(|_| AgentAttributionError::Invalid)?;
        let retention = match telemetry.retention.kind {
            TelemetryRetentionKind::Retain => "retain",
            TelemetryRetentionKind::Expire => "expire",
        };
        let correlation = &telemetry.correlation;
        let inserted = sqlx::query(
            "INSERT INTO agent_run_telemetry (
                run_id, manifest_sha256, telemetry_schema_version,
                redaction_policy_version, tournament_id, match_id, game_id,
                seat_number, telemetry_json, retention_kind, expires_at_ms,
                recorded_at_ms
             )
             SELECT r.run_id, r.manifest_sha256, ?, ?, ?, ?, r.game_id,
                    r.seat_number, ?, ?, ?, ?
             FROM agent_runs AS r
             JOIN agent_run_results AS result
               ON result.run_id = r.run_id
              AND result.manifest_sha256 = r.manifest_sha256
             LEFT JOIN matches AS m ON m.match_id = r.match_id
             WHERE r.run_id = ? AND r.manifest_sha256 = ?
               AND r.game_id = ? AND r.seat_number = ?
               AND r.match_id IS ? AND m.tournament_id IS ?
               AND result.completed_at_ms <= ?",
        )
        .bind(i64::from(RUN_TELEMETRY_SCHEMA_VERSION))
        .bind(i64::from(TELEMETRY_REDACTION_POLICY_VERSION))
        .bind(&correlation.tournament_id)
        .bind(&correlation.match_id)
        .bind(bytes)
        .bind(retention)
        .bind(telemetry.retention.expires_at_unix_ms)
        .bind(recorded_at_ms)
        .bind(&correlation.run_id)
        .bind(&telemetry.manifest.manifest_sha256)
        .bind(&correlation.game_id)
        .bind(i64::from(correlation.seat_number))
        .bind(&correlation.match_id)
        .bind(&correlation.tournament_id)
        .bind(recorded_at_ms)
        .execute(&self.pool)
        .await
        .map_err(map_insert)?;
        if inserted.rows_affected() == 1 {
            Ok(())
        } else {
            Err(AgentAttributionError::Conflict)
        }
    }

    /// Loads and revalidates a private run telemetry archive.
    ///
    /// # Errors
    ///
    /// Rejects missing, malformed, schema-drifted, or column-substituted rows.
    pub async fn load_run_telemetry(
        &self,
        run_id: &str,
    ) -> Result<RunTelemetryArchive, AgentAttributionError> {
        validate_text(run_id)?;
        let row = sqlx::query(
            "SELECT manifest_sha256, telemetry_schema_version,
                    redaction_policy_version, tournament_id, match_id, game_id,
                    seat_number, telemetry_json, retention_kind, expires_at_ms
             FROM agent_run_telemetry WHERE run_id = ?",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_storage)?
        .ok_or(AgentAttributionError::NotFound)?;
        let bytes: Vec<u8> = row
            .try_get("telemetry_json")
            .map_err(|_| AgentAttributionError::Corrupt)?;
        let telemetry: RunTelemetryArchive =
            serde_json::from_slice(&bytes).map_err(|_| AgentAttributionError::Corrupt)?;
        validate_run_telemetry(&telemetry).map_err(|_| AgentAttributionError::Corrupt)?;
        let retention = match telemetry.retention.kind {
            TelemetryRetentionKind::Retain => "retain",
            TelemetryRetentionKind::Expire => "expire",
        };
        let correlation = &telemetry.correlation;
        let matches_columns = row.try_get::<String, _>("manifest_sha256").ok()
            == Some(telemetry.manifest.manifest_sha256.clone())
            && row.try_get::<i64, _>("telemetry_schema_version").ok()
                == Some(i64::from(RUN_TELEMETRY_SCHEMA_VERSION))
            && row.try_get::<i64, _>("redaction_policy_version").ok()
                == Some(i64::from(TELEMETRY_REDACTION_POLICY_VERSION))
            && row.try_get::<Option<String>, _>("tournament_id").ok()
                == Some(correlation.tournament_id.clone())
            && row.try_get::<Option<String>, _>("match_id").ok()
                == Some(correlation.match_id.clone())
            && row.try_get::<String, _>("game_id").ok() == Some(correlation.game_id.clone())
            && row.try_get::<i64, _>("seat_number").ok()
                == Some(i64::from(correlation.seat_number))
            && row.try_get::<String, _>("retention_kind").ok() == Some(retention.to_owned())
            && row.try_get::<Option<i64>, _>("expires_at_ms").ok()
                == Some(telemetry.retention.expires_at_unix_ms);
        if !matches_columns || correlation.run_id != run_id {
            return Err(AgentAttributionError::Corrupt);
        }
        Ok(telemetry)
    }

    /// Loads the structurally content-free public analytics projection.
    ///
    /// # Errors
    ///
    /// Returns the same strict storage errors as [`Self::load_run_telemetry`].
    pub async fn public_run_telemetry(
        &self,
        run_id: &str,
    ) -> Result<PublicRunTelemetry, AgentAttributionError> {
        self.load_run_telemetry(run_id)
            .await
            .map(|telemetry| telemetry.public_projection())
    }

    /// Deletes detailed and budget telemetry whose explicit retention expired.
    ///
    /// # Errors
    ///
    /// Rejects a negative clock value or unavailable transactional storage.
    pub async fn purge_expired_run_telemetry(
        &self,
        now_ms: i64,
    ) -> Result<u64, AgentAttributionError> {
        if now_ms < 0 {
            return Err(AgentAttributionError::Invalid);
        }
        let mut transaction = self.pool.begin().await.map_err(map_storage)?;
        sqlx::query(
            "DELETE FROM agent_run_budget_telemetry
             WHERE run_id IN (
                 SELECT run_id FROM agent_run_telemetry
                 WHERE expires_at_ms IS NOT NULL AND expires_at_ms <= ?
             )",
        )
        .bind(now_ms)
        .execute(&mut *transaction)
        .await
        .map_err(map_storage)?;
        let deleted = sqlx::query(
            "DELETE FROM agent_run_telemetry
             WHERE expires_at_ms IS NOT NULL AND expires_at_ms <= ?",
        )
        .bind(now_ms)
        .execute(&mut *transaction)
        .await
        .map_err(map_storage)?
        .rows_affected();
        transaction.commit().await.map_err(map_storage)?;
        Ok(deleted)
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
                let seat = row
                    .try_get::<i64, _>("seat_number")
                    .ok()
                    .and_then(|number| u8::try_from(number).ok())
                    .and_then(Seat::from_number)
                    .ok_or(AgentAttributionError::Corrupt)?;
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

fn validate_budget_telemetry(telemetry: &BudgetTelemetry) -> Result<(), AgentAttributionError> {
    if telemetry.schema_version != BUDGET_TELEMETRY_SCHEMA_VERSION
        || telemetry.capabilities.schema_version != BUDGET_CAPABILITY_SCHEMA_VERSION
        || telemetry
            .limit_events
            .iter()
            .enumerate()
            .any(|(sequence, event)| {
                event.sequence != sequence as u64 || event.observed <= event.limit
            })
    {
        Err(AgentAttributionError::Invalid)
    } else {
        Ok(())
    }
}

fn validate_run_telemetry(telemetry: &RunTelemetryArchive) -> Result<(), AgentAttributionError> {
    validate_identity(&telemetry.manifest)?;
    telemetry
        .validate()
        .map_err(|_| AgentAttributionError::Invalid)
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
    seat.number() as i64
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
