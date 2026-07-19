#![cfg(feature = "test-support")]

use std::{collections::BTreeSet, sync::Arc};

use word_arena_application::{
    ActionRejection, AdministratorCredential, AdministratorGameQuery, ApplicationClock,
    ApplicationError, ApplicationRuntime, AuthenticatedCredential, Authorizes, CapabilityAdapters,
    CapabilityDigestKey, CapabilityError, CapabilityRole, CapabilityScope,
    CompetitiveGameCredentials, CompetitiveSeatCredential, GameActionCommand, GameId, GameIdSource,
    GameRepository, HumanSpectatorCredential, HumanSpectatorGameQuery, HumanSpectatorGameView,
    IdempotencyKey, InvalidAttemptResponse, IssueCapabilityRequest, LexiconResolver,
    MovePreviewCommand, OperationalPolicy, PreviewPolicy, PublicGameQuery, PublicViewerCredential,
    RepositoryError, SeatGameQuery, SeatGameView, SeedSource, TimeoutCommand, TimeoutResponse,
    UnixMillis,
    test_support::{
        FixedClock, InMemoryCapabilityRepository, InMemoryGameRepository, InMemoryLexiconResolver,
        ManualClock, SequenceCapabilityTokens, SequenceGameIds, SequenceSeeds,
    },
};
use word_arena_engine::{
    Coordinate, GameEventKind, GameMode, GamePhase, Language, Move, PhysicalTile, Placement,
    Ruleset, Seat, Tile, TileFace, Turn, WordValidator,
};
use word_arena_lexicon::{NormalizedKey, PackIdentity};

macro_rules! assert_not_authorizes {
    ($credential:ty, $query:ty) => {
        const _: fn() = || {
            struct Check<T: ?Sized>(std::marker::PhantomData<T>);
            trait AmbiguousIfAuthorized<Marker> {
                fn marker() {}
            }
            impl<T: ?Sized> AmbiguousIfAuthorized<()> for Check<T> {}
            impl<T: ?Sized + Authorizes<$query>> AmbiguousIfAuthorized<u8> for Check<T> {}
            let _ = <Check<$credential> as AmbiguousIfAuthorized<_>>::marker;
        };
    };
}

macro_rules! assert_not_serializable {
    ($credential:ty) => {
        const _: fn() = || {
            struct Check<T: ?Sized>(std::marker::PhantomData<T>);
            trait AmbiguousIfSerializable<Marker> {
                fn marker() {}
            }
            impl<T: ?Sized> AmbiguousIfSerializable<()> for Check<T> {}
            impl<T: ?Sized + serde::Serialize> AmbiguousIfSerializable<u8> for Check<T> {}
            let _ = <Check<$credential> as AmbiguousIfSerializable<_>>::marker;
        };
    };
}

assert_not_authorizes!(PublicViewerCredential, SeatGameQuery);
assert_not_authorizes!(PublicViewerCredential, HumanSpectatorGameQuery);
assert_not_authorizes!(PublicViewerCredential, AdministratorGameQuery);
assert_not_authorizes!(CompetitiveSeatCredential, PublicGameQuery);
assert_not_authorizes!(CompetitiveSeatCredential, HumanSpectatorGameQuery);
assert_not_authorizes!(CompetitiveSeatCredential, AdministratorGameQuery);
assert_not_authorizes!(HumanSpectatorCredential, PublicGameQuery);
assert_not_authorizes!(HumanSpectatorCredential, SeatGameQuery);
assert_not_authorizes!(HumanSpectatorCredential, AdministratorGameQuery);
assert_not_authorizes!(AdministratorCredential, PublicGameQuery);
assert_not_authorizes!(AdministratorCredential, SeatGameQuery);
assert_not_authorizes!(AdministratorCredential, HumanSpectatorGameQuery);

assert_not_serializable!(PublicViewerCredential);
assert_not_serializable!(CompetitiveSeatCredential);
assert_not_serializable!(CompetitiveGameCredentials);
assert_not_serializable!(HumanSpectatorCredential);
assert_not_serializable!(AdministratorCredential);

