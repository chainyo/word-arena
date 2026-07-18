use std::{collections::BTreeSet, sync::Arc};

use word_arena_engine::{
    Coordinate, Game, GameError, GameEventKind, Language, Placement, Player, Ruleset, Tile,
    WordValidator,
};
use word_arena_lexicon::{NormalizedKey, PackIdentity};

#[derive(serde::Deserialize)]
struct Registry {
    packs: Vec<RegistryPack>,
}

#[derive(serde::Deserialize)]
struct RegistryPack {
    pack_id: String,
    pack_version: String,
    format_version: u32,
    normalization_version: u32,
    content_sha256: String,
}

#[derive(Debug)]
struct FixtureLexicon {
    identity: PackIdentity,
    words: BTreeSet<String>,
}

impl WordValidator for FixtureLexicon {
    fn identity(&self) -> &PackIdentity {
        &self.identity
    }

    fn contains(&self, key: &NormalizedKey) -> bool {
        self.words.contains(key.as_ref())
    }
}

#[test]
fn production_rulesets_pin_complete_release_identities() {
    let english = Ruleset::english_v1();
    assert_eq!(english.language, Language::English);
    assert_eq!(english.lexicon.pack_id, "word-arena-en-world-v1");
    assert_eq!(english.lexicon.pack_version, "1.0.0");
    assert_eq!(english.lexicon.format_version, 1);
    assert_eq!(english.lexicon.normalization.version, 1);
    assert_eq!(
        english.lexicon.content_sha256,
        "27faaa6b78de526d7e7681bf1af45ce952cb0400897190c79eab7c67b278a54b"
    );

    let french = Ruleset::french_v1();
    assert_eq!(french.language, Language::French);
    assert_eq!(french.lexicon.pack_id, "word-arena-fr-v1");
    assert_eq!(french.lexicon.pack_version, "1.0.0");
    assert_eq!(french.lexicon.format_version, 1);
    assert_eq!(french.lexicon.normalization.version, 1);
    assert_eq!(
        french.lexicon.content_sha256,
        "c926a5f1ead63711d041277c9bfb3af23f3a460bb6edf57ff66408552c495193"
    );

    assert!(matches!(
        Ruleset::for_language(Language::German),
        Err(GameError::RulesetUnavailable {
            language: Language::German
        })
    ));
}

#[test]
fn production_ruleset_pins_match_the_install_registry() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lexicons/registry.toml");
    let registry: Registry =
        toml::from_str(&std::fs::read_to_string(path).expect("committed pack registry"))
            .expect("valid pack registry");
    for ruleset in [Ruleset::english_v1(), Ruleset::french_v1()] {
        let record = registry
            .packs
            .iter()
            .find(|record| record.pack_id == ruleset.lexicon.pack_id)
            .expect("ruleset pack in registry");
        assert_eq!(record.pack_version, ruleset.lexicon.pack_version);
        assert_eq!(record.format_version, ruleset.lexicon.format_version);
        assert_eq!(
            record.normalization_version,
            ruleset.lexicon.normalization.version
        );
        assert_eq!(record.content_sha256, ruleset.lexicon.content_sha256);
    }
}

