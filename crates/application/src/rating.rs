use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{BoxFuture, UnixMillis};

pub const RATING_SCHEMA_VERSION: u32 = 1;
pub const SCORE_SCALE: u32 = 1_000_000;
const GLICKO_SCALE: f64 = 173.7178;
const DEFAULT_TAU: f64 = 0.5;
const EPSILON: f64 = 0.000_001;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RatingValue {
    pub rating_milli: i32,
    pub deviation_milli: u32,
    pub volatility_nano: u32,
}

impl RatingValue {
    /// Creates a fixed-point rating from conventional Glicko-2 values.
    ///
    /// # Errors
    ///
    /// Rejects non-finite or unsafe numeric bounds.
    pub fn from_f64(rating: f64, deviation: f64, volatility: f64) -> Result<Self, RatingError> {
        if !rating.is_finite()
            || !deviation.is_finite()
            || !volatility.is_finite()
            || !(-1_000.0..=5_000.0).contains(&rating)
            || !(0.001..=350.0).contains(&deviation)
            || !(0.000_001..=1.0).contains(&volatility)
        {
            return Err(RatingError::InvalidInput);
        }
        Ok(Self {
            rating_milli: rounded_integer(rating * 1_000.0)?,
            deviation_milli: rounded_unsigned(deviation * 1_000.0)?,
            volatility_nano: rounded_unsigned(volatility * 1_000_000_000.0)?,
        })
    }

    #[must_use]
    pub fn rating(self) -> f64 {
        f64::from(self.rating_milli) / 1_000.0
    }

    #[must_use]
    pub fn deviation(self) -> f64 {
        f64::from(self.deviation_milli) / 1_000.0
    }

