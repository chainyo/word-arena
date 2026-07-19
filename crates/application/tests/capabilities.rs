#![cfg(feature = "test-support")]

use std::{collections::BTreeSet, sync::Arc};

use word_arena_application::{
    AgentRunId, ApplicationClock, ApplicationError, ApplicationRuntime, AuditAction, AuditOutcome,
    AuthenticatedCredential, CapabilityAdapters, CapabilityDigestKey, CapabilityError,
    CapabilityRepository, CapabilityRole, CapabilityScope, CapabilityTokenSource,
    GameActionCommand, GameIdSource, GameRepository, IdempotencyKey, IssueCapabilityRequest,
    LexiconResolver, SeedSource, SystemCapabilityTokenSource, UnixMillis,
    test_support::{
        InMemoryCapabilityRepository, InMemoryGameRepository, InMemoryLexiconResolver, ManualClock,
        SequenceCapabilityTokens, SequenceGameIds, SequenceSeeds,
    },
};
use word_arena_engine::{Language, Move, Ruleset, Seat, Turn, WordValidator};
use word_arena_lexicon::{NormalizedKey, PackIdentity};

const START: UnixMillis = UnixMillis(1_000);

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

#[test]
fn production_source_uses_fresh_redacted_os_material() {
    let source = SystemCapabilityTokenSource;
    let first = source.next_material().unwrap();
    let second = source.next_material().unwrap();
    assert_ne!(first, second);
    assert_eq!(format!("{first:?}"), "CapabilityMaterial([REDACTED])");
}

#[tokio::test]
async fn issue_authenticate_and_denials_are_bound_and_secret_free() {
    let fixture = fixture();
    let first = create_game(&fixture.runtime, "first").await;
    let second = create_game(&fixture.runtime, "second").await;
    let issued = fixture
        .runtime
        .issue_capability(seat_request(&first, Seat::One, UnixMillis(2_000)))
        .await
        .unwrap();

    assert_eq!(format!("{:?}", issued.token), "CapabilityToken([REDACTED])");
    let descriptor = issued.descriptor.clone();
    let token = issued.token.into_secret();
    let stored = fixture
        .capabilities
        .record(&descriptor.capability_id)
        .unwrap();
    assert_eq!(stored.descriptor, descriptor);
    assert_eq!(stored.token_digest.len(), 32);
    assert!(
        !token
            .as_bytes()
            .windows(32)
            .any(|part| part == stored.token_digest)
    );

    let authenticated = fixture
        .runtime
        .authenticate_capability(&token, &first, CapabilityScope::Act)
        .await
        .unwrap();
    let AuthenticatedCredential::Seat(credential) = authenticated else {
        panic!("seat capability must produce only a seat credential");
    };
    assert_eq!(credential.game_id(), &first);
    assert_eq!(credential.seat(), Seat::One);
    assert!(matches!(
        fixture
            .runtime
            .service()
            .act(
                &credential,
                GameActionCommand {
                    game_id: first.clone(),
                    expected_version: 0,
                    turn: Turn {
                        number: 0,
                        seat: Seat::Two,
                    },
                    idempotency_key: IdempotencyKey::new("wrong-seat").unwrap(),
                    action: Move::Pass,
                },
            )
            .await,
        Err(ApplicationError::WrongSeatAuthority { .. })
    ));

    assert_unauthorized(fixture.runtime.authenticate_capability(
        &token,
        &first,
        CapabilityScope::ObserveHumanSpectator,
    ))
    .await;
    assert_unauthorized(fixture.runtime.authenticate_capability(
        &token,
        &second,
        CapabilityScope::Act,
    ))
    .await;
    assert_unauthorized(fixture.runtime.authenticate_capability(
        "not-a-capability",
        &first,
        CapabilityScope::Act,
    ))
    .await;

    let mut tampered = token.clone().into_bytes();
    let final_byte = tampered.last_mut().unwrap();
    *final_byte = if *final_byte == b'a' { b'b' } else { b'a' };
    let tampered = String::from_utf8(tampered).unwrap();
    assert_unauthorized(fixture.runtime.authenticate_capability(
        &tampered,
        &first,
        CapabilityScope::Act,
    ))
    .await;

    let audits_json = serde_json::to_string(&fixture.capabilities.audits()).unwrap();
    assert!(!audits_json.contains(&token));
    assert!(!audits_json.contains("rack"));
    assert!(!audits_json.contains("seed"));
    assert!(!audits_json.contains("bag"));
    assert!(
        fixture
            .capabilities
            .audits()
            .iter()
            .any(|audit| audit.outcome == AuditOutcome::DeniedMalformed)
    );
}