#[test]
fn english_golden_game_validates_multiple_words_and_replays_byte_equivalently() {
    let ruleset = Ruleset::english_v1();
    let lexicon = validator(&ruleset, &["ABE", "AC", "BAR", "CAT", "ET"]);
    let mut game = Game::create(
        "english-golden",
        ruleset.clone(),
        Some(Arc::clone(&lexicon)),
    )
    .expect("exact English pack");

    game.play_tiles(
        Player::One,
        vec![regular(7, 6, "C"), regular(7, 7, "A"), regular(7, 8, "T")],
    )
    .expect("CAT opening");
    game.play_tiles(Player::Two, vec![regular(6, 7, "B"), regular(8, 7, "R")])
        .expect("BAR through existing A");
    let event = game
        .play_tiles(Player::One, vec![regular(6, 6, "A"), regular(6, 8, "E")])
        .expect("ABE plus AC and ET cross words");
    let GameEventKind::MovePlayed { words, score, .. } = &event.kind else {
        panic!("expected move event");
    };
    assert_eq!(
        words
            .iter()
            .map(|word| word.normalized.as_str())
            .collect::<Vec<_>>(),
        ["ABE", "AC", "ET"]
    );
    assert_eq!(*score, 11);

    let snapshot = game.snapshot();
    assert_eq!(snapshot.state.lexicon, ruleset.lexicon);
    let result = game.finish().expect("finish golden game");
    assert_eq!(result.lexicon, ruleset.lexicon);
    assert!(
        game.events()
            .iter()
            .all(|event| event.lexicon == ruleset.lexicon)
    );
    let bundle = game.replay_bundle().expect("complete replay history");
    assert_eq!(bundle.lexicon, ruleset.lexicon);
    let encoded_bundle = serde_json::to_vec(&bundle).expect("serialize replay");
    let decoded = serde_json::from_slice(&encoded_bundle).expect("deserialize replay");
    let expected_public = serde_json::to_vec(game.public_state()).expect("serialize public state");
    let replayed = Game::replay(&decoded, Some(lexicon)).expect("deterministic English replay");
    let actual_public =
        serde_json::to_vec(replayed.public_state()).expect("serialize replayed state");
    assert_eq!(actual_public, expected_public);
    assert_eq!(replayed.events(), game.events());
}

#[test]
fn french_blank_assignment_uses_folded_lookup_and_zero_points() {
    let ruleset = Ruleset::french_v1();
    let lexicon = validator(&ruleset, &["ETE"]);
    let mut game = Game::create("french-golden", ruleset.clone(), Some(Arc::clone(&lexicon)))
        .expect("exact French pack");
    let event = game
        .play_tiles(
            Player::One,
            vec![blank(7, 6, "É"), regular(7, 7, "T"), regular(7, 8, "É")],
        )
        .expect("accented French word");
    let GameEventKind::MovePlayed { words, score, .. } = &event.kind else {
        panic!("expected move event");
    };
    assert_eq!(words.len(), 1);
    assert_eq!(words[0].text, "ÉTÉ");
    assert_eq!(words[0].normalized, "ETE");
    assert_eq!(*score, 2, "blank É contributes zero points");
    assert!(
        game.public_state()
            .tile_at(Coordinate::new(7, 6))
            .expect("blank on board")
            .is_blank
    );
    game.finish().expect("finish French game");
    let public = serde_json::to_vec(game.public_state()).expect("French public state");
    let replay = Game::replay(
        &game.replay_bundle().expect("complete replay history"),
        Some(lexicon),
    )
    .expect("French replay");
    assert_eq!(
        serde_json::to_vec(replay.public_state()).expect("replayed French state"),
        public
    );
}

#[test]
fn invalid_cross_word_fails_before_any_state_mutation() {
    let ruleset = Ruleset::english_v1();
    let lexicon = validator(&ruleset, &["ABE", "AC", "BAR", "CAT"]);
    let mut game = Game::create("atomic-invalid", ruleset, Some(lexicon)).unwrap();
    game.play_tiles(
        Player::One,
        vec![regular(7, 6, "C"), regular(7, 7, "A"), regular(7, 8, "T")],
    )
    .unwrap();
    game.play_tiles(Player::Two, vec![regular(6, 7, "B"), regular(8, 7, "R")])
        .unwrap();
    let before = game.public_state().clone();
    let event_count = game.events().len();

    assert!(matches!(
        game.play_tiles(
            Player::One,
            vec![regular(6, 6, "A"), regular(6, 8, "E")]
        ),
        Err(GameError::InvalidWord { normalized, .. }) if normalized == "ET"
    ));
    assert_eq!(game.public_state(), &before);
    assert_eq!(game.events().len(), event_count);
}