    #[must_use]
    pub fn volatility(self) -> f64 {
        f64::from(self.volatility_nano) / 1_000_000_000.0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RatingOpponent {
    pub rating: RatingValue,
    pub score_millionths: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RatingPool {
    pub language: String,
    pub ruleset_id: String,
    pub ruleset_sha256: String,
    pub rated_format_policy: String,
}

impl RatingPool {
    /// Returns the SHA-256 identity of language, ruleset, and rated-format policy.
    ///
    /// # Errors
    ///
    /// Rejects malformed pool identity or serialization failure.
    pub fn key(&self) -> Result<String, RatingError> {
        if !valid_id(&self.language)
            || !valid_id(&self.ruleset_id)
            || !valid_sha256(&self.ruleset_sha256)
            || !valid_id(&self.rated_format_policy)
        {
            return Err(RatingError::InvalidInput);
        }
        let bytes = serde_json::to_vec(self).map_err(|_| RatingError::InvalidInput)?;
        Ok(Sha256::digest(bytes)
            .iter()
            .fold(String::with_capacity(64), |mut output, byte| {
                use std::fmt::Write;
                write!(&mut output, "{byte:02x}").expect("writing a digest to String cannot fail");
                output
            }))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RatedMatchInput {
    pub match_id: String,
    pub series_id: String,
    pub series_game_number: u16,
    pub entrant_one: String,
    pub entrant_two: String,
    pub score_one_millionths: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RatingUpdateInput {
    pub entrant_id: String,
    pub previous: RatingValue,
    pub opponents: Vec<RatingOpponent>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RatingPeriod {
    pub schema_version: u32,
    pub period_id: String,
    pub sequence: u64,
    pub pool: RatingPool,
    pub matches: Vec<RatedMatchInput>,
    pub updates: Vec<RatingUpdateInput>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StoredRatingPeriod {
    pub period: RatingPeriod,
    pub derived: Vec<(String, RatingValue)>,
    pub committed_at: UnixMillis,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RatingCommitResult {
    Applied(StoredRatingPeriod),
    AlreadyApplied(StoredRatingPeriod),
}

pub trait RatingRepository: std::fmt::Debug + Send + Sync {
    fn commit(
        &self,
        period: RatingPeriod,
        now: UnixMillis,
    ) -> BoxFuture<'_, Result<RatingCommitResult, RatingRepositoryError>>;
    fn load<'a>(
        &'a self,
        period_id: &'a str,
    ) -> BoxFuture<'a, Result<StoredRatingPeriod, RatingRepositoryError>>;
    fn rebuild(
        &self,
        pool: RatingPool,
    ) -> BoxFuture<'_, Result<Vec<(String, RatingValue)>, RatingRepositoryError>>;
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RatingRepositoryError {
    #[error("rating period was not found")]
    NotFound,
    #[error("rating period conflicts with immutable history")]
    Conflict,
    #[error("stored rating history is corrupt")]
    Corrupt,
    #[error("rating repository is unavailable")]
    Unavailable,
}

impl RatingPeriod {
    /// Validates immutable period inputs, unique games, and paired-game identity.
    ///
    /// # Errors
    ///
    /// Rejects malformed identities, duplicate matches/entrants, and invalid scores.
    pub fn validate(&self) -> Result<(), RatingError> {
        if self.schema_version != RATING_SCHEMA_VERSION
            || !valid_id(&self.period_id)
            || !valid_id(&self.pool.language)
            || !valid_id(&self.pool.ruleset_id)
            || !valid_sha256(&self.pool.ruleset_sha256)
            || !valid_id(&self.pool.rated_format_policy)
            || self.updates.is_empty()
        {
            return Err(RatingError::InvalidInput);
        }
        let mut match_ids = std::collections::BTreeSet::new();
        for game in &self.matches {
            if !valid_id(&game.match_id)
                || !valid_id(&game.series_id)
                || game.series_game_number == 0
                || !valid_id(&game.entrant_one)
                || !valid_id(&game.entrant_two)
                || game.entrant_one == game.entrant_two
                || game.score_one_millionths > SCORE_SCALE
                || !match_ids.insert(&game.match_id)
            {
                return Err(RatingError::InvalidInput);
            }
        }
        let mut entrants = std::collections::BTreeSet::new();
        for update in &self.updates {
            validate_rating(update.previous)?;
            if !valid_id(&update.entrant_id)
                || !entrants.insert(&update.entrant_id)
                || update.opponents.iter().any(|opponent| {
                    opponent.score_millionths > SCORE_SCALE
                        || validate_rating(opponent.rating).is_err()
                })
            {
                return Err(RatingError::InvalidInput);
            }
        }
        let previous = self
            .updates
            .iter()
            .map(|update| (update.entrant_id.as_str(), update.previous))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut expected = std::collections::BTreeMap::<&str, Vec<RatingOpponent>>::new();
        for game in &self.matches {
            let one = *previous
                .get(game.entrant_one.as_str())
                .ok_or(RatingError::InvalidInput)?;
            let two = *previous
                .get(game.entrant_two.as_str())
                .ok_or(RatingError::InvalidInput)?;
            expected
                .entry(&game.entrant_one)
                .or_default()
                .push(RatingOpponent {
                    rating: two,
                    score_millionths: game.score_one_millionths,
                });
            expected
                .entry(&game.entrant_two)
                .or_default()
                .push(RatingOpponent {
                    rating: one,
                    score_millionths: SCORE_SCALE - game.score_one_millionths,
                });
        }
        for update in &self.updates {
            let mut actual = update.opponents.clone();
            actual.sort();
            let mut required = expected
                .remove(update.entrant_id.as_str())
                .unwrap_or_default();
            required.sort();
            if actual != required {
                return Err(RatingError::InvalidInput);
            }
        }
        Ok(())
    }

    /// Recomputes every derived fixed-point rating in stable entrant order.
    ///
    /// # Errors
    ///
    /// Returns when the period or a numerical update is invalid.
    pub fn derive(&self) -> Result<Vec<(String, RatingValue)>, RatingError> {
        self.validate()?;
        let mut updates = self.updates.clone();
        updates.sort_by(|left, right| left.entrant_id.cmp(&right.entrant_id));
        updates
            .into_iter()
            .map(|input| {
                let next = update_rating(input.previous, &input.opponents)?;
                Ok((input.entrant_id, next))
            })
            .collect()
    }
}

/// Applies one Glicko-2 rating period and returns deterministic fixed-point output.
///
/// # Errors
///
/// Rejects invalid ratings/scores or numerical non-convergence.
pub fn update_rating(
    current: RatingValue,
    opponents: &[RatingOpponent],
) -> Result<RatingValue, RatingError> {
    validate_rating(current)?;
    if opponents.iter().any(|opponent| {
        opponent.score_millionths > SCORE_SCALE || validate_rating(opponent.rating).is_err()
    }) {
        return Err(RatingError::InvalidInput);
    }
    let mu = (current.rating() - 1_500.0) / GLICKO_SCALE;
    let phi = current.deviation() / GLICKO_SCALE;
    let sigma = current.volatility();
    if opponents.is_empty() {
        return fixed(mu, (phi.mul_add(phi, sigma * sigma)).sqrt(), sigma);
    }
    let mut variance_inverse = 0.0;
    let mut improvement = 0.0;
    for opponent in opponents {
        let opponent_mu = (opponent.rating.rating() - 1_500.0) / GLICKO_SCALE;
        let opponent_phi = opponent.rating.deviation() / GLICKO_SCALE;
        let impact =
            1.0 / (1.0 + 3.0 * opponent_phi * opponent_phi / std::f64::consts::PI.powi(2)).sqrt();
        let expected = 1.0 / (1.0 + (-impact * (mu - opponent_mu)).exp());
        variance_inverse += impact * impact * expected * (1.0 - expected);
        improvement +=
            impact * (f64::from(opponent.score_millionths) / f64::from(SCORE_SCALE) - expected);
    }
    if variance_inverse <= 0.0 || !variance_inverse.is_finite() {
        return Err(RatingError::NumericalFailure);
    }
    let variance = 1.0 / variance_inverse;
    let delta = variance * improvement;
    let next_sigma = solve_volatility(phi, sigma, variance, delta)?;
    let pre_deviation = (phi.mul_add(phi, next_sigma * next_sigma)).sqrt();
    let next_phi = 1.0 / (1.0 / (pre_deviation * pre_deviation) + 1.0 / variance).sqrt();
    let next_mu = mu + next_phi * next_phi * improvement;
    fixed(next_mu, next_phi, next_sigma)
}

fn solve_volatility(phi: f64, sigma: f64, variance: f64, delta: f64) -> Result<f64, RatingError> {
    let alpha = (sigma * sigma).ln();
    let function = |value: f64| {
        let exponential = value.exp();
        exponential * (delta * delta - phi * phi - variance - exponential)
            / (2.0 * (phi * phi + variance + exponential).powi(2))
            - (value - alpha) / (DEFAULT_TAU * DEFAULT_TAU)
    };
    let mut lower = alpha;
    let mut upper = if delta * delta > phi * phi + variance {
        (delta * delta - phi * phi - variance).ln()
    } else {
        let mut multiplier = 1.0;
        while function(alpha - multiplier * DEFAULT_TAU) < 0.0 {
            multiplier += 1.0;
            if multiplier > 10_000.0 {
                return Err(RatingError::NumericalFailure);
            }
        }
        alpha - multiplier * DEFAULT_TAU
    };
    let mut lower_value = function(lower);
    let mut upper_value = function(upper);
    for _ in 0..10_000 {
        if (upper - lower).abs() <= EPSILON {
            return Ok((lower / 2.0).exp());
        }
        let candidate = lower + (lower - upper) * lower_value / (upper_value - lower_value);
        let candidate_value = function(candidate);
        if candidate_value * upper_value <= 0.0 {
            lower = upper;
            lower_value = upper_value;
        } else {
            lower_value /= 2.0;
        }
        upper = candidate;
        upper_value = candidate_value;
    }
    Err(RatingError::NumericalFailure)
}

fn fixed(mu: f64, phi: f64, sigma: f64) -> Result<RatingValue, RatingError> {
    RatingValue::from_f64(
        mu.mul_add(GLICKO_SCALE, 1_500.0).clamp(-1_000.0, 5_000.0),
        (phi * GLICKO_SCALE).clamp(0.001, 350.0),
        sigma.clamp(0.000_001, 1.0),
    )
}

fn validate_rating(value: RatingValue) -> Result<(), RatingError> {
    RatingValue::from_f64(value.rating(), value.deviation(), value.volatility()).map(|_| ())
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && value.chars().all(|character| !character.is_control())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn rounded_integer(value: f64) -> Result<i32, RatingError> {
    format!("{value:.0}")
        .parse()
        .map_err(|_| RatingError::NumericalFailure)
}

fn rounded_unsigned(value: f64) -> Result<u32, RatingError> {
    format!("{value:.0}")
        .parse()
        .map_err(|_| RatingError::NumericalFailure)
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RatingError {
    #[error("rating input is invalid")]
    InvalidInput,
    #[error("Glicko-2 iteration did not converge")]
    NumericalFailure,
}