#[tokio::test]
async fn issuance_rejects_privilege_scope_time_and_agent_binding_escalation() {
    let fixture = fixture();
    let game_id = create_game(&fixture.runtime, "invalid-issuance").await;
    for request in [
        IssueCapabilityRequest {
            game_id: game_id.clone(),
            role: CapabilityRole::HumanSpectator,
            scopes: scopes([CapabilityScope::ObserveHumanSpectator]),
            expires_at: UnixMillis(2_000),
            agent_run_id: Some(AgentRunId::new("agent-run").unwrap()),
        },
        IssueCapabilityRequest {
            game_id: game_id.clone(),
            role: CapabilityRole::Public,
            scopes: scopes([CapabilityScope::Act]),
            expires_at: UnixMillis(2_000),
            agent_run_id: None,
        },
        IssueCapabilityRequest {
            game_id,
            role: CapabilityRole::Seat(Seat::One),
            scopes: BTreeSet::new(),
            expires_at: START,
            agent_run_id: None,
        },
    ] {
        assert!(matches!(
            fixture.runtime.issue_capability(request).await,
            Err(CapabilityError::InvalidRequest)
        ));
    }
}

#[tokio::test]
async fn expiry_revocation_and_rotation_are_immediate_and_isolated() {
    let fixture = fixture();
    let game_id = create_game(&fixture.runtime, "game").await;
    let seat_one = fixture
        .runtime
        .issue_capability(seat_request(&game_id, Seat::One, UnixMillis(1_500)))
        .await
        .unwrap();
    let seat_one_id = seat_one.descriptor.capability_id.clone();
    let seat_one_token = seat_one.token.into_secret();
    let seat_two = fixture
        .runtime
        .issue_capability(seat_request(&game_id, Seat::Two, UnixMillis(4_000)))
        .await
        .unwrap();
    let seat_two_id = seat_two.descriptor.capability_id.clone();
    let seat_two_token = seat_two.token.into_secret();

    fixture.clock.set(UnixMillis(1_500));
    assert_unauthorized(fixture.runtime.authenticate_capability(
        &seat_one_token,
        &game_id,
        CapabilityScope::Act,
    ))
    .await;
    fixture.clock.set(UnixMillis(1_600));
    fixture
        .runtime
        .revoke_capability(&seat_two_id)
        .await
        .unwrap();
    assert_unauthorized(fixture.runtime.authenticate_capability(
        &seat_two_token,
        &game_id,
        CapabilityScope::Act,
    ))
    .await;
    assert_eq!(
        fixture
            .capabilities
            .record(&seat_one_id)
            .unwrap()
            .revoked_at,
        None
    );

    let fresh = fixture
        .runtime
        .issue_capability(seat_request(&game_id, Seat::Two, UnixMillis(4_000)))
        .await
        .unwrap();
    let fresh_id = fresh.descriptor.capability_id.clone();
    let fresh_token = fresh.token.into_secret();
    let replacement = fixture
        .runtime
        .rotate_capability(&fresh_id, UnixMillis(5_000))
        .await
        .unwrap();
    let replacement_token = replacement.token.into_secret();
    assert_unauthorized(fixture.runtime.authenticate_capability(
        &fresh_token,
        &game_id,
        CapabilityScope::ObserveSeat,
    ))
    .await;
    assert!(matches!(
        fixture
            .runtime
            .authenticate_capability(
                &replacement_token,
                &game_id,
                CapabilityScope::ObserveSeat,
            )
            .await,
        Ok(AuthenticatedCredential::Seat(credential)) if credential.seat() == Seat::Two
    ));
    assert_eq!(
        fixture.capabilities.record(&fresh_id).unwrap().revoked_at,
        Some(UnixMillis(1_600))
    );
}

