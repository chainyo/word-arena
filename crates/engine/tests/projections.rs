use std::sync::Arc;

use serde_json::Value;
use word_arena_engine::{
    Coordinate, EventVisibility, Game, GameEventKind, GameSeed, HumanSpectatorProjection,
    PhysicalTile, Placement, PublicProjection, ReplayBundle, Ruleset, Seat, SeatProjection, Tile,
    TileFace, TileId, WordValidator,
};
use word_arena_lexicon::{NormalizedKey, PackIdentity};

#[derive(Debug)]
struct FixtureLexicon {
    identity: PackIdentity,
}

impl WordValidator for FixtureLexicon {
    fn identity(&self) -> &PackIdentity {
        &self.identity
    }

    fn contains(&self, _key: &NormalizedKey) -> bool {
        true
    }
}

#[test]
fn role_serialization_enforces_live_privacy_boundaries() {
    let (game, _) = active_game(Ruleset::english_v1(), 88);
    let public = game.public_projection();
    let seat_one = game.seat_projection(Seat::One);
    let seat_two = game.seat_projection(Seat::Two);
    let spectator = game.human_spectator_projection();
    let administrator = game.administrator_projection();

    let public_value = serde_json::to_value(&public).unwrap();
    assert_no_keys(
        &public_value,
        &[
            "rack",
            "racks",
            "seed",
            "bag",
            "private_events",
            "drawn",
            "removed",
            "rack_after",
        ],
    );
    for event in &public.events {
        assert_eq!(event.visibility, EventVisibility::Public);
        assert_no_keys(
            &serde_json::to_value(event).unwrap(),
            &[
                "rack",
                "racks",
                "seed",
                "bag",
                "private_events",
                "drawn",
                "removed",
                "rack_after",
            ],
        );
    }

    let seat_one_value = serde_json::to_value(&seat_one).unwrap();
    assert_eq!(
        seat_one_value["rack"],
        serde_json::to_value(game.rack(Seat::One)).unwrap()
    );
    assert!(seat_one_value.get("racks").is_none());
    assert!(
        seat_one
            .private_events
            .iter()
            .all(|event| event.seat == Seat::One
                && event.visibility == EventVisibility::SeatPrivate(Seat::One))
    );
    assert!(
        seat_two
            .private_events
            .iter()
            .all(|event| event.seat == Seat::Two
                && event.visibility == EventVisibility::SeatPrivate(Seat::Two))
    );
    assert_ne!(seat_one.rack, seat_two.rack);

    let spectator_value = serde_json::to_value(&spectator).unwrap();
    assert_eq!(spectator.racks[0], *game.rack(Seat::One));
    assert_eq!(spectator.racks[1], *game.rack(Seat::Two));
    assert_no_keys(&spectator_value, &["seed", "bag"]);

    let admin_value = serde_json::to_value(administrator).unwrap();
    assert!(admin_value["snapshot"].get("seed").is_some());
    assert!(admin_value["snapshot"].get("bag").is_some());
    assert!(serde_json::from_value::<SeatProjection>(spectator_value).is_err());
    assert!(serde_json::from_value::<HumanSpectatorProjection>(seat_one_value).is_err());

    let created = serde_json::to_value(&public.events[0]).unwrap();
    assert_eq!(created["kind"]["type"], "created");
    assert!(created["kind"].get("rack_counts").is_some());
    assert!(created["kind"].get("racks").is_none());
}

#[test]
fn english_and_french_resume_and_replay_preserve_every_projection() {
    for (ruleset, seed) in [(Ruleset::english_v1(), 101), (Ruleset::french_v1(), 202)] {
        let (mut game, lexicon) = active_game(ruleset.clone(), seed);
        let snapshot = game.snapshot();
        let resumed = Game::resume(
            serde_json::from_slice(&serde_json::to_vec(&snapshot).unwrap()).unwrap(),
            ruleset,
            Some(Arc::clone(&lexicon)),
        )
        .unwrap();
        assert_all_projections(&resumed, &game);
        assert_eq!(resumed.snapshot(), snapshot);

        let player = game.public_state().current_player;
        let version = game.public_state().version;
        game.resign(player, version).unwrap();
        let bundle = game.replay_bundle().unwrap();
        let replayed = Game::replay(
            &serde_json::from_slice(&serde_json::to_vec(&bundle).unwrap()).unwrap(),
            Some(lexicon),
        )
        .unwrap();
        assert_all_projections(&replayed, &game);
        assert_eq!(replayed.snapshot(), game.snapshot());
        let bundle_value = serde_json::to_value(bundle).unwrap();
        assert_no_keys(
            &bundle_value,
            &["dictionary", "dictionary_words", "lexicon_words"],
        );
    }
}

