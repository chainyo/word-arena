use std::collections::BTreeMap;

use word_arena_engine::{
    Bag, BoardDefinition, Coordinate, GameError, Move, PhysicalTile, Player, Premium, Rack,
    Ruleset, RulesetDefinitionError, Score, Seat, TileFace, TileId, TileToken, Turn, Violation,
};

const ENGLISH_RULESET_SHA256: &str =
    "e36324473bd0d7e4203e451d3ae604fbf5323ae654962b22f061df0ca392af58";
const FRENCH_RULESET_SHA256: &str =
    "edcd507e82ea304373484880ea7898520654e4d5b73be605476c1ca8c7d9e6ba";

#[test]
fn built_in_rulesets_have_stable_complete_identities() {
    for (ruleset, expected_hash, expected_tiles) in [
        (Ruleset::english_v1(), ENGLISH_RULESET_SHA256, 100),
        (Ruleset::french_v1(), FRENCH_RULESET_SHA256, 102),
    ] {
        ruleset.validate().expect("valid built-in ruleset");
        assert_eq!(ruleset.game.total_tiles(), expected_tiles);
        assert_eq!(ruleset.identity().content_sha256, expected_hash);

        let encoded = serde_json::to_vec(&ruleset).expect("serialize ruleset");
        let decoded: Ruleset = serde_json::from_slice(&encoded).expect("deserialize ruleset");
        assert_eq!(decoded, ruleset);
        assert_eq!(decoded.identity(), ruleset.identity());
    }
}

#[test]
fn classic_board_is_complete_symmetric_and_has_expected_premiums() {
    let board = &Ruleset::english_v1().game.board;
    assert_eq!(
        (board.width, board.height, board.squares.len()),
        (15, 15, 225)
    );
    assert_eq!(
        board
            .square(Coordinate::new(7, 7))
            .map(|square| square.premium),
        Some(Premium::DoubleWord)
    );

    let mut counts = BTreeMap::new();
    for square in &board.squares {
        *counts.entry(premium_name(square.premium)).or_insert(0) += 1;
        let horizontal = Coordinate::new(square.coordinate.row, 14 - square.coordinate.column);
        let vertical = Coordinate::new(14 - square.coordinate.row, square.coordinate.column);
        assert_eq!(board.square(horizontal).unwrap().premium, square.premium);
        assert_eq!(board.square(vertical).unwrap().premium, square.premium);
    }
    assert_eq!(counts["double_letter"], 24);
    assert_eq!(counts["triple_letter"], 12);
    assert_eq!(counts["double_word"], 17);
    assert_eq!(counts["triple_word"], 8);
    assert_eq!(counts["normal"], 164);
}

#[test]
fn english_and_french_tile_distributions_match_their_rulesets() {
    let english = Ruleset::english_v1();
    assert_tile(&english, "A", 9, 1);
    assert_tile(&english, "E", 12, 1);
    assert_tile(&english, "Q", 1, 10);
    assert_tile(&english, "Z", 1, 10);
    assert_eq!(english.game.blank().unwrap().count, 2);
    assert_eq!(english.game.blank().unwrap().value, 0);

    let french = Ruleset::french_v1();
    assert_tile(&french, "A", 9, 1);
    assert_tile(&french, "E", 15, 1);
    assert_tile(&french, "K", 1, 10);
    assert_tile(&french, "Q", 1, 8);
    assert_eq!(french.game.blank().unwrap().count, 2);
    assert_eq!(french.game.blank().unwrap().value, 0);

    for ruleset in [&english, &french] {
        assert_eq!(ruleset.game.rack_capacity, 7);
        assert_eq!(ruleset.game.bingo_bonus, 50);
        assert_eq!(ruleset.game.exchange_minimum, 7);
        assert_eq!(ruleset.game.scoreless_turn_limit, 6);
        assert_eq!(ruleset.game.tiles.len(), 27);
    }
}

#[test]
fn physical_tokens_are_exactly_one_canonical_board_letter() {
    assert_eq!(TileToken::new("E").unwrap().as_str(), "E");
    for invalid in ["", "e", "É", "Œ", "OE", "-"] {
        assert!(TileToken::new(invalid).is_err(), "accepted {invalid:?}");
    }

    let physical = PhysicalTile {
        id: TileId(42),
        face: TileFace::Letter(TileToken::new("E").unwrap()),
    };
    let rack = Rack::new(vec![physical.clone()]);
    let bag = Bag::new(vec![PhysicalTile {
        id: TileId(43),
        face: TileFace::Blank,
    }]);
    assert_eq!(rack.tiles(), &[physical]);
    assert_eq!(rack.len(), 1);
    assert!(!rack.is_empty());
    assert_eq!(bag.len(), 1);
    assert!(!bag.is_empty());
}

