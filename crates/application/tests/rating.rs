use word_arena_application::{
    RATING_SCHEMA_VERSION, RatedMatchInput, RatingOpponent, RatingPeriod, RatingPool,
    RatingUpdateInput, RatingValue, SCORE_SCALE, update_rating,
};

#[test]
fn published_glicko_two_example_matches_reference() {
    let current = RatingValue::from_f64(1_500.0, 200.0, 0.06).unwrap();
    let opponents = [
        opponent(1_400.0, 30.0, SCORE_SCALE),
        opponent(1_550.0, 100.0, 0),
        opponent(1_700.0, 300.0, 0),
    ];
    let updated = update_rating(current, &opponents).unwrap();
    assert!((updated.rating() - 1_464.06).abs() < 0.01);
    assert!((updated.deviation() - 151.52).abs() < 0.01);
    assert!((updated.volatility() - 0.059_996).abs() < 0.000_001);
}

#[test]
fn inactivity_increases_deviation_without_rating_drift() {
    let current = RatingValue::from_f64(1_600.0, 50.0, 0.06).unwrap();
    let updated = update_rating(current, &[]).unwrap();
    assert_eq!(updated.rating_milli, current.rating_milli);
    assert!(updated.deviation_milli > current.deviation_milli);
    assert_eq!(updated.volatility_nano, current.volatility_nano);
}

#[test]
fn inactivity_respects_the_published_deviation_ceiling() {
    let current = RatingValue::from_f64(1_500.0, 350.0, 0.06).unwrap();
    let updated = update_rating(current, &[]).unwrap();
    assert_eq!(updated.rating_milli, current.rating_milli);
    assert_eq!(updated.deviation_milli, 350_000);
}

#[test]
fn fixed_point_serialization_is_byte_deterministic() {
    let rating = RatingValue::from_f64(1_464.06, 151.52, 0.059_996).unwrap();
    let first = serde_json::to_vec(&rating).unwrap();
    let second = serde_json::to_vec(&rating).unwrap();
    assert_eq!(first, second);
    assert!(!String::from_utf8(first).unwrap().contains('.'));
}

#[test]
fn symmetric_tie_preserves_rating_and_reduces_uncertainty() {
    let current = RatingValue::from_f64(1_500.0, 200.0, 0.06).unwrap();
    let updated = update_rating(current, &[opponent(1_500.0, 200.0, SCORE_SCALE / 2)]).unwrap();
    assert_eq!(updated.rating_milli, current.rating_milli);
    assert!(updated.deviation_milli < current.deviation_milli);
    assert!(updated.volatility_nano > 0);
}

#[test]
fn period_rejects_hidden_or_duplicate_match_inputs() {
    let previous = RatingValue::from_f64(1_500.0, 200.0, 0.06).unwrap();
    let game = RatedMatchInput {
        match_id: "match-1".to_owned(),
        series_id: "series-1".to_owned(),
        series_game_number: 1,
        entrant_one: "alpha".to_owned(),
        entrant_two: "beta".to_owned(),
        score_one_millionths: SCORE_SCALE,
    };
    let mut period = RatingPeriod {
        schema_version: RATING_SCHEMA_VERSION,
        period_id: "period-1".to_owned(),
        sequence: 0,
        pool: RatingPool {
            language: "en".to_owned(),
            ruleset_id: "classic".to_owned(),
            ruleset_sha256: "a".repeat(64),
            rated_format_policy: "single-game-v1".to_owned(),
        },
        matches: vec![game.clone()],
        updates: vec![
            RatingUpdateInput {
                entrant_id: "alpha".to_owned(),
                previous,
                opponents: vec![RatingOpponent {
                    rating: previous,
                    score_millionths: SCORE_SCALE,
                }],
            },
            RatingUpdateInput {
                entrant_id: "beta".to_owned(),
                previous,
                opponents: vec![RatingOpponent {
                    rating: previous,
                    score_millionths: 0,
                }],
            },
        ],
    };
    assert!(period.validate().is_ok());
    period.matches.push(game);
    assert!(period.validate().is_err());
    period.matches.pop();
    period.updates[0].opponents.clear();
    assert!(period.validate().is_err());
}

fn opponent(rating: f64, deviation: f64, score_millionths: u32) -> RatingOpponent {
    RatingOpponent {
        rating: RatingValue::from_f64(rating, deviation, 0.06).unwrap(),
        score_millionths,
    }
}
