use std::collections::BTreeSet;

use sqlx::{Row, Sqlite, SqlitePool, Transaction, sqlite::SqliteRow};
use word_arena_application::{
    AgentRunId, AuditActor, AuditRecord, BoxFuture, CapabilityDescriptor, CapabilityId,
    CapabilityRecord, CapabilityRepository, CapabilityRepositoryError, CapabilityRole,
    CapabilityScope, GameId, UnixMillis,
};
use word_arena_engine::Seat;

/// SQLx-backed capability and privacy-safe audit repository.
#[derive(Clone, Debug)]
pub struct SqliteCapabilityRepository {
    pool: SqlitePool,
}

impl SqliteCapabilityRepository {
    /// Binds the repository to an already-migrated pool.
    #[must_use]
    pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl CapabilityRepository for SqliteCapabilityRepository {
    fn insert(
        &self,
        capability: CapabilityRecord,
        audit: AuditRecord,
    ) -> BoxFuture<'_, Result<(), CapabilityRepositoryError>> {
        Box::pin(async move {
            let mut transaction = self.pool.begin().await.map_err(map_transient)?;
            insert_capability(&mut transaction, &capability).await?;
            insert_audit(&mut transaction, &audit).await?;
            transaction.commit().await.map_err(map_transient)
        })
    }

