use std::{collections::BTreeSet, sync::Arc};

use proptest::prelude::*;
use word_arena_engine::{
    Bag, BoardTile, Coordinate, Game, GameError, GameEventKind, GameSeed, Language, PhysicalTile,
    Placement, Player, Ruleset, Seat, Tile, TileFace, TileId, WordValidator, prepare_initial_deal,
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
fn english_golden_game_scores_premiums_cross_words_refills_and_replays() {
    let ruleset = Ruleset::english_v1();
    let lexicon = validator(&ruleset, &["ABE", "AC", "BAR", "CAT", "ET"]);
    let mut game = Game::create(
        "english-golden",
        ruleset.clone(),
        Some(Arc::clone(&lexicon)),
        numbered_seed(9_106),
    )
    .unwrap();

    let opening = vec![
        regular(&game, Seat::One, 7, 6, "C"),
        regular(&game, Seat::One, 7, 7, "A"),
        regular(&game, Seat::One, 7, 8, "T"),
    ];
    let first = game.play_tiles(Player::One, 0, opening).unwrap();
    let GameEventKind::MovePlayed {
        words,
        score,
        draw_count,
        ..
    } = &first.kind
    else {
        panic!("expected move event");
    };
    assert_eq!((words[0].letter_score, words[0].word_multiplier), (5, 2));
    assert_eq!((*score, *draw_count), (10, 3));

    let bar = vec![
        regular(&game, Seat::Two, 6, 7, "B"),
        regular(&game, Seat::Two, 8, 7, "R"),
    ];
    game.play_tiles(Player::Two, 1, bar).unwrap();
    let crosses = vec![
        regular(&game, Seat::One, 6, 6, "A"),
        regular(&game, Seat::One, 6, 8, "E"),
    ];
    let event = game.play_tiles(Player::One, 2, crosses).unwrap();
    let GameEventKind::MovePlayed { words, score, .. } = &event.kind else {
        panic!("expected move event");
    };
    assert_eq!(
        words
            .iter()
            .map(|word| (word.normalized.as_str(), word.score))
            .collect::<Vec<_>>(),
        [("ABE", 7), ("AC", 5), ("ET", 3)]
    );
    assert_eq!(*score, 15);
    assert_eq!(game.public_state().scores, [25, 5]);
    assert_eq!(game.public_state().bag_count, 79);
    assert_eq!(game.public_state().rack_counts, [7, 7]);
    assert_eq!(game.private_events(Seat::One).count(), 2);

    let snapshot = game.snapshot();
    let resumed = Game::resume(
        serde_json::from_slice(&serde_json::to_vec(&snapshot).unwrap()).unwrap(),
        ruleset.clone(),
        Some(Arc::clone(&lexicon)),
    )
    .unwrap();
    assert_eq!(resumed.public_state(), game.public_state());
    assert_eq!(resumed.rack(Seat::One), game.rack(Seat::One));

    game.finish().unwrap();
    let bundle = game.replay_bundle().expect("finished replay");
    let replayed = Game::replay(
        &serde_json::from_slice(&serde_json::to_vec(&bundle).unwrap()).unwrap(),
        Some(lexicon),
    )
    .unwrap();
    assert_eq!(replayed.public_state(), game.public_state());
    assert_eq!(replayed.events(), game.events());
    assert_eq!(
        replayed.private_events(Seat::One).collect::<Vec<_>>(),
        game.private_events(Seat::One).collect::<Vec<_>>()
    );
}

#[test]
fn french_accents_blank_and_ligature_boundaries_use_physical_tiles() {
    let ruleset = Ruleset::french_v1();
    let lexicon = validator(&ruleset, &["ETE"]);
    let mut game = Game::create(
        "french-golden",
        ruleset.clone(),
        Some(Arc::clone(&lexicon)),
        numbered_seed(164),
    )
    .unwrap();
    let placements = vec![
        blank(&game, Seat::One, 7, 6, "É"),
        regular(&game, Seat::One, 7, 7, "T"),
        Placement::new(
            owned_id(&game, Seat::One, Some("E")),
            Coordinate::new(7, 8),
            Tile::letter("É"),
        ),
    ];
    let event = game.play_tiles(Player::One, 0, placements).unwrap();
    let GameEventKind::MovePlayed {
        placements,
        words,
        score,
        ..
    } = &event.kind
    else {
        panic!("expected move event");
    };
    assert_eq!(
        placements
            .iter()
            .map(|placement| placement.tile.letter.as_str())
            .collect::<Vec<_>>(),
        ["E", "T", "E"]
    );
    assert_eq!(
        (words[0].text.as_str(), words[0].normalized.as_str()),
        ("ETE", "ETE")
    );
    assert_eq!(*score, 4, "blank E is zero and the center doubles the word");

    let before = authoritative_bytes(&game);
    let e_id = owned_id(&game, Seat::Two, None);
    assert!(matches!(
        game.play_tiles(
            Player::Two,
            1,
            vec![Placement::new(
                e_id,
                Coordinate::new(6, 7),
                Tile::letter("Œ")
            )]
        ),
        Err(GameError::InvalidTileToken { token, normalized })
            if token == "Œ" && normalized == "OE"
    ));
    assert_eq!(authoritative_bytes(&game), before);

    let mut noncanonical = game.snapshot();
    noncanonical.state.board[7 * 15 + 8]
        .as_mut()
        .unwrap()
        .letter = "É".to_owned();
    assert!(matches!(
        Game::resume(noncanonical, ruleset, Some(lexicon)),
        Err(GameError::NonCanonicalBoardTile { token, canonical })
            if token == "É" && canonical == "E"
    ));
}

#[test]
fn full_rack_bingo_scores_once_and_refills_atomically() {
    let ruleset = Ruleset::english_v1();
    let lexicon = validator(&ruleset, &["READING"]);
    let mut game = Game::create("bingo", ruleset, Some(lexicon), numbered_seed(10_403)).unwrap();
    let placements = "READING"
        .chars()
        .enumerate()
        .map(|(index, letter)| {
            regular(
                &game,
                Seat::One,
                7,
                4 + u8::try_from(index).unwrap(),
                &letter.to_string(),
            )
        })
        .collect();
    let event = game.play_tiles(Seat::One, 0, placements).unwrap();
    let GameEventKind::MovePlayed {
        words,
        bingo_bonus,
        score,
        draw_count,
        ..
    } = event.kind
    else {
        panic!("expected move event");
    };
    assert_eq!((words[0].letter_score, words[0].word_multiplier), (9, 2));
    assert_eq!((bingo_bonus, score, draw_count), (50, 68, 7));
    assert_eq!(
        (game.public_state().bag_count, game.rack(Seat::One).len()),
        (79, 7)
    );
}

#[test]
fn french_ligature_spelling_requires_four_physical_tiles() {
    let ruleset = Ruleset::french_v1();
    let lexicon = validator(&ruleset, &["OEUF"]);
    let mut game =
        Game::create("french-ligature", ruleset, Some(lexicon), numbered_seed(63)).unwrap();
    let one_tile_ligature = Placement::new(
        owned_id(&game, Seat::One, Some("O")),
        Coordinate::new(7, 6),
        Tile::letter("Œ"),
    );
    let before = authoritative_bytes(&game);
    assert!(matches!(
        game.play_tiles(Seat::One, 0, vec![one_tile_ligature]),
        Err(GameError::InvalidTileToken { normalized, .. }) if normalized == "OE"
    ));
    assert_eq!(authoritative_bytes(&game), before);

    let placements = ["O", "E", "U", "F"]
        .into_iter()
        .enumerate()
        .map(|(index, letter)| {
            regular(
                &game,
                Seat::One,
                7,
                6 + u8::try_from(index).unwrap(),
                letter,
            )
        })
        .collect();
    let event = game.play_tiles(Seat::One, 0, placements).unwrap();
    let GameEventKind::MovePlayed {
        placements, words, ..
    } = event.kind
    else {
        panic!("expected move event");
    };
    assert_eq!(placements.len(), 4);
    assert_eq!(words[0].text, "OEUF");
}

#[test]
fn depleted_bag_commits_a_partial_rack_without_inventing_tiles() {
    let ruleset = Ruleset::english_v1();
    let bootstrap = validator(&ruleset, &["AA"]);
    let game = Game::create(
        "depleted",
        ruleset.clone(),
        Some(bootstrap),
        numbered_seed(9_106),
    )
    .unwrap();
    let mut snapshot = game.snapshot();
    let bag_tiles: Vec<PhysicalTile> =
        serde_json::from_value(serde_json::to_value(&snapshot.bag).unwrap()).unwrap();
    for (index, tile) in bag_tiles.iter().enumerate() {
        snapshot.state.board[index] = Some(board_tile(tile));
    }
    snapshot.bag = Bag::new(Vec::new());
    snapshot.state.bag_count = 0;

    let assignments = snapshot.racks[0].tiles()[..2]
        .iter()
        .enumerate()
        .map(|(index, tile)| assignment(tile, 6, u8::try_from(index).unwrap()))
        .collect::<Vec<_>>();
    let mut words = vec![
        assignments
            .iter()
            .map(|placement| placement.tile.letter.as_str())
            .collect::<String>(),
    ];
    for column in 0..2_u8 {
        let mut word = String::new();
        for row in 0..6_u8 {
            word.push_str(
                snapshot.state.board[usize::from(row) * 15 + usize::from(column)]
                    .as_ref()
                    .unwrap()
                    .letter
                    .as_str(),
            );
        }
        word.push_str(&assignments[usize::from(column)].tile.letter);
        words.push(word);
    }
    let word_refs = words.iter().map(String::as_str).collect::<Vec<_>>();
    let lexicon = validator(&ruleset, &word_refs);
    let mut game = Game::resume(snapshot, ruleset.clone(), Some(Arc::clone(&lexicon))).unwrap();
    let event = game.play_tiles(Seat::One, 0, assignments).unwrap();
    let GameEventKind::MovePlayed { draw_count, .. } = event.kind else {
        panic!("expected move event");
    };
    assert_eq!(draw_count, 0);
    assert_eq!(
        (game.public_state().bag_count, game.rack(Seat::One).len()),
        (0, 5)
    );
    assert!(Game::resume(game.snapshot(), ruleset, Some(lexicon)).is_ok());
}

#[test]
fn ownership_face_turn_and_version_rejections_are_fully_atomic() {
    let ruleset = Ruleset::english_v1();
    let lexicon = validator(&ruleset, &["CAT"]);
    let game = Game::create("ownership", ruleset, Some(lexicon), numbered_seed(9_106)).unwrap();
    let cat = vec![
        regular(&game, Seat::One, 7, 6, "C"),
        regular(&game, Seat::One, 7, 7, "A"),
        regular(&game, Seat::One, 7, 8, "T"),
    ];
    let wrong_seat_id = game.rack(Seat::Two).tiles()[0].id;
    let owned = cat[0].tile_id;
    let second_owned = cat[1].tile_id;

    let attempts = [
        (
            Player::Two,
            0,
            cat.clone(),
            "wrong seat must fail before ownership",
        ),
        (Player::One, 1, cat.clone(), "stale version must fail"),
        (
            Player::One,
            0,
            vec![
                Placement::new(owned, Coordinate::new(7, 7), Tile::letter("C")),
                Placement::new(second_owned, Coordinate::new(7, 7), Tile::letter("A")),
            ],
            "duplicate coordinate must fail",
        ),
        (
            Player::One,
            0,
            vec![Placement::new(
                wrong_seat_id,
                Coordinate::new(7, 7),
                Tile::letter("A"),
            )],
            "opponent tile must fail",
        ),
        (
            Player::One,
            0,
            vec![
                Placement::new(owned, Coordinate::new(7, 7), Tile::letter("C")),
                Placement::new(owned, Coordinate::new(7, 8), Tile::letter("C")),
            ],
            "duplicate ID must fail",
        ),
        (
            Player::One,
            0,
            vec![Placement::new(
                owned,
                Coordinate::new(7, 7),
                Tile::blank("C"),
            )],
            "forged blank must fail",
        ),
        (
            Player::One,
            0,
            vec![Placement::new(
                owned,
                Coordinate::new(7, 7),
                Tile::letter("Z"),
            )],
            "substituted token must fail",
        ),
        (
            Player::One,
            0,
            vec![Placement::new(
                TileId(u16::MAX),
                Coordinate::new(7, 7),
                Tile::letter("A"),
            )],
            "forged ID must fail",
        ),
    ];
    for (player, version, placements, reason) in attempts {
        let mut candidate = Game::resume(
            game.snapshot(),
            Ruleset::english_v1(),
            Some(validator(&Ruleset::english_v1(), &["CAT"])),
        )
        .unwrap();
        let before = authoritative_bytes(&candidate);
        assert!(
            candidate.play_tiles(player, version, placements).is_err(),
            "{reason}"
        );
        assert_eq!(authoritative_bytes(&candidate), before, "{reason}");
    }
}

#[test]
fn invalid_cross_word_and_overflows_leave_authoritative_state_unchanged() {
    let ruleset = Ruleset::english_v1();
    let lexicon = validator(&ruleset, &["ABE", "AC", "BAR", "CAT"]);
    let mut game = Game::create(
        "atomic-invalid",
        ruleset.clone(),
        Some(Arc::clone(&lexicon)),
        numbered_seed(9_106),
    )
    .unwrap();
    let cat = vec![
        regular(&game, Seat::One, 7, 6, "C"),
        regular(&game, Seat::One, 7, 7, "A"),
        regular(&game, Seat::One, 7, 8, "T"),
    ];
    game.play_tiles(Player::One, 0, cat).unwrap();
    let bar = vec![
        regular(&game, Seat::Two, 6, 7, "B"),
        regular(&game, Seat::Two, 8, 7, "R"),
    ];
    game.play_tiles(Player::Two, 1, bar).unwrap();
    let occupied = vec![regular(&game, Seat::One, 7, 7, "T")];
    let before = authoritative_bytes(&game);
    assert!(matches!(
        game.play_tiles(Player::One, 2, occupied),
        Err(GameError::OccupiedSquare { .. })
    ));
    assert_eq!(authoritative_bytes(&game), before);

    let disconnected = vec![regular(&game, Seat::One, 0, 0, "T")];
    let before = authoritative_bytes(&game);
    assert!(matches!(
        game.play_tiles(Player::One, 2, disconnected),
        Err(GameError::DisconnectedPlacement)
    ));
    assert_eq!(authoritative_bytes(&game), before);

    let invalid = vec![
        regular(&game, Seat::One, 6, 6, "A"),
        regular(&game, Seat::One, 6, 8, "E"),
    ];
    let before = authoritative_bytes(&game);
    assert!(matches!(
        game.play_tiles(Player::One, 2, invalid),
        Err(GameError::InvalidWord { normalized, .. }) if normalized == "ET"
    ));
    assert_eq!(authoritative_bytes(&game), before);

    let base = Game::create(
        "overflow",
        ruleset.clone(),
        Some(validator(&ruleset, &["CAT"])),
        numbered_seed(9_106),
    )
    .unwrap();
    let mut score_snapshot = base.snapshot();
    score_snapshot.state.scores[0] = u32::MAX;
    let mut score_game = Game::resume(
        score_snapshot,
        ruleset.clone(),
        Some(validator(&ruleset, &["CAT"])),
    )
    .unwrap();
    let score_move = vec![
        regular(&score_game, Seat::One, 7, 6, "C"),
        regular(&score_game, Seat::One, 7, 7, "A"),
        regular(&score_game, Seat::One, 7, 8, "T"),
    ];
    let before = authoritative_bytes(&score_game);
    assert!(matches!(
        score_game.play_tiles(Player::One, 0, score_move),
        Err(GameError::ScoreOverflow)
    ));
    assert_eq!(authoritative_bytes(&score_game), before);

    let mut version_snapshot = base.snapshot();
    version_snapshot.state.version = u64::MAX;
    let mut version_game = Game::resume(
        version_snapshot,
        ruleset.clone(),
        Some(validator(&ruleset, &["CAT"])),
    )
    .unwrap();
    let version_move = vec![
        regular(&version_game, Seat::One, 7, 6, "C"),
        regular(&version_game, Seat::One, 7, 7, "A"),
        regular(&version_game, Seat::One, 7, 8, "T"),
    ];
    let before = authoritative_bytes(&version_game);
    assert!(matches!(
        version_game.play_tiles(Player::One, u64::MAX, version_move),
        Err(GameError::VersionOverflow)
    ));
    assert_eq!(authoritative_bytes(&version_game), before);
}

#[test]
fn missing_or_substituted_pack_fails_before_create_resume_or_replay() {
    let ruleset = Ruleset::english_v1();
    assert!(matches!(
        Game::create("missing", ruleset.clone(), None, numbered_seed(9_106)),
        Err(GameError::MissingLexicon { .. })
    ));

    let exact = validator(&ruleset, &["CAT"]);
    let mut game = Game::create(
        "identity",
        ruleset.clone(),
        Some(Arc::clone(&exact)),
        numbered_seed(9_106),
    )
    .unwrap();
    game.finish().unwrap();
    let snapshot = game.snapshot();
    let bundle = game.replay_bundle().unwrap();

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
    assert!(matches!(
        Game::replay(&bundle, Some(substituted)),
        Err(GameError::IncompatibleLexicon(_))
    ));
    assert!(Game::resume(snapshot, ruleset, Some(exact)).is_ok());
}

#[test]
fn caller_cannot_rebind_a_static_ruleset_to_another_pack() {
    let mut tampered = Ruleset::english_v1();
    tampered.lexicon.content_sha256 = "f".repeat(64);
    let matching_tampered = validator(&tampered, &["CAT"]);
    assert!(matches!(
        Game::create(
            "tampered-rules",
            tampered,
            Some(matching_tampered),
            numbered_seed(9_106)
        ),
        Err(GameError::InvalidRuleset { .. })
    ));
}

proptest! {
    #[test]
    fn accepted_openings_conserve_tiles_and_decompose_scores(seed_bytes in any::<[u8; 32]>(), french in any::<bool>()) {
        let ruleset = if french { Ruleset::french_v1() } else { Ruleset::english_v1() };
        let seed = GameSeed::from_bytes(seed_bytes);
        let deal = prepare_initial_deal(&ruleset, &seed).unwrap();
        let rack = deal.rack(Seat::One);
        let assignments = rack.tiles()[..2]
            .iter()
            .enumerate()
            .map(|(index, tile)| assignment(tile, 7, 7 + u8::try_from(index).unwrap()))
            .collect::<Vec<_>>();
        let word = assignments.iter().map(|placement| placement.tile.letter.as_str()).collect::<String>();
        let lexicon = validator(&ruleset, &[&word]);
        let mut game = Game::create("property", ruleset.clone(), Some(Arc::clone(&lexicon)), seed).unwrap();
        let event = game.play_tiles(Seat::One, 0, assignments).unwrap();
        let GameEventKind::MovePlayed { words, bingo_bonus, score, .. } = &event.kind else {
            unreachable!();
        };
        let decomposed = words.iter().try_fold(*bingo_bonus, |sum, word| sum.checked_add(word.score)).unwrap();
        prop_assert_eq!(*score, decomposed);
        let snapshot = game.snapshot();
        prop_assert!(Game::resume(snapshot, ruleset, Some(lexicon)).is_ok());
    }
}

fn validator(ruleset: &Ruleset, words: &[&str]) -> Arc<dyn WordValidator> {
    Arc::new(FixtureLexicon {
        identity: ruleset.lexicon.clone(),
        words: words.iter().map(|word| (*word).to_owned()).collect(),
    })
}

fn numbered_seed(value: u64) -> GameSeed {
    let mut bytes = [0_u8; 32];
    bytes[..8].copy_from_slice(&value.to_be_bytes());
    GameSeed::from_bytes(bytes)
}

fn owned_id(game: &Game, seat: Seat, token: Option<&str>) -> TileId {
    game.rack(seat)
        .tiles()
        .iter()
        .find(|tile| match (&tile.face, token) {
            (TileFace::Letter(actual), Some(expected)) => actual.as_str() == expected,
            (TileFace::Blank | TileFace::Letter(_), None) => true,
            (TileFace::Blank, Some(_)) => false,
        })
        .expect("required deterministic rack tile")
        .id
}

fn regular(game: &Game, seat: Seat, row: u8, column: u8, letter: &str) -> Placement {
    Placement::new(
        owned_id(game, seat, Some(letter)),
        Coordinate::new(row, column),
        Tile::letter(letter),
    )
}

fn blank(game: &Game, seat: Seat, row: u8, column: u8, assigned: &str) -> Placement {
    let tile_id = game
        .rack(seat)
        .tiles()
        .iter()
        .find(|tile| tile.face == TileFace::Blank)
        .expect("required deterministic blank")
        .id;
    Placement::new(tile_id, Coordinate::new(row, column), Tile::blank(assigned))
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

fn board_tile(tile: &PhysicalTile) -> BoardTile {
    match &tile.face {
        TileFace::Letter(token) => BoardTile {
            tile_id: tile.id,
            letter: token.as_str().to_owned(),
            is_blank: false,
        },
        TileFace::Blank => BoardTile {
            tile_id: tile.id,
            letter: "A".to_owned(),
            is_blank: true,
        },
    }
}

fn authoritative_bytes(game: &Game) -> Vec<u8> {
    let snapshot = game.snapshot();
    let private = [
        game.private_events(Seat::One).cloned().collect::<Vec<_>>(),
        game.private_events(Seat::Two).cloned().collect::<Vec<_>>(),
    ];
    serde_json::to_vec(&(snapshot, game.events(), private)).unwrap()
}