fn assert_authorizes<Credential, Query>()
where
    Credential: Authorizes<Query>,
{
}

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
fn credential_query_authorization_matrix_is_exact_at_compile_time() {
    assert_authorizes::<PublicViewerCredential, PublicGameQuery>();
    assert_authorizes::<CompetitiveSeatCredential, SeatGameQuery>();
    assert_authorizes::<HumanSpectatorCredential, HumanSpectatorGameQuery>();
    assert_authorizes::<AdministratorCredential, AdministratorGameQuery>();
}

#[tokio::test]
async fn english_and_french_finish_through_typed_application_apis() {
    let runtime = setup_runtime(&[Language::English, Language::French]);
    let service = runtime.service();
    for language in [Language::English, Language::French] {
        let command = service.prepare_create_game(language, key(&format!("create-{language:?}")));
        let created = service.create_game(command).await.unwrap();
        assert_eq!(created.created_at, UnixMillis(1_700_000_000_000));
        assert_eq!(created.public.state.version, 0);

        for version in 0..6_u64 {
            let authority = if version % 2 == 0 {
                &created.access.seat_one
            } else {
                &created.access.seat_two
            };
            let result = service
                .act(
                    authority,
                    action(&created.game_id, version, authority.seat(), Move::Pass),
                )
                .await
                .unwrap();
            assert_eq!(result.event.sequence, version + 1);
        }

        let public = service
            .public_game(
                &created.access.public,
                PublicGameQuery {
                    game_id: created.game_id.clone(),
                },
            )
            .await
            .unwrap();
        assert_eq!(public.game.state.phase, GamePhase::Finished);

        let seat_one = service
            .seat_game(
                &created.access.seat_one,
                SeatGameQuery {
                    game_id: created.game_id.clone(),
                },
            )
            .await
            .unwrap();
        let seat_two = service
            .seat_game(
                &created.access.seat_two,
                SeatGameQuery {
                    game_id: created.game_id.clone(),
                },
            )
            .await
            .unwrap();
        assert_eq!(seat_one.game.seat, Seat::One);
        assert_eq!(seat_two.game.seat, Seat::Two);
        assert_ne!(seat_one.game.rack, seat_two.game.rack);

        let spectator_credential = human_spectator_credential(&runtime, &created.game_id).await;
        let spectator = service
            .human_spectator_game(
                &spectator_credential,
                HumanSpectatorGameQuery {
                    game_id: created.game_id.clone(),
                },
            )
            .await
            .unwrap();
        assert_eq!(spectator.game.racks[0], seat_one.game.rack);
        assert_eq!(spectator.game.racks[1], seat_two.game.rack);

        let administrator_credential = administrator_credential(&runtime, &created.game_id).await;
        let administrator = service
            .administrator_game(
                &administrator_credential,
                AdministratorGameQuery {
                    game_id: created.game_id,
                },
            )
            .await
            .unwrap();
        assert_eq!(administrator.game.snapshot.state.phase, GamePhase::Finished);
    }
}

#[tokio::test]
async fn creation_retry_returns_the_original_generated_game() {
    let runtime = setup_runtime(&[Language::English, Language::French]);
    let service = runtime.service();
    let first_command = service.prepare_create_game(Language::English, key("same-create"));
    let retry_command = service.prepare_create_game(Language::English, key("same-create"));
    assert_ne!(first_command.game_id, retry_command.game_id);
    let first = service.create_game(first_command).await.unwrap();
    let retry = service.create_game(retry_command).await.unwrap();
    assert_eq!(retry.game_id, first.game_id);
    assert_eq!(retry.public, first.public);
    assert!(matches!(
        service
            .create_game(service.prepare_create_game(Language::French, key("same-create")))
            .await,
        Err(ApplicationError::ActionRejected(
            ActionRejection::IdempotencyConflict
        ))
    ));
}

