use std::collections::BTreeSet;

use sqlx::{Row, SqlitePool};
use tempfile::TempDir;
use word_arena_application::{
    AgentRunId, AuditAction, AuditActor, AuditOutcome, AuditRecord, CapabilityDescriptor,
    CapabilityId, CapabilityRecord, CapabilityRepository, CapabilityRepositoryError,
    CapabilityRole, CapabilityScope, GameId, UnixMillis,
};
use word_arena_engine::Seat;
use word_arena_persistence::{SqliteCapabilityRepository, connect_and_migrate};

#[tokio::test]
async fn digest_only_records_and_privacy_safe_audits_round_trip() {
    let database = Database::new().await;
    let record = capability(1, Seat::One, Some("agent-run"));
    database
        .repository
        .insert(record.clone(), audit(&record, AuditAction::Issue))
        .await
        .unwrap();
    assert_eq!(
        database
            .repository
            .load(&record.descriptor.capability_id)
            .await
            .unwrap(),
        record
    );

    let columns = sqlx::query_scalar::<_, String>(
        "SELECT name FROM pragma_table_info('capabilities') ORDER BY cid",
    )
    .fetch_all(&database.pool)
    .await
    .unwrap();
    assert!(columns.iter().any(|column| column == "token_digest"));
    assert!(!columns.iter().any(|column| column == "token"));
    let row = sqlx::query("SELECT token_digest, metadata_json FROM capabilities JOIN audit_records USING (game_id) WHERE capability_id = ?")
        .bind(record.descriptor.capability_id.as_str())
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(row.get::<Vec<u8>, _>("token_digest"), vec![1; 32]);
    let metadata = row.get::<Vec<u8>, _>("metadata_json");
    let metadata_text = String::from_utf8(metadata).unwrap();
    assert!(metadata_text.contains(record.descriptor.capability_id.as_str()));
    for forbidden in ["wa_cap_v1", "rack", "seed", "bag", "token"] {
        assert!(!metadata_text.contains(forbidden));
    }
}

#[tokio::test]
async fn revoke_and_rotation_are_atomic_and_isolated() {
    let database = Database::new().await;
    let seat_one = capability(1, Seat::One, None);
    let seat_two = capability(2, Seat::Two, None);
    database
        .repository
        .insert(seat_one.clone(), audit(&seat_one, AuditAction::Issue))
        .await
        .unwrap();
    database
        .repository
        .insert(seat_two.clone(), audit(&seat_two, AuditAction::Issue))
        .await
        .unwrap();

    database
        .repository
        .revoke(
            &seat_one.descriptor.capability_id,
            UnixMillis(20),
            audit(&seat_one, AuditAction::Revoke),
        )
        .await
        .unwrap();
    assert_eq!(
        database
            .repository
            .load(&seat_one.descriptor.capability_id)
            .await
            .unwrap()
            .revoked_at,
        Some(UnixMillis(20))
    );
    assert_eq!(
        database
            .repository
            .load(&seat_two.descriptor.capability_id)
            .await
            .unwrap()
            .revoked_at,
        None
    );
    assert_eq!(
        database
            .repository
            .revoke(
                &seat_one.descriptor.capability_id,
                UnixMillis(21),
                audit(&seat_one, AuditAction::Revoke),
            )
            .await,
        Err(CapabilityRepositoryError::Conflict)
    );

    let replacement = capability(3, Seat::Two, None);
    database
        .repository
        .rotate(
            &seat_two.descriptor.capability_id,
            UnixMillis(30),
            replacement.clone(),
            [
                audit(&seat_two, AuditAction::Rotate),
                audit(&replacement, AuditAction::Issue),
            ],
        )
        .await
        .unwrap();
    assert_eq!(
        database
            .repository
            .load(&seat_two.descriptor.capability_id)
            .await
            .unwrap()
            .revoked_at,
        Some(UnixMillis(30))
    );
    assert_eq!(
        database
            .repository
            .load(&replacement.descriptor.capability_id)
            .await
            .unwrap(),
        replacement
    );
}