#[test]
fn snapshot_tampering_is_rejected_before_resume() {
    let ruleset = Ruleset::english_v1();
    let (mut game, lexicon) = active_game(ruleset.clone(), 303);
    let player = game.public_state().current_player;
    game.resign(player, game.public_state().version).unwrap();
    let snapshot = game.snapshot();

    let mut cases = Vec::new();
    let mut wrong_schema = snapshot.clone();
    wrong_schema.schema_version += 1;
    cases.push(wrong_schema);

    let mut wrong_ruleset_hash = snapshot.clone();
    wrong_ruleset_hash.ruleset.content_sha256 = "0".repeat(64);
    cases.push(wrong_ruleset_hash);

    let mut wrong_pack = snapshot.clone();
    wrong_pack.state.lexicon.pack_version = "9.9.9".to_owned();
    cases.push(wrong_pack);

    let mut wrong_seed = snapshot.clone();
    wrong_seed.seed[0] ^= 1;
    cases.push(wrong_seed);

    let mut wrong_tile = snapshot.clone();
    wrong_tile.racks[0] = replace_first_id(&wrong_tile.racks[0], TileId(u16::MAX));
    cases.push(wrong_tile);

    let mut wrong_draw = snapshot.clone();
    wrong_draw.private_events[0].drawn[0].id = TileId(u16::MAX);
    cases.push(wrong_draw);

    let mut reordered = snapshot.clone();
    reordered.events.swap(0, 1);
    cases.push(reordered);

    let mut missing = snapshot.clone();
    missing.events.remove(1);
    cases.push(missing);

    let mut missing_private = snapshot.clone();
    missing_private.private_events.remove(0);
    cases.push(missing_private);

    let mut private_public_event = snapshot.clone();
    private_public_event.events[1].visibility = EventVisibility::SeatPrivate(Seat::One);
    cases.push(private_public_event);

    let mut wrong_result = snapshot.clone();
    wrong_result.state.result.as_mut().unwrap().winner = None;
    cases.push(wrong_result);

    for tampered in cases {
        assert!(Game::resume(tampered, ruleset.clone(), Some(Arc::clone(&lexicon))).is_err());
    }
}

#[test]
fn replay_tampering_and_privacy_invalid_json_are_rejected() {
    let ruleset = Ruleset::french_v1();
    let (mut game, lexicon) = active_game(ruleset, 404);
    let player = game.public_state().current_player;
    game.resign(player, game.public_state().version).unwrap();
    let bundle = game.replay_bundle().unwrap();

    let mut cases = Vec::new();
    let mut wrong_schema = bundle.clone();
    wrong_schema.schema_version += 1;
    cases.push(wrong_schema);

    let mut wrong_ruleset = bundle.clone();
    wrong_ruleset.ruleset_identity.content_sha256 = "f".repeat(64);
    cases.push(wrong_ruleset);

    let mut wrong_pack = bundle.clone();
    wrong_pack.lexicon.pack_version = "0.0.0".to_owned();
    cases.push(wrong_pack);

    let mut wrong_seed = bundle.clone();
    wrong_seed.seed_reveal[31] ^= 1;
    cases.push(wrong_seed);

    let mut wrong_draw = bundle.clone();
    wrong_draw.private_events[0].drawn[0].id = TileId(u16::MAX);
    cases.push(wrong_draw);

    let mut public_private_event = bundle.clone();
    public_private_event.private_events[0].visibility = EventVisibility::Public;
    cases.push(public_private_event);

    let mut reordered = bundle.clone();
    reordered.events.swap(0, 1);
    cases.push(reordered);

    let mut wrong_result = bundle.clone();
    let resigned = wrong_result
        .events
        .iter_mut()
        .find_map(|event| match &mut event.kind {
            GameEventKind::Resigned { result, .. } => Some(result),
            _ => None,
        })
        .unwrap();
    resigned.winner = None;
    cases.push(wrong_result);

    for tampered in cases {
        assert!(Game::replay(&tampered, Some(Arc::clone(&lexicon))).is_err());
    }

    let mut public_json = serde_json::to_value(game.public_projection()).unwrap();
    public_json
        .as_object_mut()
        .unwrap()
        .insert("seed".to_owned(), Value::String("leak".to_owned()));
    assert!(serde_json::from_value::<PublicProjection>(public_json).is_err());

    let mut bundle_json = serde_json::to_value(bundle).unwrap();
    bundle_json
        .as_object_mut()
        .unwrap()
        .insert("dictionary_words".to_owned(), Value::Array(Vec::new()));
    assert!(serde_json::from_value::<ReplayBundle>(bundle_json).is_err());
}

