use std::{collections::BTreeSet, sync::Arc};

use word_arena_engine::{
    Coordinate, Game, GameError, GameEventKind, GameMode, GamePhase, GameSeed, Move, PhysicalTile,
    Placement, Rack, Ruleset, Score, Seat, TerminalReason, Tile, TileFace, TileId, WordValidator,
};
use word_arena_lexicon::{NormalizedKey, PackIdentity};

#[derive(Debug)]
struct FixtureLexicon {
    identity: PackIdentity,
    words: BTreeSet<String>,
    accept_all: bool,
}

#[test]
fn four_player_turns_rotate_and_replay_with_four_private_racks() {
    let ruleset = Ruleset::english_v1();
    let lexicon = validator(&ruleset, &[]);
    let mut game = Game::create_with_mode_and_players(
        "four-players",
        ruleset,
        Some(Arc::clone(&lexicon)),
        numbered_seed(404),
        GameMode::Competitive,
        4,
    )
    .unwrap();

    assert_eq!(game.public_state().scores, vec![Score::ZERO; 4]);
    assert_eq!(game.public_state().rack_counts, vec![7; 4]);
    assert_eq!(game.human_spectator_projection().racks.len(), 4);

    for (version, seat) in Seat::ALL.into_iter().enumerate() {
        game.apply_move(seat, version as u64, Move::Pass).unwrap();
    }

    assert_eq!(game.public_state().current_player, Seat::One);
    assert_eq!(game.public_state().version, 4);
    game.apply_move(Seat::One, 4, Move::Pass).unwrap();
    game.apply_move(Seat::Two, 5, Move::Pass).unwrap();
    let bundle = game.replay_bundle().unwrap();
    let replayed = Game::replay(&bundle, Some(lexicon)).unwrap();
    assert_eq!(replayed.public_state(), game.public_state());
    assert_eq!(replayed.human_spectator_projection().racks.len(), 4);
}

impl WordValidator for FixtureLexicon {
    fn identity(&self) -> &PackIdentity {
        &self.identity
    }

    fn contains(&self, key: &NormalizedKey) -> bool {
        self.accept_all || self.words.contains(key.as_ref())
    }
}

#[test]
fn passes_finish_at_the_scoreless_limit_and_replay_exactly() {
    let ruleset = Ruleset::english_v1();
    let lexicon = validator(&ruleset, &[]);
    let mut game = Game::create(
        "passes",
        ruleset.clone(),
        Some(Arc::clone(&lexicon)),
        numbered_seed(7),
    )
    .unwrap();
    let opening_scores = game.public_state().scores.clone();
    for version in 0..6 {
        let seat = if version % 2 == 0 {
            Seat::One
        } else {
            Seat::Two
        };
        let event = game.apply_move(seat, version, Move::Pass).unwrap();
        let GameEventKind::Passed {
            scoreless_turns_after,
            result,
            ..
        } = event.kind
        else {
            panic!("expected pass event");
        };
        assert_eq!(scoreless_turns_after, u8::try_from(version + 1).unwrap());
        assert_eq!(result.is_some(), version == 5);
    }
    assert_eq!(game.public_state().phase, GamePhase::Finished);
    let result = game.result().unwrap();
    assert_eq!(result.reason, TerminalReason::ScorelessTurns);
    assert!(result.scores[0] < opening_scores[0]);
    assert!(result.scores[1] < opening_scores[1]);
    assert_terminal_replay(&game, lexicon);
}