#[test]
fn core_action_turn_score_and_violation_models_round_trip() {
    let turn = Turn {
        number: 12,
        seat: Seat::Two,
    };
    assert_eq!(turn.seat, Player::Two);
    assert_eq!(Score::new(-4).checked_add(7).unwrap().value(), 3);
    assert!(Score::new(i32::MAX).checked_add(1).is_none());

    let actions = [
        Move::Pass,
        Move::Resign,
        Move::Exchange {
            tile_ids: vec![TileId(3)],
        },
    ];
    for action in actions {
        let bytes = serde_json::to_vec(&action).unwrap();
        assert_eq!(serde_json::from_slice::<Move>(&bytes).unwrap(), action);
    }
    let violation = Violation::TileNotOwned;
    assert_eq!(
        serde_json::from_str::<Violation>(&serde_json::to_string(&violation).unwrap()).unwrap(),
        violation
    );
}

#[test]
fn malformed_board_limits_and_tile_sets_are_rejected() {
    let mut missing_square = Ruleset::english_v1();
    missing_square.game.board.squares.pop();
    assert!(matches!(
        missing_square.validate_definition(),
        Err(RulesetDefinitionError::Board { .. })
    ));

    let mut asymmetric = Ruleset::english_v1();
    asymmetric.game.board.squares[1].premium = Premium::TripleLetter;
    assert!(matches!(
        asymmetric.validate_definition(),
        Err(RulesetDefinitionError::PremiumAsymmetry { .. })
    ));

    let mut wrong_center = Ruleset::english_v1();
    wrong_center.game.board.squares[7 * 15 + 7].premium = Premium::Normal;
    assert!(matches!(
        wrong_center.validate_definition(),
        Err(RulesetDefinitionError::Board { .. })
    ));

    let mut missing_face = Ruleset::english_v1();
    missing_face.game.tiles.pop();
    assert!(matches!(
        missing_face.validate_definition(),
        Err(RulesetDefinitionError::Tiles { .. })
    ));

    let mut duplicate_face = Ruleset::english_v1();
    duplicate_face.game.tiles[1] = duplicate_face.game.tiles[0].clone();
    assert!(matches!(
        duplicate_face.validate_definition(),
        Err(RulesetDefinitionError::Tiles { .. })
    ));

    let mut invalid_limit = Ruleset::english_v1();
    invalid_limit.game.exchange_minimum = 6;
    assert!(matches!(
        invalid_limit.validate_definition(),
        Err(RulesetDefinitionError::Limit { .. })
    ));

    let mut zero_rack = Ruleset::english_v1();
    zero_rack.game.rack_capacity = 0;
    assert!(matches!(
        zero_rack.validate_definition(),
        Err(RulesetDefinitionError::Limit { .. })
    ));

    let mut overflowing_counts = Ruleset::english_v1();
    overflowing_counts.game.tiles[0].count = u16::MAX;
    overflowing_counts.game.tiles[1].count = u16::MAX;
    assert!(matches!(
        overflowing_counts.validate_definition(),
        Err(RulesetDefinitionError::Tiles { .. })
    ));
}

#[test]
fn structurally_valid_changes_cannot_rebind_a_builtin_ruleset() {
    let mut tampered = Ruleset::english_v1();
    tampered.game.bingo_bonus = 51;
    assert!(tampered.validate_definition().is_ok());
    assert!(matches!(
        tampered.validate(),
        Err(GameError::InvalidRuleset { .. })
    ));

    let mut wrong_shape = Ruleset::french_v1();
    wrong_shape.game.board = BoardDefinition {
        width: 0,
        height: 0,
        squares: vec![],
    };
    assert!(matches!(
        wrong_shape.validate(),
        Err(GameError::InvalidRulesetDefinition { .. })
    ));
}

fn assert_tile(ruleset: &Ruleset, token: &str, count: u16, value: u16) {
    let definition = ruleset.game.letter(token).expect("letter definition");
    assert_eq!((definition.count, definition.value), (count, value));
}

const fn premium_name(premium: Premium) -> &'static str {
    match premium {
        Premium::Normal => "normal",
        Premium::DoubleLetter => "double_letter",
        Premium::TripleLetter => "triple_letter",
        Premium::DoubleWord => "double_word",
        Premium::TripleWord => "triple_word",
    }
}