fn active_game(ruleset: Ruleset, seed_number: u64) -> (Game, Arc<dyn WordValidator>) {
    let lexicon: Arc<dyn WordValidator> = Arc::new(FixtureLexicon {
        identity: ruleset.lexicon.clone(),
    });
    let mut game = Game::create(
        format!("projection-{}", ruleset.language.code()),
        ruleset,
        Some(Arc::clone(&lexicon)),
        numbered_seed(seed_number),
    )
    .unwrap();
    let placements = game.rack(Seat::One).tiles()[..2]
        .iter()
        .enumerate()
        .map(|(index, tile)| assignment(tile, 7, 7 + u8::try_from(index).unwrap()))
        .collect();
    game.play_tiles(Seat::One, 0, placements).unwrap();
    let exchange_ids = game.rack(Seat::Two).tiles()[..2]
        .iter()
        .map(|tile| tile.id)
        .collect();
    game.exchange_tiles(Seat::Two, 1, exchange_ids).unwrap();
    (game, lexicon)
}

fn numbered_seed(value: u64) -> GameSeed {
    let mut bytes = [0_u8; 32];
    bytes[..8].copy_from_slice(&value.to_be_bytes());
    GameSeed::from_bytes(bytes)
}

fn assignment(tile: &PhysicalTile, row: u8, column: u8) -> Placement {
    match &tile.face {
        TileFace::Letter(token) => Placement::new(
            tile.id,
            Coordinate::new(row, column),
            Tile::letter(token.as_str()),
        ),
        TileFace::Blank => Placement::new(tile.id, Coordinate::new(row, column), Tile::blank("A")),
    }
}

fn replace_first_id(rack: &word_arena_engine::Rack, id: TileId) -> word_arena_engine::Rack {
    let mut tiles = rack.tiles().to_vec();
    tiles[0].id = id;
    word_arena_engine::Rack::new(tiles)
}

fn assert_all_projections(actual: &Game, expected: &Game) {
    assert_eq!(actual.public_projection(), expected.public_projection());
    assert_eq!(
        actual.seat_projection(Seat::One),
        expected.seat_projection(Seat::One)
    );
    assert_eq!(
        actual.seat_projection(Seat::Two),
        expected.seat_projection(Seat::Two)
    );
    assert_eq!(
        actual.human_spectator_projection(),
        expected.human_spectator_projection()
    );
    assert_eq!(
        actual.administrator_projection(),
        expected.administrator_projection()
    );
}

fn assert_no_keys(value: &Value, forbidden: &[&str]) {
    match value {
        Value::Object(object) => {
            for key in forbidden {
                assert!(
                    !object.contains_key(*key),
                    "forbidden key {key:?} in {object:?}"
                );
            }
            for nested in object.values() {
                assert_no_keys(nested, forbidden);
            }
        }
        Value::Array(items) => {
            for item in items {
                assert_no_keys(item, forbidden);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}