#[test]
fn exchange_is_deterministic_private_and_conservative() {
    let ruleset = Ruleset::english_v1();
    let lexicon = validator(&ruleset, &[]);
    let mut first = Game::create(
        "exchange",
        ruleset.clone(),
        Some(Arc::clone(&lexicon)),
        numbered_seed(44),
    )
    .unwrap();
    let mut second = Game::create(
        "exchange",
        ruleset.clone(),
        Some(Arc::clone(&lexicon)),
        numbered_seed(44),
    )
    .unwrap();
    let selected = first.rack(Seat::One).tiles()[..2]
        .iter()
        .map(|tile| tile.id)
        .rev()
        .collect::<Vec<_>>();
    let before_ids = selected.iter().copied().collect::<BTreeSet<_>>();
    let event = first
        .apply_move(
            Seat::One,
            0,
            Move::Exchange {
                tile_ids: selected.clone(),
            },
        )
        .unwrap();
    let second_event = second.exchange_tiles(Seat::One, 0, selected).unwrap();
    assert_eq!(event, second_event);
    assert_eq!(first.snapshot(), second.snapshot());
    let GameEventKind::Exchanged {
        tile_ids,
        bag_count_after,
        ..
    } = event.kind
    else {
        panic!("expected exchange event");
    };
    assert!(tile_ids.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(bag_count_after, 86);
    let private = first.private_events(Seat::One).next().unwrap();
    assert_eq!(private.removed.len(), 2);
    assert_eq!(private.drawn.len(), 2);
    assert!(
        private
            .drawn
            .iter()
            .all(|tile| !before_ids.contains(&tile.id))
    );
    assert_eq!(first.rack(Seat::One).len(), 7);

    for version in 1..6 {
        let seat = first.public_state().current_player;
        first.pass(seat, version).unwrap();
    }
    assert_eq!(
        first.result().unwrap().reason,
        TerminalReason::ScorelessTurns
    );
    assert_terminal_replay(&first, lexicon);
}

#[test]
fn invalid_exchanges_and_checked_score_boundaries_are_atomic() {
    let ruleset = Ruleset::english_v1();
    let lexicon = validator(&ruleset, &[]);
    let game = Game::create(
        "invalid-exchange",
        ruleset.clone(),
        Some(Arc::clone(&lexicon)),
        numbered_seed(55),
    )
    .unwrap();
    let owned = game.rack(Seat::One).tiles()[0].id;
    let attempts = [Vec::new(), vec![owned, owned], vec![TileId(u16::MAX)]];
    for tile_ids in attempts {
        let mut candidate =
            Game::resume(game.snapshot(), ruleset.clone(), Some(Arc::clone(&lexicon))).unwrap();
        let before = authoritative_bytes(&candidate);
        assert!(candidate.exchange_tiles(Seat::One, 0, tile_ids).is_err());
        assert_eq!(authoritative_bytes(&candidate), before);
    }

    let broad_lexicon = accepting_validator(&ruleset);
    let mut depleted_game = Game::create(
        "small-bag-exchange",
        ruleset.clone(),
        Some(broad_lexicon),
        numbered_seed(55),
    )
    .unwrap();
    for (index, row) in [7, 8, 9, 10, 11, 12, 13, 14, 6, 5, 4, 3]
        .into_iter()
        .enumerate()
    {
        let player = depleted_game.public_state().current_player;
        let placements = depleted_game
            .rack(player)
            .tiles()
            .iter()
            .enumerate()
            .map(|(column, tile)| assignment(tile, row, 4 + u8::try_from(column).unwrap()))
            .collect();
        depleted_game
            .play_tiles(player, u64::try_from(index).unwrap(), placements)
            .unwrap();
    }
    assert_eq!(depleted_game.public_state().bag_count, 2);
    let exchange_id = depleted_game.rack(Seat::One).tiles()[0].id;
    let before = authoritative_bytes(&depleted_game);
    assert!(matches!(
        depleted_game.exchange_tiles(Seat::One, 12, vec![exchange_id]),
        Err(GameError::ExchangeBagTooSmall { actual: 2, .. })
    ));
    assert_eq!(authoritative_bytes(&depleted_game), before);
    assert!(Score::new(i32::MIN).checked_add(-1).is_none());
    assert!(Score::new(i32::MAX).checked_add(1).is_none());
}

#[test]
fn resignation_is_explicit_immutable_and_rejects_every_later_action() {
    let ruleset = Ruleset::french_v1();
    let lexicon = validator(&ruleset, &["AA"]);
    let mut game = Game::create(
        "resignation",
        ruleset,
        Some(Arc::clone(&lexicon)),
        numbered_seed(164),
    )
    .unwrap();
    let scores_before = game.public_state().scores.clone();
    let event = game.apply_move(Seat::One, 0, Move::Resign).unwrap();
    let GameEventKind::Resigned { result, .. } = event.kind else {
        panic!("expected resignation event");
    };
    assert_eq!(
        result.reason,
        TerminalReason::Resignation {
            resigned: Seat::One
        }
    );
    assert_eq!(result.winner, Some(Seat::Two));
    assert_eq!(result.scores, scores_before);
    let frozen = authoritative_bytes(&game);
    assert!(matches!(
        game.pass(Seat::One, 1),
        Err(GameError::GameFinished)
    ));
    assert!(matches!(
        game.exchange_tiles(Seat::One, 1, vec![]),
        Err(GameError::GameFinished)
    ));
    assert!(matches!(
        game.play_tiles(Seat::One, 1, Vec::new()),
        Err(GameError::GameFinished)
    ));
    assert!(matches!(
        game.resign(Seat::One, 1),
        Err(GameError::GameFinished)
    ));
    assert_eq!(authoritative_bytes(&game), frozen);
    assert_terminal_replay(&game, lexicon);
}

#[test]
fn zero_score_blank_placement_counts_toward_completion() {
    let ruleset = Ruleset::english_v1();
    let (seed, tile_ids) = seed_with_two_opening_blanks(&ruleset);
    let lexicon = validator(&ruleset, &["AA"]);
    let mut game = Game::create(
        "zero-score",
        ruleset.clone(),
        Some(Arc::clone(&lexicon)),
        seed,
    )
    .unwrap();
    for version in 0..4 {
        let player = game.public_state().current_player;
        game.pass(player, version).unwrap();
    }
    let event = game
        .play_tiles(
            Seat::One,
            4,
            vec![
                Placement::new(tile_ids[0], Coordinate::new(7, 7), Tile::blank("A")),
                Placement::new(tile_ids[1], Coordinate::new(7, 8), Tile::blank("A")),
            ],
        )
        .unwrap();
    let GameEventKind::MovePlayed { score, result, .. } = event.kind else {
        panic!("expected placement event");
    };
    assert_eq!(score, 0);
    assert!(result.is_none());
    assert_eq!(game.public_state().scoreless_turns, 5);
    game.pass(Seat::Two, 5).unwrap();
    assert_eq!(
        game.result().unwrap().reason,
        TerminalReason::ScorelessTurns
    );
}

#[test]
fn scoreless_completion_can_tie_and_blank_racks_deduct_zero() {
    let ruleset = Ruleset::french_v1();
    let lexicon = validator(&ruleset, &[]);
    let seed = seed_with_equal_rack_values_and_blank(&ruleset);
    let mut game = Game::create("tie", ruleset.clone(), Some(lexicon), seed).unwrap();
    assert!(Seat::ALL.iter().any(|seat| {
        game.rack(*seat)
            .tiles()
            .iter()
            .any(|tile| tile.face == TileFace::Blank)
    }));
    for version in 0..6 {
        let player = game.public_state().current_player;
        game.pass(player, version).unwrap();
    }
    let result = game.result().unwrap();
    assert_eq!(result.scores[0], result.scores[1]);
    assert_eq!(result.winner, None);
}

#[test]
fn natural_bag_exhaustion_awards_opponent_deductions_and_replays() {
    let ruleset = Ruleset::english_v1();
    let lexicon = accepting_validator(&ruleset);
    let seed = seed_with_blank_in_last_rack(&ruleset, &lexicon);
    let mut game = Game::create(
        "natural-endgame",
        ruleset.clone(),
        Some(Arc::clone(&lexicon)),
        seed,
    )
    .unwrap();
    let rows = [7, 8, 9, 10, 11, 12, 13, 14, 6, 5, 4, 3, 2, 1];
    let mut before_final = vec![Score::ZERO, Score::ZERO];
    let mut remaining_value = 0;
    let mut final_move_score = 0;
    for (index, row) in rows.into_iter().enumerate() {
        let player = game.public_state().current_player;
        if index == 13 {
            before_final = game.public_state().scores.clone();
            remaining_value = rack_value(&ruleset, game.rack(Seat::One));
        }
        let placements = game
            .rack(player)
            .tiles()
            .iter()
            .enumerate()
            .map(|(column, tile)| assignment(tile, row, 4 + u8::try_from(column).unwrap()))
            .collect();
        let event = game
            .play_tiles(player, u64::try_from(index).unwrap(), placements)
            .unwrap();
        if index == 13 {
            let GameEventKind::MovePlayed { score, .. } = event.kind else {
                unreachable!();
            };
            final_move_score = i32::try_from(score).unwrap();
        }
    }
    let result = game.result().unwrap();
    assert_eq!(
        result.reason,
        TerminalReason::RackEmptied {
            outgoing: Seat::Two
        }
    );
    assert_eq!(game.public_state().bag_count, 0);
    assert!(game.rack(Seat::Two).is_empty());
    assert_eq!(game.rack(Seat::One).len(), 2);
    assert!(
        game.rack(Seat::One)
            .tiles()
            .iter()
            .any(|tile| tile.face == TileFace::Blank)
    );
    assert_eq!(
        result.scores,
        [
            before_final[0].checked_add(-remaining_value).unwrap(),
            before_final[1]
                .checked_add(final_move_score)
                .unwrap()
                .checked_add(remaining_value)
                .unwrap(),
        ]
    );
    assert_terminal_replay(&game, lexicon);
}

fn validator(ruleset: &Ruleset, words: &[&str]) -> Arc<dyn WordValidator> {
    Arc::new(FixtureLexicon {
        identity: ruleset.lexicon.clone(),
        words: words.iter().map(|word| (*word).to_owned()).collect(),
        accept_all: false,
    })
}

fn accepting_validator(ruleset: &Ruleset) -> Arc<dyn WordValidator> {
    Arc::new(FixtureLexicon {
        identity: ruleset.lexicon.clone(),
        words: BTreeSet::new(),
        accept_all: true,
    })
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

fn rack_value(ruleset: &Ruleset, rack: &Rack) -> i32 {
    rack.tiles().iter().fold(0, |total, tile| {
        total
            + match &tile.face {
                TileFace::Letter(token) => i32::from(ruleset.letter_value(token.as_str()).unwrap()),
                TileFace::Blank => 0,
            }
    })
}

fn seed_with_two_opening_blanks(ruleset: &Ruleset) -> (GameSeed, [TileId; 2]) {
    for value in 0..10_000 {
        let seed = numbered_seed(value);
        let game = Game::create(
            "blank-search",
            ruleset.clone(),
            Some(validator(ruleset, &[])),
            seed.clone(),
        )
        .unwrap();
        let blanks = game
            .rack(Seat::One)
            .tiles()
            .iter()
            .filter(|tile| tile.face == TileFace::Blank)
            .map(|tile| tile.id)
            .collect::<Vec<_>>();
        if let [first, second] = blanks.as_slice() {
            return (seed, [*first, *second]);
        }
    }
    panic!("deterministic search must find two opening blanks");
}

fn seed_with_equal_rack_values_and_blank(ruleset: &Ruleset) -> GameSeed {
    for value in 0..10_000 {
        let seed = numbered_seed(value);
        let game = Game::create(
            "tie-search",
            ruleset.clone(),
            Some(validator(ruleset, &[])),
            seed.clone(),
        )
        .unwrap();
        let has_blank = Seat::TWO_PLAYER.iter().any(|seat| {
            game.rack(*seat)
                .tiles()
                .iter()
                .any(|tile| tile.face == TileFace::Blank)
        });
        if has_blank
            && rack_value(ruleset, game.rack(Seat::One))
                == rack_value(ruleset, game.rack(Seat::Two))
        {
            return seed;
        }
    }
    panic!("deterministic search must find equal final rack values");
}

fn seed_with_blank_in_last_rack(ruleset: &Ruleset, lexicon: &Arc<dyn WordValidator>) -> GameSeed {
    for value in 0..1_000 {
        let seed = numbered_seed(value);
        let game = Game::create(
            "end-seed-search",
            ruleset.clone(),
            Some(Arc::clone(lexicon)),
            seed.clone(),
        )
        .unwrap();
        let bag: Vec<PhysicalTile> =
            serde_json::from_value(serde_json::to_value(&game.snapshot().bag).unwrap()).unwrap();
        if bag[..2].iter().any(|tile| tile.face == TileFace::Blank) {
            return seed;
        }
    }
    panic!("deterministic search must find a final-rack blank");
}

fn assert_terminal_replay(game: &Game, lexicon: Arc<dyn WordValidator>) {
    let bundle = game.replay_bundle().expect("terminal replay bundle");
    let decoded = serde_json::from_slice(&serde_json::to_vec(&bundle).unwrap()).unwrap();
    let replayed = Game::replay(&decoded, Some(lexicon)).unwrap();
    assert_eq!(replayed.public_state(), game.public_state());
    assert_eq!(replayed.events(), game.events());
    for seat in Seat::TWO_PLAYER {
        assert_eq!(
            replayed.private_events(seat).collect::<Vec<_>>(),
            game.private_events(seat).collect::<Vec<_>>()
        );
    }
}

fn authoritative_bytes(game: &Game) -> Vec<u8> {
    let private = [
        game.private_events(Seat::One).cloned().collect::<Vec<_>>(),
        game.private_events(Seat::Two).cloned().collect::<Vec<_>>(),
    ];
    serde_json::to_vec(&(game.snapshot(), game.events(), private)).unwrap()
}
