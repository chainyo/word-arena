use std::sync::Arc;

use word_arena_engine::{
    Game, GameSeed, Move, PUBLIC_REPLAY_SCHEMA_VERSION, PublicReplayBundle, Ruleset, Seat,
    WordValidator,
};
use word_arena_lexicon::{NormalizedKey, PackIdentity};

#[test]
fn public_bundle_replays_exactly_without_serialized_private_state() {
    let ruleset = Ruleset::english_v1();
    let validator = Arc::new(AcceptAll(ruleset.lexicon.clone()));
    let mut game = Game::create(
        "public-replay",
        ruleset,
        Some(validator.clone()),
        GameSeed::from_bytes([13; 32]),
    )
    .unwrap();
    let tile_id = game.rack(Seat::One).tiles()[0].id;
    game.apply_move(
        Seat::One,
        0,
        Move::Exchange {
            tile_ids: vec![tile_id],
        },
    )
    .unwrap();
    game.apply_move(Seat::Two, 1, Move::Resign).unwrap();
    let complete = game.replay_bundle().unwrap();
    assert!(!complete.private_events.is_empty());
    let public = PublicReplayBundle::from(&complete);
    let json = String::from_utf8(serde_json::to_vec(&public).unwrap()).unwrap();
    for forbidden in ["private_events", "rack_after", "drawn", "removed"] {
        assert!(!json.contains(forbidden));
    }

    let replayed = Game::replay_public(&public, Some(validator)).unwrap();
    assert_eq!(replayed.public_projection(), game.public_projection());
}

#[test]
fn public_replay_rejects_schema_seed_and_event_tampering() {
    let ruleset = Ruleset::french_v1();
    let validator = Arc::new(AcceptAll(ruleset.lexicon.clone()));
    let mut game = Game::create(
        "tamper",
        ruleset,
        Some(validator.clone()),
        GameSeed::from_bytes([21; 32]),
    )
    .unwrap();
    game.apply_move(Seat::One, 0, Move::Resign).unwrap();
    let public = PublicReplayBundle::from(&game.replay_bundle().unwrap());

    let mut incompatible = public.clone();
    incompatible.schema_version = PUBLIC_REPLAY_SCHEMA_VERSION + 1;
    assert!(Game::replay_public(&incompatible, Some(validator.clone())).is_err());
    let mut seed = public.clone();
    seed.seed_reveal[0] ^= 1;
    assert!(Game::replay_public(&seed, Some(validator.clone())).is_err());
    let mut event = public;
    event.events[1].sequence = 9;
    assert!(Game::replay_public(&event, Some(validator)).is_err());
}

#[derive(Debug)]
struct AcceptAll(PackIdentity);

impl WordValidator for AcceptAll {
    fn identity(&self) -> &PackIdentity {
        &self.0
    }

    fn contains(&self, _key: &NormalizedKey) -> bool {
        true
    }
}