#[tokio::test]
async fn privileged_authentication_is_explicitly_audited() {
    let fixture = fixture();
    let game_id = create_game(&fixture.runtime, "privileged").await;
    for (role, scope) in [
        (
            CapabilityRole::HumanSpectator,
            CapabilityScope::ObserveHumanSpectator,
        ),
        (
            CapabilityRole::Administrator,
            CapabilityScope::ObserveAdministrator,
        ),
    ] {
        let issued = fixture
            .runtime
            .issue_capability(IssueCapabilityRequest {
                game_id: game_id.clone(),
                role,
                scopes: scopes([scope]),
                expires_at: UnixMillis(2_000),
                agent_run_id: None,
            })
            .await
            .unwrap();
        fixture
            .runtime
            .authenticate_capability(&issued.token.into_secret(), &game_id, scope)
            .await
            .unwrap();
    }
    let audits = fixture.capabilities.audits();
    assert_eq!(
        audits
            .iter()
            .filter(|audit| audit.action == AuditAction::PrivilegedAccess)
            .count(),
        2
    );
    assert!(
        audits
            .iter()
            .filter(|audit| audit.action == AuditAction::PrivilegedAccess)
            .all(|audit| audit.outcome == AuditOutcome::Success)
    );
}

struct Fixture {
    runtime: ApplicationRuntime,
    capabilities: Arc<InMemoryCapabilityRepository>,
    clock: Arc<ManualClock>,
}

fn fixture() -> Fixture {
    let ruleset = Ruleset::english_v1();
    let lexicon = Arc::new(AcceptingLexicon(ruleset.lexicon)) as Arc<dyn WordValidator>;
    let game_repository: Arc<dyn GameRepository> = Arc::new(InMemoryGameRepository::default());
    let resolver: Arc<dyn LexiconResolver> = Arc::new(InMemoryLexiconResolver::new([lexicon]));
    let ids: Arc<dyn GameIdSource> = Arc::new(SequenceGameIds::new("capability-game"));
    let seeds: Arc<dyn SeedSource> = Arc::new(SequenceSeeds::new(10));
    let clock = Arc::new(ManualClock::new(START));
    let application_clock: Arc<dyn ApplicationClock> = clock.clone();
    let capabilities = Arc::new(InMemoryCapabilityRepository::default());
    let capability_repository: Arc<dyn CapabilityRepository> = capabilities.clone();
    let runtime = ApplicationRuntime::new(
        game_repository,
        resolver,
        ids,
        seeds,
        application_clock,
        CapabilityAdapters::new(
            capability_repository,
            Arc::new(SequenceCapabilityTokens::new(1)),
            CapabilityDigestKey::new([42; 32]),
        ),
    );
    Fixture {
        runtime,
        capabilities,
        clock,
    }
}

async fn create_game(runtime: &ApplicationRuntime, key: &str) -> word_arena_application::GameId {
    let service = runtime.service();
    service
        .create_game(
            service.prepare_create_game(Language::English, IdempotencyKey::new(key).unwrap()),
        )
        .await
        .unwrap()
        .game_id
}

fn seat_request(
    game_id: &word_arena_application::GameId,
    seat: Seat,
    expires_at: UnixMillis,
) -> IssueCapabilityRequest {
    IssueCapabilityRequest {
        game_id: game_id.clone(),
        role: CapabilityRole::Seat(seat),
        scopes: scopes([
            CapabilityScope::ObservePublic,
            CapabilityScope::ObserveSeat,
            CapabilityScope::Act,
        ]),
        expires_at,
        agent_run_id: Some(AgentRunId::new(format!("run-{seat:?}")).unwrap()),
    }
}

fn scopes<const N: usize>(values: [CapabilityScope; N]) -> BTreeSet<CapabilityScope> {
    values.into_iter().collect()
}

async fn assert_unauthorized(
    result: impl Future<Output = Result<AuthenticatedCredential, CapabilityError>>,
) {
    assert!(matches!(result.await, Err(CapabilityError::Unauthorized)));
}