#[tokio::test]
async fn placement_exchange_pass_and_resignation_route_to_the_engine() {
    let runtime = setup_runtime(&[Language::English]);
    let service = runtime.service();
    let created = service
        .create_game(service.prepare_create_game(Language::English, key("create")))
        .await
        .unwrap();
    let seat_one = service
        .seat_game(
            &created.access.seat_one,
            SeatGameQuery {
                game_id: created.game_id.clone(),
            },
        )
        .await
        .unwrap();
    let placements = seat_one
        .game
        .rack
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
    let placed = service
        .act(
            &created.access.seat_one,
            action(&created.game_id, 0, Seat::One, Move::Place { placements }),
        )
        .await
        .unwrap();
    assert!(matches!(
        placed.event.kind,
        GameEventKind::MovePlayed { .. }
    ));

    let seat_two = service
        .seat_game(
            &created.access.seat_two,
            SeatGameQuery {
                game_id: created.game_id.clone(),
            },
        )
        .await
        .unwrap();
    let exchanged = service
        .act(
            &created.access.seat_two,
            action(
                &created.game_id,
                1,
                Seat::Two,
                Move::Exchange {
                    tile_ids: vec![seat_two.game.rack.tiles()[0].id],
                },
            ),
        )
        .await
        .unwrap();
    assert!(matches!(
        exchanged.event.kind,
        GameEventKind::Exchanged { .. }
    ));

    let passed = service
        .act(
            &created.access.seat_one,
            action(&created.game_id, 2, Seat::One, Move::Pass),
        )
        .await
        .unwrap();
    assert!(matches!(passed.event.kind, GameEventKind::Passed { .. }));

    let resigned = service
        .act(
            &created.access.seat_two,
            action(&created.game_id, 3, Seat::Two, Move::Resign),
        )
        .await
        .unwrap();
    assert!(matches!(
        resigned.event.kind,
        GameEventKind::Resigned { .. }
    ));
    assert_eq!(resigned.game.public.state.phase, GamePhase::Finished);
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one application narrative compares preview, rejection, persistence, and rate limits"
)]
async fn practice_preview_is_bounded_exact_and_never_mutates_game_state() {
    let runtime = setup_runtime(&[Language::English]).with_preview_policy(PreviewPolicy {
        version: 7,
        max_requests: 2,
        window_ms: 60_000,
    });
    let service = runtime.service();
    let competitive = service
        .create_game(service.prepare_create_game(Language::English, key("competitive")))
        .await
        .unwrap();
    assert_eq!(competitive.public.state.mode, GameMode::Competitive);
    assert!(matches!(
        service
            .preview_tiles(
                &competitive.access.seat_one,
                MovePreviewCommand {
                    game_id: competitive.game_id,
                    expected_version: 0,
                    turn: Turn {
                        number: 0,
                        seat: Seat::One,
                    },
                    placements: Vec::new(),
                },
            )
            .await,
        Err(ApplicationError::PracticeOnly)
    ));

    let practice = service
        .create_game(service.prepare_create_game_with_mode(
            Language::English,
            GameMode::Practice,
            key("practice"),
        ))
        .await
        .unwrap();
    assert_eq!(practice.public.state.mode, GameMode::Practice);
    let before = service
        .seat_game(
            &practice.access.seat_one,
            SeatGameQuery {
                game_id: practice.game_id.clone(),
            },
        )
        .await
        .unwrap();
    let placements = before
        .game
        .rack
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
        .collect::<Vec<_>>();
    let preview = service
        .preview_tiles(
            &practice.access.seat_one,
            MovePreviewCommand {
                game_id: practice.game_id.clone(),
                expected_version: 0,
                turn: Turn {
                    number: 0,
                    seat: Seat::One,
                },
                placements: placements.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(preview.base_version, 0);

    let invalid_preview = service
        .preview_tiles(
            &practice.access.seat_one,
            MovePreviewCommand {
                game_id: practice.game_id.clone(),
                expected_version: 0,
                turn: Turn {
                    number: 0,
                    seat: Seat::One,
                },
                placements: Vec::new(),
            },
        )
        .await
        .unwrap_err();
    let after_previews = service
        .seat_game(
            &practice.access.seat_one,
            SeatGameQuery {
                game_id: practice.game_id.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(after_previews, before);
    assert!(matches!(
        service
            .preview_tiles(
                &practice.access.seat_one,
                MovePreviewCommand {
                    game_id: practice.game_id.clone(),
                    expected_version: 0,
                    turn: Turn {
                        number: 0,
                        seat: Seat::One,
                    },
                    placements: placements.clone(),
                },
            )
            .await,
        Err(ApplicationError::PreviewRateLimited {
            retry_after_ms: 60_000
        })
    ));

    let invalid_commit = service
        .act(
            &practice.access.seat_one,
            GameActionCommand {
                game_id: practice.game_id.clone(),
                expected_version: 0,
                turn: Turn {
                    number: 0,
                    seat: Seat::One,
                },
                idempotency_key: key("invalid-commit"),
                action: Move::Place {
                    placements: Vec::new(),
                },
            },
        )
        .await
        .unwrap_err();
    assert_eq!(invalid_preview.to_string(), invalid_commit.to_string());
    let committed = service
        .act(
            &practice.access.seat_one,
            GameActionCommand {
                game_id: practice.game_id,
                expected_version: 0,
                turn: Turn {
                    number: 0,
                    seat: Seat::One,
                },
                idempotency_key: key("valid-commit"),
                action: Move::Place { placements },
            },
        )
        .await
        .unwrap();
    assert_eq!(committed.event, preview.event);
    assert_eq!(committed.game.public.state.version, 1);
}

#[tokio::test]
async fn every_credential_rejects_cross_game_reuse() {
    let runtime = setup_runtime(&[Language::English]);
    let service = runtime.service();
    let first = service
        .create_game(service.prepare_create_game(Language::English, key("first")))
        .await
        .unwrap();
    let second = service
        .create_game(service.prepare_create_game(Language::English, key("second")))
        .await
        .unwrap();

    assert!(matches!(
        service
            .public_game(
                &first.access.public,
                PublicGameQuery {
                    game_id: second.game_id.clone()
                }
            )
            .await,
        Err(ApplicationError::WrongGameAuthority { .. })
    ));
    assert!(matches!(
        service
            .seat_game(
                &first.access.seat_one,
                SeatGameQuery {
                    game_id: second.game_id.clone()
                }
            )
            .await,
        Err(ApplicationError::WrongGameAuthority { .. })
    ));
    let spectator = human_spectator_credential(&runtime, &first.game_id).await;
    assert!(matches!(
        service
            .human_spectator_game(
                &spectator,
                HumanSpectatorGameQuery {
                    game_id: second.game_id.clone()
                }
            )
            .await,
        Err(ApplicationError::WrongGameAuthority { .. })
    ));
    let administrator = administrator_credential(&runtime, &first.game_id).await;
    assert!(matches!(
        service
            .administrator_game(
                &administrator,
                AdministratorGameQuery {
                    game_id: second.game_id.clone()
                }
            )
            .await,
        Err(ApplicationError::WrongGameAuthority { .. })
    ));

    let competitive = first.access.competitive(Seat::One);
    assert_eq!(competitive.public.game_id(), &first.game_id);
    assert_eq!(competitive.seat.game_id(), &first.game_id);
    assert_eq!(competitive.seat.seat(), Seat::One);
}

#[tokio::test]
async fn authorization_staleness_missing_inputs_and_engine_errors_fail_closed() {
    let runtime = setup_runtime(&[Language::English]);
    let service = runtime.service();
    let first = service
        .create_game(service.prepare_create_game(Language::English, key("first")))
        .await
        .unwrap();

    assert!(matches!(
        service
            .act(
                &first.access.seat_one,
                action(&first.game_id, 0, Seat::Two, Move::Pass)
            )
            .await,
        Err(ApplicationError::WrongSeatAuthority { .. })
    ));
    assert!(matches!(
        service
            .act(
                &first.access.seat_one,
                action(&first.game_id, 99, Seat::One, Move::Pass)
            )
            .await,
        Err(ApplicationError::ActionRejected(
            ActionRejection::VersionConflict
        ))
    ));
    assert!(matches!(
        service
            .act(
                &first.access.seat_one,
                action(
                    &first.game_id,
                    0,
                    Seat::One,
                    Move::Place {
                        placements: Vec::new()
                    }
                )
            )
            .await,
        Err(ApplicationError::ActionRejected(
            ActionRejection::IllegalAction { .. }
        ))
    ));
    let unchanged = service
        .public_game(
            &first.access.public,
            PublicGameQuery {
                game_id: first.game_id.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(unchanged.game.state.version, 0);

    assert!(matches!(
        service
            .public_game(
                &first.access.public,
                PublicGameQuery {
                    game_id: GameId::new("missing").unwrap()
                }
            )
            .await,
        Err(ApplicationError::WrongGameAuthority { .. })
    ));

    assert!(matches!(
        runtime
            .issue_public_viewer(&GameId::new("missing").unwrap())
            .await,
        Err(ApplicationError::Repository(RepositoryError::NotFound))
    ));
    let missing_game = GameId::new("missing").unwrap();
    for role in [
        CapabilityRole::HumanSpectator,
        CapabilityRole::Administrator,
    ] {
        assert!(matches!(
            runtime
                .issue_capability(privileged_request(missing_game.clone(), role))
                .await,
            Err(CapabilityError::Game(RepositoryError::NotFound))
        ));
    }

    let english_only = setup_runtime(&[Language::English]);
    let missing_pack = english_only
        .service()
        .prepare_create_game(Language::French, key("missing-pack"));
    assert!(matches!(
        english_only.service().create_game(missing_pack).await,
        Err(ApplicationError::MissingLexicon { .. })
    ));
}

#[tokio::test]
async fn role_results_are_serialization_isolated() {
    let runtime = setup_runtime(&[Language::English]);
    let service = runtime.service();
    let created = service
        .create_game(service.prepare_create_game(Language::English, key("create")))
        .await
        .unwrap();
    let public = service
        .public_game(
            &created.access.public,
            PublicGameQuery {
                game_id: created.game_id.clone(),
            },
        )
        .await
        .unwrap();
    let seat = service
        .seat_game(
            &created.access.seat_one,
            SeatGameQuery {
                game_id: created.game_id.clone(),
            },
        )
        .await
        .unwrap();
    let spectator_credential = human_spectator_credential(&runtime, &created.game_id).await;
    let spectator = service
        .human_spectator_game(
            &spectator_credential,
            HumanSpectatorGameQuery {
                game_id: created.game_id,
            },
        )
        .await
        .unwrap();

    let public_json = serde_json::to_string(&public).unwrap();
    assert!(!public_json.contains("\"rack\""));
    assert!(!public_json.contains("\"seed\""));
    assert!(!public_json.contains("\"bag\""));
    assert!(serde_json::from_str::<SeatGameView>(&public_json).is_err());

    let seat_json = serde_json::to_string(&seat).unwrap();
    assert!(!seat_json.contains("\"seed\""));
    assert!(!seat_json.contains("\"bag\""));
    assert!(!seat_json.contains("\"racks\""));
    assert!(!seat_json.contains("\"snapshot\""));
    assert!(serde_json::from_str::<HumanSpectatorGameView>(&seat_json).is_err());
    let spectator_json = serde_json::to_string(&spectator).unwrap();
    assert!(serde_json::from_str::<SeatGameView>(&spectator_json).is_err());
}

fn setup_runtime(languages: &[Language]) -> ApplicationRuntime {
    let lexicons = languages.iter().map(|language| {
        let ruleset = Ruleset::for_language(*language).unwrap();
        Arc::new(AcceptingLexicon(ruleset.lexicon)) as Arc<dyn WordValidator>
    });
    let repository: Arc<dyn GameRepository> = Arc::new(InMemoryGameRepository::default());
    let resolver: Arc<dyn LexiconResolver> = Arc::new(InMemoryLexiconResolver::new(lexicons));
    let ids: Arc<dyn GameIdSource> = Arc::new(SequenceGameIds::new("game"));
    let seeds: Arc<dyn SeedSource> = Arc::new(SequenceSeeds::new(7));
    let clock: Arc<dyn ApplicationClock> = Arc::new(FixedClock(UnixMillis(1_700_000_000_000)));
    ApplicationRuntime::new(
        repository,
        resolver,
        ids,
        seeds,
        clock,
        CapabilityAdapters::new(
            Arc::new(InMemoryCapabilityRepository::default()),
            Arc::new(SequenceCapabilityTokens::new(1)),
            CapabilityDigestKey::new([7; 32]),
        ),
    )
}

#[tokio::test]
async fn mutation_retries_return_the_exact_outcome_and_reject_key_reuse() {
    let runtime = setup_runtime(&[Language::English]);
    let service = runtime.service();
    let created = service
        .create_game(service.prepare_create_game(Language::English, key("create-retry")))
        .await
        .unwrap();
    let game_id = created.game_id.clone();
    let command = action(&created.game_id, 0, Seat::One, Move::Pass);
    let first = service
        .act(&created.access.seat_one, command.clone())
        .await
        .unwrap();
    let duplicate = service
        .act(&created.access.seat_one, command.clone())
        .await
        .unwrap();
    assert_eq!(duplicate, first);

    let mut reused = command;
    reused.action = Move::Resign;
    assert!(matches!(
        service.act(&created.access.seat_one, reused).await,
        Err(ApplicationError::ActionRejected(
            ActionRejection::IdempotencyConflict
        ))
    ));
    let observed = service
        .public_game(
            &created.access.public,
            PublicGameQuery {
                game_id: game_id.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(observed.game.state.version, 1);

    let mut stale = action(&game_id, 0, Seat::Two, Move::Pass);
    stale.idempotency_key = key("stale-retry");
    for _ in 0..2 {
        assert!(matches!(
            service.act(&created.access.seat_two, stale.clone()).await,
            Err(ApplicationError::ActionRejected(
                ActionRejection::VersionConflict
            ))
        ));
    }
}

#[tokio::test]
async fn invalid_attempt_policy_is_persisted_without_applying_the_rejected_move() {
    let policy = OperationalPolicy {
        version: 7,
        turn_duration_ms: 10,
        timeout_response: TimeoutResponse::Pass,
        invalid_attempt_limit: 2,
        invalid_attempt_response: InvalidAttemptResponse::Pass,
    };
    let (runtime, repository, _clock) = setup_reliability_runtime(policy);
    let service = runtime.service();
    let created = service
        .create_game(service.prepare_create_game(Language::English, key("create-invalid")))
        .await
        .unwrap();
    for attempt in 1..=2 {
        let mut command = action(
            &created.game_id,
            0,
            Seat::One,
            Move::Place {
                placements: Vec::new(),
            },
        );
        command.idempotency_key = key(&format!("invalid-{attempt}"));
        assert!(matches!(
            service.act(&created.access.seat_one, command).await,
            Err(ApplicationError::ActionRejected(
                ActionRejection::IllegalAction { .. }
            ))
        ));
    }
    let attempts = repository
        .load_invalid_attempt(&created.game_id, 0, Seat::One)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(attempts.count, 2);
    assert_eq!(attempts.policy_version, 7);
    let observed = service
        .public_game(
            &created.access.public,
            PublicGameQuery {
                game_id: created.game_id,
            },
        )
        .await
        .unwrap();
    assert_eq!(observed.game.state.version, 1);
    assert!(observed.game.state.board.iter().all(Option::is_none));
}

#[tokio::test]
async fn deadline_retry_and_player_race_commit_exactly_one_transition() {
    let policy = OperationalPolicy {
        version: 2,
        turn_duration_ms: 10,
        timeout_response: TimeoutResponse::Pass,
        invalid_attempt_limit: 3,
        invalid_attempt_response: InvalidAttemptResponse::RejectOnly,
    };
    let (runtime, _repository, clock) = setup_reliability_runtime(policy);
    let service = runtime.service();
    let created = service
        .create_game(service.prepare_create_game(Language::English, key("create-timeout")))
        .await
        .unwrap();
    let timeout = TimeoutCommand {
        game_id: created.game_id.clone(),
        expected_version: 0,
    };
    assert!(matches!(
        service.resolve_timeout(timeout.clone()).await,
        Err(ApplicationError::ActionRejected(
            ActionRejection::DeadlineNotReached
        ))
    ));
    clock.set(UnixMillis(1_010));
    let player = action(&created.game_id, 0, Seat::One, Move::Pass);
    let (player_result, timeout_result) = tokio::join!(
        service.act(&created.access.seat_one, player),
        service.resolve_timeout(timeout.clone())
    );
    assert_eq!(
        u8::from(player_result.is_ok()) + u8::from(timeout_result.is_ok()),
        1
    );
    if let Ok(result) = timeout_result {
        assert_eq!(service.resolve_timeout(timeout).await.unwrap(), result);
    }
    let observed = service
        .public_game(
            &created.access.public,
            PublicGameQuery {
                game_id: created.game_id,
            },
        )
        .await
        .unwrap();
    assert_eq!(observed.game.state.version, 1);
}

fn setup_reliability_runtime(
    policy: OperationalPolicy,
) -> (
    ApplicationRuntime,
    Arc<InMemoryGameRepository>,
    Arc<ManualClock>,
) {
    let ruleset = Ruleset::english_v1();
    let lexicon = Arc::new(AcceptingLexicon(ruleset.lexicon)) as Arc<dyn WordValidator>;
    let repository = Arc::new(InMemoryGameRepository::default());
    let clock = Arc::new(ManualClock::new(UnixMillis(1_000)));
    let runtime = ApplicationRuntime::new(
        repository.clone(),
        Arc::new(InMemoryLexiconResolver::new([lexicon])),
        Arc::new(SequenceGameIds::new("reliability")),
        Arc::new(SequenceSeeds::new(9)),
        clock.clone(),
        CapabilityAdapters::new(
            Arc::new(InMemoryCapabilityRepository::default()),
            Arc::new(SequenceCapabilityTokens::new(1)),
            CapabilityDigestKey::new([8; 32]),
        ),
    )
    .with_operational_policy(policy);
    (runtime, repository, clock)
}

async fn human_spectator_credential(
    runtime: &ApplicationRuntime,
    game_id: &GameId,
) -> HumanSpectatorCredential {
    let issued = runtime
        .issue_capability(privileged_request(
            game_id.clone(),
            CapabilityRole::HumanSpectator,
        ))
        .await
        .unwrap();
    match runtime
        .authenticate_capability(
            &issued.token.into_secret(),
            game_id,
            CapabilityScope::ObserveHumanSpectator,
        )
        .await
        .unwrap()
    {
        AuthenticatedCredential::HumanSpectator(credential) => credential,
        _ => panic!("spectator capability returned another credential role"),
    }
}

async fn administrator_credential(
    runtime: &ApplicationRuntime,
    game_id: &GameId,
) -> AdministratorCredential {
    let issued = runtime
        .issue_capability(privileged_request(
            game_id.clone(),
            CapabilityRole::Administrator,
        ))
        .await
        .unwrap();
    match runtime
        .authenticate_capability(
            &issued.token.into_secret(),
            game_id,
            CapabilityScope::ObserveAdministrator,
        )
        .await
        .unwrap()
    {
        AuthenticatedCredential::Administrator(credential) => credential,
        _ => panic!("administrator capability returned another credential role"),
    }
}

fn privileged_request(game_id: GameId, role: CapabilityRole) -> IssueCapabilityRequest {
    let scope = match role {
        CapabilityRole::HumanSpectator => CapabilityScope::ObserveHumanSpectator,
        CapabilityRole::Administrator => CapabilityScope::ObserveAdministrator,
        _ => panic!("fixture supports only privileged roles"),
    };
    IssueCapabilityRequest {
        game_id,
        role,
        scopes: BTreeSet::from([scope]),
        expires_at: UnixMillis(1_700_000_001_000),
        agent_run_id: None,
    }
}

fn key(value: &str) -> IdempotencyKey {
    IdempotencyKey::new(value).unwrap()
}

fn action(game_id: &GameId, version: u64, seat: Seat, action: Move) -> GameActionCommand {
    GameActionCommand {
        game_id: game_id.clone(),
        expected_version: version,
        turn: Turn {
            number: version,
            seat,
        },
        idempotency_key: key(&format!("action-{version}-{seat:?}")),
        action,
    }
}

fn assignment(tile: &PhysicalTile, index: usize) -> Tile {
    match &tile.face {
        TileFace::Letter(token) => Tile::letter(token.as_str()),
        TileFace::Blank => Tile::blank(if index == 0 { "A" } else { "B" }),
    }
}
