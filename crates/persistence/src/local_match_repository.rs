use sqlx::{Row, SqlitePool};
use thiserror::Error;

const MAX_STATUS_BYTES: usize = 256 * 1024;

/// Opaque, versioned local-agent-match status retained for the server adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredLocalAgentMatch {
    pub game_id: String,
    pub status_schema_version: u16,
    pub status_json: Vec<u8>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Persistence failures for the local operator match index.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum LocalMatchRepositoryError {
    #[error("local match record is invalid")]
    Invalid,
    #[error("local match storage is unavailable")]
    Unavailable,
    #[error("local match storage is corrupt")]
    Corrupt,
}

/// SQLx-backed privacy-safe index of locally orchestrated agent matches.
#[derive(Clone, Debug)]
pub struct SqliteLocalMatchRepository {
    pool: SqlitePool,
}

impl SqliteLocalMatchRepository {
    #[must_use]
    pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Inserts or replaces one strict status snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`LocalMatchRepositoryError::Invalid`] for a status outside the
    /// bounded storage contract, or `Unavailable` when `SQLite` cannot commit it.
    pub async fn upsert(
        &self,
        record: StoredLocalAgentMatch,
    ) -> Result<(), LocalMatchRepositoryError> {
        validate(&record)?;
        let affected = sqlx::query(
            "INSERT INTO local_agent_matches (
                game_id, status_schema_version, status_json, created_at_ms, updated_at_ms
             ) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(game_id) DO UPDATE SET
                status_schema_version = excluded.status_schema_version,
                status_json = excluded.status_json,
                updated_at_ms = excluded.updated_at_ms",
        )
        .bind(&record.game_id)
        .bind(i64::from(record.status_schema_version))
        .bind(record.status_json)
        .bind(record.created_at_ms)
        .bind(record.updated_at_ms)
        .execute(&self.pool)
        .await
        .map_err(|_| LocalMatchRepositoryError::Unavailable)?
        .rows_affected();
        if affected == 1 {
            Ok(())
        } else {
            Err(LocalMatchRepositoryError::Unavailable)
        }
    }

    /// Loads every retained local match, newest first.
    ///
    /// # Errors
    ///
    /// Returns `Unavailable` when `SQLite` cannot be read, or `Corrupt` when a
    /// stored row violates the versioned status bounds.
    pub async fn list(&self) -> Result<Vec<StoredLocalAgentMatch>, LocalMatchRepositoryError> {
        let rows = sqlx::query(
            "SELECT game_id, status_schema_version, status_json, created_at_ms, updated_at_ms
             FROM local_agent_matches
             ORDER BY updated_at_ms DESC, game_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|_| LocalMatchRepositoryError::Unavailable)?;
        rows.into_iter()
            .map(|row| {
                let version = row
                    .try_get::<i64, _>("status_schema_version")
                    .ok()
                    .and_then(|value| u16::try_from(value).ok())
                    .ok_or(LocalMatchRepositoryError::Corrupt)?;
                let record = StoredLocalAgentMatch {
                    game_id: row
                        .try_get("game_id")
                        .map_err(|_| LocalMatchRepositoryError::Corrupt)?,
                    status_schema_version: version,
                    status_json: row
                        .try_get("status_json")
                        .map_err(|_| LocalMatchRepositoryError::Corrupt)?,
                    created_at_ms: row
                        .try_get("created_at_ms")
                        .map_err(|_| LocalMatchRepositoryError::Corrupt)?,
                    updated_at_ms: row
                        .try_get("updated_at_ms")
                        .map_err(|_| LocalMatchRepositoryError::Corrupt)?,
                };
                validate(&record).map_err(|_| LocalMatchRepositoryError::Corrupt)?;
                Ok(record)
            })
            .collect()
    }
}

fn validate(record: &StoredLocalAgentMatch) -> Result<(), LocalMatchRepositoryError> {
    if record.game_id.is_empty()
        || record.game_id.len() > 128
        || record.status_schema_version == 0
        || record.status_json.is_empty()
        || record.status_json.len() > MAX_STATUS_BYTES
        || record.created_at_ms < 0
        || record.updated_at_ms < record.created_at_ms
    {
        return Err(LocalMatchRepositoryError::Invalid);
    }
    Ok(())
}