    fn load(
        &self,
        capability_id: &CapabilityId,
    ) -> BoxFuture<'_, Result<CapabilityRecord, CapabilityRepositoryError>> {
        let capability_id = capability_id.clone();
        Box::pin(async move {
            let row = sqlx::query(
                "SELECT capability_id, game_id, seat_number, authority_kind, scopes,
                        token_digest, digest_version, issued_at_ms, expires_at_ms,
                        revoked_at_ms, agent_run_id
                 FROM capabilities WHERE capability_id = ?",
            )
            .bind(capability_id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(map_transient)?
            .ok_or(CapabilityRepositoryError::NotFound)?;
            decode_capability(&row)
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
            let mut transaction = self.pool.begin().await.map_err(map_transient)?;
            let result = sqlx::query(
                "UPDATE capabilities SET revoked_at_ms = ?
                 WHERE capability_id = ? AND revoked_at_ms IS NULL",
            )
            .bind(revoked_at.0)
            .bind(capability_id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(map_write)?;
            if result.rows_affected() != 1 {
                return Err(classify_missing_or_conflict(&mut transaction, &capability_id).await);
            }
            insert_audit(&mut transaction, &audit).await?;
            transaction.commit().await.map_err(map_transient)
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
            let mut transaction = self.pool.begin().await.map_err(map_transient)?;
            let result = sqlx::query(
                "UPDATE capabilities SET revoked_at_ms = ?
                 WHERE capability_id = ? AND revoked_at_ms IS NULL",
            )
            .bind(revoked_at.0)
            .bind(prior_id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(map_write)?;
            if result.rows_affected() != 1 {
                return Err(classify_missing_or_conflict(&mut transaction, &prior_id).await);
            }
            insert_capability(&mut transaction, &replacement).await?;
            for audit in audits {
                insert_audit(&mut transaction, &audit).await?;
            }
            transaction.commit().await.map_err(map_transient)
        })
    }

    fn append_audit(
        &self,
        audit: AuditRecord,
    ) -> BoxFuture<'_, Result<(), CapabilityRepositoryError>> {
        Box::pin(async move {
            let mut transaction = self.pool.begin().await.map_err(map_transient)?;
            insert_audit(&mut transaction, &audit).await?;
            transaction.commit().await.map_err(map_transient)
        })
    }
}

async fn insert_capability(
    transaction: &mut Transaction<'_, Sqlite>,
    capability: &CapabilityRecord,
) -> Result<(), CapabilityRepositoryError> {
    let (authority_kind, seat_number) = encode_role(capability.descriptor.role);
    let scopes = serde_json::to_string(&capability.descriptor.scopes)
        .map_err(|_| CapabilityRepositoryError::Corrupt)?;
    sqlx::query(
        "INSERT INTO capabilities (
            capability_id, game_id, seat_number, authority_kind, scopes,
            token_digest, digest_version, issued_at_ms, expires_at_ms,
            revoked_at_ms, agent_run_id
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(capability.descriptor.capability_id.as_str())
    .bind(capability.descriptor.game_id.as_str())
    .bind(seat_number)
    .bind(authority_kind)
    .bind(scopes)
    .bind(capability.token_digest.as_slice())
    .bind(i64::from(capability.digest_version))
    .bind(capability.descriptor.issued_at.0)
    .bind(capability.descriptor.expires_at.0)
    .bind(capability.revoked_at.map(|value| value.0))
    .bind(
        capability
            .descriptor
            .agent_run_id
            .as_ref()
            .map(AgentRunId::as_str),
    )
    .execute(&mut **transaction)
    .await
    .map_err(map_insert)?;
    Ok(())
}

async fn insert_audit(
    transaction: &mut Transaction<'_, Sqlite>,
    audit: &AuditRecord,
) -> Result<(), CapabilityRepositoryError> {
    let (actor_kind, seat_number) = encode_actor(audit.actor);
    let metadata = serde_json::to_vec(&serde_json::json!({
        "capability_id": audit.capability_id,
        "scope": audit.scope,
    }))
    .map_err(|_| CapabilityRepositoryError::Corrupt)?;
    sqlx::query(
        "INSERT INTO audit_records (
            game_id, actor_kind, seat_number, action, outcome, metadata_json, occurred_at_ms
         ) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(audit.game_id.as_ref().map(GameId::as_str))
    .bind(actor_kind)
    .bind(seat_number)
    .bind(audit_action(audit))
    .bind(audit_outcome(audit))
    .bind(metadata)
    .bind(audit.occurred_at.0)
    .execute(&mut **transaction)
    .await
    .map_err(map_write)?;
    Ok(())
}

fn decode_capability(row: &SqliteRow) -> Result<CapabilityRecord, CapabilityRepositoryError> {
    let capability_id = CapabilityId::new(
        row.try_get::<String, _>("capability_id")
            .map_err(|_| CapabilityRepositoryError::Corrupt)?,
    )
    .map_err(|_| CapabilityRepositoryError::Corrupt)?;
    let game_id = GameId::new(
        row.try_get::<String, _>("game_id")
            .map_err(|_| CapabilityRepositoryError::Corrupt)?,
    )
    .map_err(|_| CapabilityRepositoryError::Corrupt)?;
    let seat_number = row
        .try_get::<Option<i64>, _>("seat_number")
        .map_err(|_| CapabilityRepositoryError::Corrupt)?;
    let authority_kind = row
        .try_get::<String, _>("authority_kind")
        .map_err(|_| CapabilityRepositoryError::Corrupt)?;
    let role = decode_role(&authority_kind, seat_number)?;
    let scopes = serde_json::from_str::<BTreeSet<CapabilityScope>>(
        &row.try_get::<String, _>("scopes")
            .map_err(|_| CapabilityRepositoryError::Corrupt)?,
    )
    .map_err(|_| CapabilityRepositoryError::Corrupt)?;
    if scopes.is_empty() || scopes.iter().any(|scope| !role_allows(role, *scope)) {
        return Err(CapabilityRepositoryError::Corrupt);
    }
    let digest = row
        .try_get::<Vec<u8>, _>("token_digest")
        .map_err(|_| CapabilityRepositoryError::Corrupt)?;
    let token_digest = digest
        .try_into()
        .map_err(|_| CapabilityRepositoryError::Corrupt)?;
    let digest_version = u16::try_from(
        row.try_get::<i64, _>("digest_version")
            .map_err(|_| CapabilityRepositoryError::Corrupt)?,
    )
    .map_err(|_| CapabilityRepositoryError::Corrupt)?;
    let issued_at = UnixMillis(
        row.try_get::<i64, _>("issued_at_ms")
            .map_err(|_| CapabilityRepositoryError::Corrupt)?,
    );
    let expires_at = UnixMillis(
        row.try_get::<i64, _>("expires_at_ms")
            .map_err(|_| CapabilityRepositoryError::Corrupt)?,
    );
    if expires_at <= issued_at {
        return Err(CapabilityRepositoryError::Corrupt);
    }
    let revoked_at = row
        .try_get::<Option<i64>, _>("revoked_at_ms")
        .map_err(|_| CapabilityRepositoryError::Corrupt)?
        .map(UnixMillis);
    let agent_run_id = row
        .try_get::<Option<String>, _>("agent_run_id")
        .map_err(|_| CapabilityRepositoryError::Corrupt)?
        .map(AgentRunId::new)
        .transpose()
        .map_err(|_| CapabilityRepositoryError::Corrupt)?;
    if agent_run_id.is_some() && !matches!(role, CapabilityRole::Seat(_)) {
        return Err(CapabilityRepositoryError::Corrupt);
    }
    Ok(CapabilityRecord {
        descriptor: CapabilityDescriptor {
            capability_id,
            game_id,
            role,
            scopes,
            issued_at,
            expires_at,
            agent_run_id,
        },
        token_digest,
        digest_version,
        revoked_at,
    })
}

async fn classify_missing_or_conflict(
    transaction: &mut Transaction<'_, Sqlite>,
    capability_id: &CapabilityId,
) -> CapabilityRepositoryError {
    match sqlx::query_scalar::<_, i64>("SELECT 1 FROM capabilities WHERE capability_id = ? LIMIT 1")
        .bind(capability_id.as_str())
        .fetch_optional(&mut **transaction)
        .await
    {
        Ok(Some(_)) => CapabilityRepositoryError::Conflict,
        Ok(None) => CapabilityRepositoryError::NotFound,
        Err(_) => CapabilityRepositoryError::Unavailable,
    }
}

const fn encode_role(role: CapabilityRole) -> (&'static str, Option<i64>) {
    match role {
        CapabilityRole::Public => ("public", None),
        CapabilityRole::Seat(Seat::One) => ("seat", Some(1)),
        CapabilityRole::Seat(Seat::Two) => ("seat", Some(2)),
        CapabilityRole::HumanSpectator => ("human_spectator", None),
        CapabilityRole::Administrator => ("administrator", None),
    }
}

fn decode_role(
    authority_kind: &str,
    seat_number: Option<i64>,
) -> Result<CapabilityRole, CapabilityRepositoryError> {
    match (authority_kind, seat_number) {
        ("public", None) => Ok(CapabilityRole::Public),
        ("seat", Some(1)) => Ok(CapabilityRole::Seat(Seat::One)),
        ("seat", Some(2)) => Ok(CapabilityRole::Seat(Seat::Two)),
        ("human_spectator", None) => Ok(CapabilityRole::HumanSpectator),
        ("administrator", None) => Ok(CapabilityRole::Administrator),
        _ => Err(CapabilityRepositoryError::Corrupt),
    }
}

const fn role_allows(role: CapabilityRole, scope: CapabilityScope) -> bool {
    match role {
        CapabilityRole::Public => matches!(scope, CapabilityScope::ObservePublic),
        CapabilityRole::Seat(_) => matches!(
            scope,
            CapabilityScope::ObservePublic
                | CapabilityScope::ObserveSeat
                | CapabilityScope::Act
                | CapabilityScope::Preview
        ),
        CapabilityRole::HumanSpectator => matches!(
            scope,
            CapabilityScope::ObservePublic | CapabilityScope::ObserveHumanSpectator
        ),
        CapabilityRole::Administrator => matches!(
            scope,
            CapabilityScope::ObservePublic | CapabilityScope::ObserveAdministrator
        ),
    }
}

const fn encode_actor(actor: AuditActor) -> (&'static str, Option<i64>) {
    match actor {
        AuditActor::System => ("system", None),
        AuditActor::Public => ("public", None),
        AuditActor::Seat(Seat::One) => ("seat", Some(1)),
        AuditActor::Seat(Seat::Two) => ("seat", Some(2)),
        AuditActor::HumanSpectator => ("human_spectator", None),
        AuditActor::Administrator => ("administrator", None),
    }
}

fn audit_action(audit: &AuditRecord) -> &'static str {
    match audit.action {
        word_arena_application::AuditAction::Issue => "capability_issue",
        word_arena_application::AuditAction::Authenticate => "capability_authenticate",
        word_arena_application::AuditAction::Revoke => "capability_revoke",
        word_arena_application::AuditAction::Rotate => "capability_rotate",
        word_arena_application::AuditAction::PrivilegedAccess => "privileged_access",
    }
}

fn audit_outcome(audit: &AuditRecord) -> &'static str {
    match audit.outcome {
        word_arena_application::AuditOutcome::Success => "success",
        word_arena_application::AuditOutcome::DeniedMalformed => "denied_malformed",
        word_arena_application::AuditOutcome::DeniedUnknown => "denied_unknown",
        word_arena_application::AuditOutcome::DeniedExpired => "denied_expired",
        word_arena_application::AuditOutcome::DeniedRevoked => "denied_revoked",
        word_arena_application::AuditOutcome::DeniedGame => "denied_game",
        word_arena_application::AuditOutcome::DeniedScope => "denied_scope",
    }
}

fn map_insert(error: sqlx::Error) -> CapabilityRepositoryError {
    if let sqlx::Error::Database(database) = &error
        && database.is_unique_violation()
    {
        CapabilityRepositoryError::AlreadyExists
    } else {
        map_write(error)
    }
}

fn map_write(error: sqlx::Error) -> CapabilityRepositoryError {
    if let sqlx::Error::Database(database) = &error
        && (database.is_foreign_key_violation() || database.is_check_violation())
    {
        CapabilityRepositoryError::Corrupt
    } else {
        map_transient(error)
    }
}

fn map_transient(_error: sqlx::Error) -> CapabilityRepositoryError {
    CapabilityRepositoryError::Unavailable
}
