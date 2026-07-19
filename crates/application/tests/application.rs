#![cfg(feature = "test-support")]

use std::sync::Arc;

use word_arena_application::{
    AdministratorCredential, AdministratorGameQuery, ApplicationClock, ApplicationError,
    ApplicationRuntime, Authorizes, CompetitiveSeatCredential, GameActionCommand, GameId,
    GameIdSource, GameRepository, HumanSpectatorCredential, HumanSpectatorGameQuery,
    HumanSpectatorGameView, IdempotencyKey, LexiconResolver, PublicGameQuery,
    PublicViewerCredential, RepositoryError, SeatGameQuery, SeatGameView, SeedSource, UnixMillis,
    test_support::{
        FixedClock, InMemoryGameRepository, InMemoryLexiconResolver, SequenceGameIds, SequenceSeeds,
    },
};
use word_arena_engine::{
    Coordinate, GameError, GameEventKind, GamePhase, Language, Move, PhysicalTile, Placement,
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
        let command = service.prepare_create_game(language, key("create"));
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

        let spectator_credential = runtime
            .issue_human_spectator(&created.game_id)
            .await
            .unwrap();
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

        let administrator_credential = runtime.issue_administrator(&created.game_id).await.unwrap();
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
    let spectator = runtime.issue_human_spectator(&first.game_id).await.unwrap();
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
    let administrator = runtime.issue_administrator(&first.game_id).await.unwrap();
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
        Err(ApplicationError::Engine(GameError::StaleVersion { .. }))
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
        Err(ApplicationError::Engine(GameError::EmptyPlacement))
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
    assert!(matches!(
        runtime
            .issue_human_spectator(&GameId::new("missing").unwrap())
            .await,
        Err(ApplicationError::Repository(RepositoryError::NotFound))
    ));
    assert!(matches!(
        runtime
            .issue_administrator(&GameId::new("missing").unwrap())
            .await,
        Err(ApplicationError::Repository(RepositoryError::NotFound))
    ));

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
    let spectator_credential = runtime
        .issue_human_spectator(&created.game_id)
        .await
        .unwrap();
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
    ApplicationRuntime::new(repository, resolver, ids, seeds, clock)
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