#[tokio::test]
async fn failed_insert_and_rotation_roll_back_capabilities_and_audits() {
    let database = Database::new().await;
    let original = capability(1, Seat::One, None);
    database
        .repository
        .insert(original.clone(), audit(&original, AuditAction::Issue))
        .await
        .unwrap();
    let mut duplicate_digest = capability(2, Seat::Two, None);
    duplicate_digest.token_digest = original.token_digest;
    assert_eq!(
        database
            .repository
            .insert(
                duplicate_digest.clone(),
                audit(&duplicate_digest, AuditAction::Issue),
            )
            .await,
        Err(CapabilityRepositoryError::AlreadyExists)
    );
    assert_eq!(audit_count(&database.pool).await, 1);

    let replacement = capability(3, Seat::One, None);
    sqlx::query("INSERT INTO capabilities (capability_id, game_id, seat_number, authority_kind, scopes, token_digest, digest_version, issued_at_ms, expires_at_ms) VALUES (?, 'game', 1, 'seat', '[\"observe_seat\"]', ?, 1, 10, 100)")
        .bind(replacement.descriptor.capability_id.as_str())
        .bind([9_u8; 32].as_slice())
        .execute(&database.pool)
        .await
        .unwrap();
    assert_eq!(
        database
            .repository
            .rotate(
                &original.descriptor.capability_id,
                UnixMillis(40),
                replacement,
                [
                    audit(&original, AuditAction::Rotate),
                    audit(&original, AuditAction::Issue),
                ],
            )
            .await,
        Err(CapabilityRepositoryError::AlreadyExists)
    );
    assert_eq!(
        database
            .repository
            .load(&original.descriptor.capability_id)
            .await
            .unwrap()
            .revoked_at,
        None
    );
    assert_eq!(audit_count(&database.pool).await, 1);
}

struct Database {
    _directory: TempDir,
    pool: SqlitePool,
    repository: SqliteCapabilityRepository,
}

impl Database {
    async fn new() -> Self {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("capabilities.sqlite3");
        let pool = connect_and_migrate(&format!("sqlite://{}", path.display()))
            .await
            .unwrap();
        seed_game_and_agent(&pool).await;
        let repository = SqliteCapabilityRepository::new(pool.clone());
        Self {
            _directory: directory,
            pool,
            repository,
        }
    }
}

async fn seed_game_and_agent(pool: &SqlitePool) {
    let hash_a = "a".repeat(64);
    let hash_b = "b".repeat(64);
    sqlx::query("INSERT INTO rulesets (ruleset_id, schema_version, content_sha256, definition_json, created_at_ms) VALUES ('english-v1', 1, ?, X'7b7d', 1)")
        .bind(&hash_a)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO lexicon_packs (pack_id, pack_version, content_sha256, format_version, normalization_version, locale, identity_json, installed_at_ms) VALUES ('pack', '1', ?, 1, 1, 'en', X'7b7d', 1)")
        .bind(&hash_b)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO games (game_id, status, version, ruleset_id, ruleset_sha256, lexicon_pack_id, lexicon_pack_version, lexicon_content_sha256, rng_algorithm, seed_commitment_sha256, created_at_ms, updated_at_ms) VALUES ('game', 'active', 0, 'english-v1', ?, 'pack', '1', ?, 'rng', ?, 1, 1)")
        .bind(&hash_a)
        .bind(&hash_b)
        .bind("c".repeat(64))
        .execute(pool)
        .await
        .unwrap();
    for seat in [1_i64, 2] {
        sqlx::query("INSERT INTO seats (game_id, seat_number, participant_kind, participant_id, created_at_ms) VALUES ('game', ?, 'agent', ?, 1)")
            .bind(seat)
            .bind(format!("agent-{seat}"))
            .execute(pool)
            .await
            .unwrap();
    }
    let manifest = "d".repeat(64);
    sqlx::query("INSERT INTO agent_manifests (manifest_sha256, schema_version, manifest_json, created_at_ms) VALUES (?, 1, X'7b7d', 1)")
        .bind(&manifest)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO agent_runs (run_id, game_id, seat_number, manifest_sha256, status, created_at_ms) VALUES ('agent-run', 'game', 1, ?, 'running', 1)")
        .bind(manifest)
        .execute(pool)
        .await
        .unwrap();
}

fn capability(id_byte: u8, seat: Seat, agent_run: Option<&str>) -> CapabilityRecord {
    let id = format!("{id_byte:02x}").repeat(16);
    CapabilityRecord {
        descriptor: CapabilityDescriptor {
            capability_id: CapabilityId::new(id).unwrap(),
            game_id: GameId::new("game").unwrap(),
            role: CapabilityRole::Seat(seat),
            scopes: [
                CapabilityScope::ObservePublic,
                CapabilityScope::ObserveSeat,
                CapabilityScope::Act,
            ]
            .into_iter()
            .collect::<BTreeSet<_>>(),
            issued_at: UnixMillis(10),
            expires_at: UnixMillis(100),
            agent_run_id: agent_run.map(|value| AgentRunId::new(value).unwrap()),
        },
        token_digest: [id_byte; 32],
        digest_version: 1,
        revoked_at: None,
    }
}

fn audit(capability: &CapabilityRecord, action: AuditAction) -> AuditRecord {
    AuditRecord {
        game_id: Some(capability.descriptor.game_id.clone()),
        actor: AuditActor::System,
        action,
        outcome: AuditOutcome::Success,
        capability_id: Some(capability.descriptor.capability_id.clone()),
        scope: None,
        occurred_at: UnixMillis(10),
    }
}

async fn audit_count(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_records")
        .fetch_one(pool)
        .await
        .unwrap()
}