#[test]
fn missing_or_substituted_pack_fails_before_create_resume_or_replay() {
    let ruleset = Ruleset::english_v1();
    assert!(matches!(
        Game::create("missing", ruleset.clone(), None),
        Err(GameError::MissingLexicon { .. })
    ));

    let exact = validator(&ruleset, &["CAT"]);
    let mut game = Game::create("identity", ruleset.clone(), Some(Arc::clone(&exact))).unwrap();
    game.play_tiles(
        Player::One,
        vec![regular(7, 6, "C"), regular(7, 7, "A"), regular(7, 8, "T")],
    )
    .unwrap();
    let snapshot = game.snapshot();
    let bundle = game.replay_bundle().expect("complete replay history");
    let persisted = snapshot.clone();

    let mut substituted_identity = ruleset.lexicon.clone();
    substituted_identity.content_sha256 = "f".repeat(64);
    let substituted: Arc<dyn WordValidator> = Arc::new(FixtureLexicon {
        identity: substituted_identity,
        words: BTreeSet::from(["CAT".to_owned()]),
    });
    assert!(matches!(
        Game::resume(
            snapshot.clone(),
            ruleset.clone(),
            Some(Arc::clone(&substituted))
        ),
        Err(GameError::IncompatibleLexicon(_))
    ));
    assert_eq!(snapshot, persisted);
    assert!(matches!(
        Game::replay(&bundle, Some(substituted)),
        Err(GameError::IncompatibleLexicon(_))
    ));
    let resumed = Game::resume(snapshot, ruleset, Some(exact)).expect("exact resume");
    assert_eq!(resumed.public_state(), game.public_state());
    assert!(resumed.replay_bundle().is_none());
}

#[test]
fn caller_cannot_rebind_a_static_ruleset_to_another_pack() {
    let mut tampered = Ruleset::english_v1();
    tampered.lexicon.content_sha256 = "f".repeat(64);
    let matching_tampered = validator(&tampered, &["CAT"]);

    assert!(matches!(
        Game::create("tampered-rules", tampered, Some(matching_tampered)),
        Err(GameError::InvalidRuleset { .. })
    ));
}

#[test]
fn defensive_arithmetic_failures_are_atomic() {
    let ruleset = Ruleset::english_v1();
    let lexicon = validator(&ruleset, &["CAT"]);
    let game = Game::create("overflow", ruleset.clone(), Some(Arc::clone(&lexicon))).unwrap();

    let mut score_snapshot = game.snapshot();
    score_snapshot.state.scores[0] = u32::MAX;
    let mut score_game =
        Game::resume(score_snapshot, ruleset.clone(), Some(Arc::clone(&lexicon))).unwrap();
    let before = score_game.public_state().clone();
    assert!(matches!(
        score_game.play_tiles(
            Player::One,
            vec![regular(7, 6, "C"), regular(7, 7, "A"), regular(7, 8, "T")]
        ),
        Err(GameError::ScoreOverflow)
    ));
    assert_eq!(score_game.public_state(), &before);

    let mut version_snapshot = game.snapshot();
    version_snapshot.state.version = u64::MAX;
    let mut version_game = Game::resume(version_snapshot, ruleset, Some(lexicon)).unwrap();
    let before = version_game.public_state().clone();
    assert!(matches!(
        version_game.play_tiles(
            Player::One,
            vec![regular(7, 6, "C"), regular(7, 7, "A"), regular(7, 8, "T")]
        ),
        Err(GameError::VersionOverflow)
    ));
    assert_eq!(version_game.public_state(), &before);
}

fn validator(ruleset: &Ruleset, words: &[&str]) -> Arc<dyn WordValidator> {
    Arc::new(FixtureLexicon {
        identity: ruleset.lexicon.clone(),
        words: words.iter().map(|word| (*word).to_owned()).collect(),
    })
}

fn regular(row: u8, column: u8, letter: &str) -> Placement {
    Placement::new(Coordinate::new(row, column), Tile::letter(letter))
}

fn blank(row: u8, column: u8, assigned_letter: &str) -> Placement {
    Placement::new(Coordinate::new(row, column), Tile::blank(assigned_letter))
}
