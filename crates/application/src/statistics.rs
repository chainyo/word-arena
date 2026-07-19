use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use word_arena_engine::{
    EventVisibility, GameEvent, GameEventKind, GameResult, Premium, Seat, TerminalReason,
};

use crate::{BoxFuture, UnixMillis};

pub const STATISTICS_SCHEMA_VERSION: u32 = 1;
pub const RATE_SCALE: u64 = 1_000_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatisticAvailability {
    Exact,
    Estimated,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourcedStatistic {
    pub availability: StatisticAvailability,
    pub value: Option<u64>,
}

impl SourcedStatistic {
    #[must_use]
    pub const fn exact(value: u64) -> Self {
        Self {
            availability: StatisticAvailability::Exact,
            value: Some(value),
        }
    }

    #[must_use]
    pub const fn estimated(value: u64) -> Self {
        Self {
            availability: StatisticAvailability::Estimated,
            value: Some(value),
        }
    }

    #[must_use]
    pub const fn unavailable() -> Self {
        Self {
            availability: StatisticAvailability::Unavailable,
            value: None,
        }
    }

    fn validate(&self) -> Result<(), StatisticsError> {
        match (self.availability, self.value) {
            (StatisticAvailability::Exact | StatisticAvailability::Estimated, Some(_))
            | (StatisticAvailability::Unavailable, None) => Ok(()),
            _ => Err(StatisticsError::InvalidInput),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizedRunStatistics {
    pub turn_durations_ms: Option<Vec<u64>>,
    pub tool_calls: SourcedStatistic,
    pub input_tokens: SourcedStatistic,
    pub output_tokens: SourcedStatistic,
    pub cost_microusd: SourcedStatistic,
}

impl NormalizedRunStatistics {
    #[must_use]
    pub const fn unavailable() -> Self {
        Self {
            turn_durations_ms: None,
            tool_calls: SourcedStatistic::unavailable(),
            input_tokens: SourcedStatistic::unavailable(),
            output_tokens: SourcedStatistic::unavailable(),
            cost_microusd: SourcedStatistic::unavailable(),
        }
    }

    fn validate(&self) -> Result<(), StatisticsError> {
        self.tool_calls.validate()?;
        self.input_tokens.validate()?;
        self.output_tokens.validate()?;
        self.cost_microusd.validate()?;
        if self
            .turn_durations_ms
            .as_ref()
            .is_some_and(|durations| durations.len() > 1_000_000)
        {
            return Err(StatisticsError::InvalidInput);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StatisticsParticipant {
    pub entrant_id: String,
    pub agent_manifest_sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MatchStatisticsInput {
    pub schema_version: u32,
    pub source_id: String,
    pub tournament_id: Option<String>,
    pub match_id: String,
    pub game_id: String,
    pub finished_at: UnixMillis,
    pub participants: [StatisticsParticipant; 2],
    pub events: Vec<GameEvent>,
    pub invalid_attempts: [u64; 2],
    pub telemetry: [NormalizedRunStatistics; 2],
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StatisticsScope {
    pub language: String,
    pub ruleset_id: String,
    pub ruleset_sha256: String,
    pub pack_id: String,
    pub pack_version: String,
    pub pack_sha256: String,
    pub agent_manifest_sha256: Option<String>,
    pub tournament_id: Option<String>,
    pub match_id: String,
    pub game_id: String,
    pub entrant_id: String,
    pub seat_number: u8,
    pub finished_at: UnixMillis,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PremiumUse {
    pub double_letter: u64,
    pub triple_letter: u64,
    pub double_word: u64,
    pub triple_word: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StatisticsObservation {
    pub schema_version: u32,
    pub source_id: String,
    pub scope: StatisticsScope,
    pub games: u64,
    pub wins: u64,
    pub losses: u64,
    pub ties: u64,
    pub score_for: i64,
    pub score_against: i64,
    pub spread: i64,
    pub scoring_moves: u64,
    pub move_score: u64,
    pub bingos: u64,
    pub invalid_actions: u64,
    pub passes: u64,
    pub exchanges: u64,
    pub premium_use: PremiumUse,
    pub word_frequencies: BTreeMap<String, u64>,
    pub turn_latency_total_ms: SourcedStatistic,
    pub turn_latency_samples: SourcedStatistic,
    pub tool_calls: SourcedStatistic,
    pub input_tokens: SourcedStatistic,
    pub output_tokens: SourcedStatistic,
    pub cost_microusd: SourcedStatistic,
}

impl MatchStatisticsInput {
    /// Derives the two immutable seat observations from public referee events
    /// and privacy-safe normalized telemetry.
    ///
    /// # Errors
    ///
    /// Rejects incomplete/tampered event histories, unsafe identities, invalid
    /// missing-data states, and arithmetic overflow.
    pub fn derive(&self) -> Result<[StatisticsObservation; 2], StatisticsError> {
        let (ruleset, result) = self.validate()?;
        let mut observations = [
            empty_observation(self, 0, ruleset, result)?,
            empty_observation(self, 1, ruleset, result)?,
        ];
        for event in &self.events {
            match &event.kind {
                GameEventKind::MovePlayed {
                    player,
                    placements,
                    words,
                    bingo_bonus,
                    score,
                    ..
                } => {
                    let index = seat_index(*player);
                    let observation = &mut observations[index];
                    observation.scoring_moves = checked_add(observation.scoring_moves, 1)?;
                    observation.move_score =
                        checked_add(observation.move_score, u64::from(*score))?;
                    if *bingo_bonus > 0 {
                        observation.bingos = checked_add(observation.bingos, 1)?;
                    }
                    for placement in placements {
                        let premium = ruleset
                            .game
                            .board
                            .square(placement.coordinate)
                            .ok_or(StatisticsError::InvalidInput)?
                            .premium;
                        add_premium(&mut observation.premium_use, premium)?;
                    }
                    for word in words {
                        if !valid_word(&word.normalized) {
                            return Err(StatisticsError::InvalidInput);
                        }
                        let count = observation
                            .word_frequencies
                            .entry(word.normalized.clone())
                            .or_default();
                        *count = checked_add(*count, 1)?;
                    }
                }
                GameEventKind::Passed { player, .. } => {
                    let observation = &mut observations[seat_index(*player)];
                    observation.passes = checked_add(observation.passes, 1)?;
                }
                GameEventKind::Exchanged { player, .. } => {
                    let observation = &mut observations[seat_index(*player)];
                    observation.exchanges = checked_add(observation.exchanges, 1)?;
                }
                GameEventKind::Created { .. } | GameEventKind::Resigned { .. } => {}
            }
        }
        for (index, observation) in observations.iter_mut().enumerate() {
            observation.invalid_actions = self.invalid_attempts[index];
            apply_telemetry(observation, &self.telemetry[index])?;
        }
        Ok(observations)
    }

    fn validate(&self) -> Result<(&word_arena_engine::Ruleset, &GameResult), StatisticsError> {
        if self.schema_version != STATISTICS_SCHEMA_VERSION
            || !valid_id(&self.source_id)
            || !valid_id(&self.match_id)
            || !valid_id(&self.game_id)
            || self.finished_at.0 < 0
            || self
                .tournament_id
                .as_ref()
                .is_some_and(|identity| !valid_id(identity))
            || self.participants[0].entrant_id == self.participants[1].entrant_id
        {
            return Err(StatisticsError::InvalidInput);
        }
        for participant in &self.participants {
            if !valid_id(&participant.entrant_id)
                || participant
                    .agent_manifest_sha256
                    .as_ref()
                    .is_some_and(|digest| !valid_sha256(digest))
            {
                return Err(StatisticsError::InvalidInput);
            }
        }
        for telemetry in &self.telemetry {
            telemetry.validate()?;
        }
        let first = self.events.first().ok_or(StatisticsError::InvalidInput)?;
        let GameEventKind::Created {
            game_id, ruleset, ..
        } = &first.kind
        else {
            return Err(StatisticsError::InvalidInput);
        };
        if game_id != &self.game_id || ruleset.lexicon != first.lexicon {
            return Err(StatisticsError::InvalidInput);
        }
        let mut terminal = None;
        for (index, event) in self.events.iter().enumerate() {
            if event.sequence != u64::try_from(index).map_err(|_| StatisticsError::Overflow)?
                || event.visibility != EventVisibility::Public
                || event.lexicon != ruleset.lexicon
            {
                return Err(StatisticsError::InvalidInput);
            }
            if let Some(result) = event_result(&event.kind)
                && (terminal.replace(result).is_some() || index + 1 != self.events.len())
            {
                return Err(StatisticsError::InvalidInput);
            }
            if !event_result_is_consistent(&event.kind) {
                return Err(StatisticsError::InvalidInput);
            }
        }
        let result = terminal.ok_or(StatisticsError::InvalidInput)?;
        let identity = ruleset.identity();
        if result.game_id != self.game_id
            || result.ruleset_id != ruleset.id
            || result.lexicon != ruleset.lexicon
            || result.final_version
                != u64::try_from(self.events.len().saturating_sub(1))
                    .map_err(|_| StatisticsError::Overflow)?
            || identity.ruleset_id != result.ruleset_id
        {
            return Err(StatisticsError::InvalidInput);
        }
        let score_one = result.scores[0].value();
        let score_two = result.scores[1].value();
        let expected_winner = match &result.reason {
            TerminalReason::Resignation {
                resigned: Seat::One,
            } => Some(Seat::Two),
            TerminalReason::Resignation {
                resigned: Seat::Two,
            } => Some(Seat::One),
            TerminalReason::ScorelessTurns | TerminalReason::RackEmptied { .. } => {
                match score_one.cmp(&score_two) {
                    std::cmp::Ordering::Greater => Some(Seat::One),
                    std::cmp::Ordering::Less => Some(Seat::Two),
                    std::cmp::Ordering::Equal => None,
                }
            }
        };
        if result.winner != expected_winner {
            return Err(StatisticsError::InvalidInput);
        }
        Ok((ruleset, result))
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StatisticsFilter {
    pub language: Option<String>,
    pub ruleset_id: Option<String>,
    pub ruleset_sha256: Option<String>,
    pub pack_id: Option<String>,
    pub pack_version: Option<String>,
    pub pack_sha256: Option<String>,
    pub agent_manifest_sha256: Option<String>,
    pub tournament_id: Option<String>,
    pub entrant_id: Option<String>,
    pub seat_number: Option<u8>,
    pub finished_from_ms: Option<i64>,
    pub finished_before_ms: Option<i64>,
}

impl StatisticsFilter {
    fn validate(&self) -> Result<(), StatisticsError> {
        for identity in [
            self.language.as_ref(),
            self.ruleset_id.as_ref(),
            self.pack_id.as_ref(),
            self.pack_version.as_ref(),
            self.tournament_id.as_ref(),
            self.entrant_id.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if !valid_id(identity) {
                return Err(StatisticsError::InvalidInput);
            }
        }
        for digest in [
            self.ruleset_sha256.as_ref(),
            self.pack_sha256.as_ref(),
            self.agent_manifest_sha256.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if !valid_sha256(digest) {
                return Err(StatisticsError::InvalidInput);
            }
        }
        if self
            .seat_number
            .is_some_and(|seat| !(1..=2).contains(&seat))
            || self.finished_from_ms.is_some_and(|value| value < 0)
            || self.finished_before_ms.is_some_and(|value| value < 0)
            || matches!((self.finished_from_ms, self.finished_before_ms), (Some(from), Some(before)) if from >= before)
        {
            return Err(StatisticsError::InvalidInput);
        }
        Ok(())
    }

    #[must_use]
    pub fn matches(&self, scope: &StatisticsScope) -> bool {
        optional_matches(self.language.as_ref(), &scope.language)
            && optional_matches(self.ruleset_id.as_ref(), &scope.ruleset_id)
            && optional_matches(self.ruleset_sha256.as_ref(), &scope.ruleset_sha256)
            && optional_matches(self.pack_id.as_ref(), &scope.pack_id)
            && optional_matches(self.pack_version.as_ref(), &scope.pack_version)
            && optional_matches(self.pack_sha256.as_ref(), &scope.pack_sha256)
            && optional_option_matches(
                self.agent_manifest_sha256.as_ref(),
                scope.agent_manifest_sha256.as_ref(),
            )
            && optional_option_matches(self.tournament_id.as_ref(), scope.tournament_id.as_ref())
            && optional_matches(self.entrant_id.as_ref(), &scope.entrant_id)
            && self
                .seat_number
                .is_none_or(|value| value == scope.seat_number)
            && self
                .finished_from_ms
                .is_none_or(|value| scope.finished_at.0 >= value)
            && self
                .finished_before_ms
                .is_none_or(|value| scope.finished_at.0 < value)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicStatistics {
    pub schema_version: u32,
    pub filter: StatisticsFilter,
    pub games: u64,
    pub wins: u64,
    pub losses: u64,
    pub ties: u64,
    pub win_rate_millionths: Option<u64>,
    pub score_for: i64,
    pub score_against: i64,
    pub spread: i64,
    pub scoring_moves: u64,
    pub average_move_score_milli: Option<u64>,
    pub bingos: u64,
    pub invalid_actions: u64,
    pub passes: u64,
    pub exchanges: u64,
    pub premium_use: PremiumUse,
    pub vocabulary_size: u64,
    pub average_turn_latency_ms: SourcedStatistic,
    pub tool_calls: SourcedStatistic,
    pub input_tokens: SourcedStatistic,
    pub output_tokens: SourcedStatistic,
    pub cost_microusd: SourcedStatistic,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorStatistics {
    pub public: PublicStatistics,
    pub word_frequencies: BTreeMap<String, u64>,
    pub source_ids: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct StatisticsAccumulator {
    filter: StatisticsFilter,
    observations: BTreeMap<String, StatisticsObservation>,
}

impl StatisticsAccumulator {
    /// Creates an incremental aggregate for one validated scope.
    ///
    /// # Errors
    ///
    /// Rejects invalid identities or date windows.
    pub fn new(filter: StatisticsFilter) -> Result<Self, StatisticsError> {
        filter.validate()?;
        Ok(Self {
            filter,
            observations: BTreeMap::new(),
        })
    }

    /// Adds one immutable source, deduplicating byte-equivalent retries.
    ///
    /// # Errors
    ///
    /// Rejects malformed observations or changed reuse of a source identity.
    pub fn add(&mut self, observation: StatisticsObservation) -> Result<(), StatisticsError> {
        validate_observation(&observation)?;
        if !self.filter.matches(&observation.scope) {
            return Ok(());
        }
        match self.observations.get(&observation.source_id) {
            Some(existing) if existing == &observation => Ok(()),
            Some(_) => Err(StatisticsError::Conflict),
            None => {
                self.observations
                    .insert(observation.source_id.clone(), observation);
                Ok(())
            }
        }
    }

    /// Produces the privacy-safe public projection in stable source order.
    ///
    /// # Errors
    ///
    /// Returns on checked arithmetic overflow.
    pub fn public(&self) -> Result<PublicStatistics, StatisticsError> {
        aggregate_values(&self.filter, self.observations.values()).map(|aggregate| aggregate.public)
    }

    /// Produces the authorized operator projection with word-level drill-down.
    ///
    /// # Errors
    ///
    /// Returns on checked arithmetic overflow.
    pub fn operator(&self) -> Result<OperatorStatistics, StatisticsError> {
        aggregate_values(&self.filter, self.observations.values()).map(|aggregate| {
            OperatorStatistics {
                public: aggregate.public,
                word_frequencies: aggregate.words,
                source_ids: self.observations.keys().cloned().collect(),
            }
        })
    }
}

/// Rebuilds one authorized operator aggregate from immutable observations.
///
/// # Errors
///
/// Rejects invalid filters, conflicting duplicate sources, or arithmetic
/// overflow.
pub fn aggregate_statistics(
    filter: StatisticsFilter,
    observations: impl IntoIterator<Item = StatisticsObservation>,
) -> Result<OperatorStatistics, StatisticsError> {
    let mut accumulator = StatisticsAccumulator::new(filter)?;
    for observation in observations {
        accumulator.add(observation)?;
    }
    accumulator.operator()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StatisticsRecordResult {
    Applied([StatisticsObservation; 2]),
    AlreadyApplied([StatisticsObservation; 2]),
}

pub trait StatisticsRepository: std::fmt::Debug + Send + Sync {
    fn record(
        &self,
        source: MatchStatisticsInput,
        now: UnixMillis,
    ) -> BoxFuture<'_, Result<StatisticsRecordResult, StatisticsRepositoryError>>;
    fn rebuild_public(
        &self,
        filter: StatisticsFilter,
    ) -> BoxFuture<'_, Result<PublicStatistics, StatisticsRepositoryError>>;
    fn rebuild_operator(
        &self,
        filter: StatisticsFilter,
    ) -> BoxFuture<'_, Result<OperatorStatistics, StatisticsRepositoryError>>;
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum StatisticsRepositoryError {
    #[error("statistics input is invalid")]
    InvalidInput,
    #[error("statistics source conflicts with immutable history")]
    Conflict,
    #[error("stored statistics history is corrupt")]
    Corrupt,
    #[error("statistics repository is unavailable")]
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum StatisticsError {
    #[error("statistics input is invalid")]
    InvalidInput,
    #[error("statistics arithmetic overflowed")]
    Overflow,
    #[error("statistics source identity conflicts")]
    Conflict,
}

fn empty_observation(
    input: &MatchStatisticsInput,
    index: usize,
    ruleset: &word_arena_engine::Ruleset,
    result: &GameResult,
) -> Result<StatisticsObservation, StatisticsError> {
    let identity = ruleset.identity();
    let own = result.scores[index].value();
    let other = result.scores[1 - index].value();
    let spread = i64::from(own)
        .checked_sub(i64::from(other))
        .ok_or(StatisticsError::Overflow)?;
    let winner = result.winner.map(seat_index);
    Ok(StatisticsObservation {
        schema_version: STATISTICS_SCHEMA_VERSION,
        source_id: format!("{}:seat-{}", input.source_id, index + 1),
        scope: StatisticsScope {
            language: ruleset.language.code().to_owned(),
            ruleset_id: ruleset.id.as_str().to_owned(),
            ruleset_sha256: identity.content_sha256,
            pack_id: ruleset.lexicon.pack_id.clone(),
            pack_version: ruleset.lexicon.pack_version.clone(),
            pack_sha256: ruleset.lexicon.content_sha256.clone(),
            agent_manifest_sha256: input.participants[index].agent_manifest_sha256.clone(),
            tournament_id: input.tournament_id.clone(),
            match_id: input.match_id.clone(),
            game_id: input.game_id.clone(),
            entrant_id: input.participants[index].entrant_id.clone(),
            seat_number: u8::try_from(index + 1).map_err(|_| StatisticsError::Overflow)?,
            finished_at: input.finished_at,
        },
        games: 1,
        wins: u64::from(winner == Some(index)),
        losses: u64::from(winner.is_some_and(|seat| seat != index)),
        ties: u64::from(winner.is_none()),
        score_for: i64::from(own),
        score_against: i64::from(other),
        spread,
        scoring_moves: 0,
        move_score: 0,
        bingos: 0,
        invalid_actions: 0,
        passes: 0,
        exchanges: 0,
        premium_use: PremiumUse::default(),
        word_frequencies: BTreeMap::new(),
        turn_latency_total_ms: SourcedStatistic::unavailable(),
        turn_latency_samples: SourcedStatistic::unavailable(),
        tool_calls: SourcedStatistic::unavailable(),
        input_tokens: SourcedStatistic::unavailable(),
        output_tokens: SourcedStatistic::unavailable(),
        cost_microusd: SourcedStatistic::unavailable(),
    })
}

fn apply_telemetry(
    observation: &mut StatisticsObservation,
    telemetry: &NormalizedRunStatistics,
) -> Result<(), StatisticsError> {
    if let Some(durations) = &telemetry.turn_durations_ms {
        let total = durations
            .iter()
            .try_fold(0_u64, |sum, value| checked_add(sum, *value))?;
        observation.turn_latency_total_ms = SourcedStatistic::exact(total);
        observation.turn_latency_samples = SourcedStatistic::exact(
            u64::try_from(durations.len()).map_err(|_| StatisticsError::Overflow)?,
        );
    }
    observation.tool_calls = telemetry.tool_calls.clone();
    observation.input_tokens = telemetry.input_tokens.clone();
    observation.output_tokens = telemetry.output_tokens.clone();
    observation.cost_microusd = telemetry.cost_microusd.clone();
    Ok(())
}

fn validate_observation(observation: &StatisticsObservation) -> Result<(), StatisticsError> {
    if observation.schema_version != STATISTICS_SCHEMA_VERSION
        || !valid_id(&observation.source_id)
        || observation.games != 1
        || observation
            .wins
            .checked_add(observation.losses)
            .and_then(|value| value.checked_add(observation.ties))
            != Some(1)
        || !(1..=2).contains(&observation.scope.seat_number)
        || !valid_scope(&observation.scope)
        || observation
            .word_frequencies
            .iter()
            .any(|(word, count)| !valid_word(word) || *count == 0)
    {
        return Err(StatisticsError::InvalidInput);
    }
    for metric in [
        &observation.turn_latency_total_ms,
        &observation.turn_latency_samples,
        &observation.tool_calls,
        &observation.input_tokens,
        &observation.output_tokens,
        &observation.cost_microusd,
    ] {
        metric.validate()?;
    }
    if observation.turn_latency_total_ms.availability
        != observation.turn_latency_samples.availability
    {
        return Err(StatisticsError::InvalidInput);
    }
    Ok(())
}

struct AggregateValues {
    public: PublicStatistics,
    words: BTreeMap<String, u64>,
}

fn aggregate_values<'a>(
    filter: &StatisticsFilter,
    observations: impl Iterator<Item = &'a StatisticsObservation>,
) -> Result<AggregateValues, StatisticsError> {
    let mut games = 0_u64;
    let mut wins = 0_u64;
    let mut losses = 0_u64;
    let mut ties = 0_u64;
    let mut score_for = 0_i64;
    let mut score_against = 0_i64;
    let mut spread = 0_i64;
    let mut scoring_moves = 0_u64;
    let mut move_score = 0_u64;
    let mut bingos = 0_u64;
    let mut invalid_actions = 0_u64;
    let mut passes = 0_u64;
    let mut exchanges = 0_u64;
    let mut premium_use = PremiumUse::default();
    let mut words = BTreeMap::new();
    let mut latency_total = Vec::new();
    let mut latency_samples = Vec::new();
    let mut tools = Vec::new();
    let mut input_tokens = Vec::new();
    let mut output_tokens = Vec::new();
    let mut costs = Vec::new();
    for observation in observations {
        games = checked_add(games, observation.games)?;
        wins = checked_add(wins, observation.wins)?;
        losses = checked_add(losses, observation.losses)?;
        ties = checked_add(ties, observation.ties)?;
        score_for = score_for
            .checked_add(observation.score_for)
            .ok_or(StatisticsError::Overflow)?;
        score_against = score_against
            .checked_add(observation.score_against)
            .ok_or(StatisticsError::Overflow)?;
        spread = spread
            .checked_add(observation.spread)
            .ok_or(StatisticsError::Overflow)?;
        scoring_moves = checked_add(scoring_moves, observation.scoring_moves)?;
        move_score = checked_add(move_score, observation.move_score)?;
        bingos = checked_add(bingos, observation.bingos)?;
        invalid_actions = checked_add(invalid_actions, observation.invalid_actions)?;
        passes = checked_add(passes, observation.passes)?;
        exchanges = checked_add(exchanges, observation.exchanges)?;
        add_premium_counts(&mut premium_use, observation.premium_use)?;
        for (word, count) in &observation.word_frequencies {
            let current = words.entry(word.clone()).or_default();
            *current = checked_add(*current, *count)?;
        }
        latency_total.push(observation.turn_latency_total_ms.clone());
        latency_samples.push(observation.turn_latency_samples.clone());
        tools.push(observation.tool_calls.clone());
        input_tokens.push(observation.input_tokens.clone());
        output_tokens.push(observation.output_tokens.clone());
        costs.push(observation.cost_microusd.clone());
    }
    let latency_total = combine_metrics(&latency_total)?;
    let latency_samples = combine_metrics(&latency_samples)?;
    let average_turn_latency_ms = average_metric(&latency_total, &latency_samples)?;
    Ok(AggregateValues {
        public: PublicStatistics {
            schema_version: STATISTICS_SCHEMA_VERSION,
            filter: filter.clone(),
            games,
            wins,
            losses,
            ties,
            win_rate_millionths: ratio(wins, games, RATE_SCALE)?,
            score_for,
            score_against,
            spread,
            scoring_moves,
            average_move_score_milli: ratio(move_score, scoring_moves, 1_000)?,
            bingos,
            invalid_actions,
            passes,
            exchanges,
            premium_use,
            vocabulary_size: u64::try_from(words.len()).map_err(|_| StatisticsError::Overflow)?,
            average_turn_latency_ms,
            tool_calls: combine_metrics(&tools)?,
            input_tokens: combine_metrics(&input_tokens)?,
            output_tokens: combine_metrics(&output_tokens)?,
            cost_microusd: combine_metrics(&costs)?,
        },
        words,
    })
}

fn combine_metrics(metrics: &[SourcedStatistic]) -> Result<SourcedStatistic, StatisticsError> {
    if metrics.is_empty()
        || metrics
            .iter()
            .any(|metric| metric.availability == StatisticAvailability::Unavailable)
    {
        return Ok(SourcedStatistic::unavailable());
    }
    let value = metrics.iter().try_fold(0_u64, |sum, metric| {
        checked_add(sum, metric.value.ok_or(StatisticsError::InvalidInput)?)
    })?;
    Ok(
        if metrics
            .iter()
            .any(|metric| metric.availability == StatisticAvailability::Estimated)
        {
            SourcedStatistic::estimated(value)
        } else {
            SourcedStatistic::exact(value)
        },
    )
}

fn average_metric(
    total: &SourcedStatistic,
    samples: &SourcedStatistic,
) -> Result<SourcedStatistic, StatisticsError> {
    if total.availability == StatisticAvailability::Unavailable
        || samples.availability == StatisticAvailability::Unavailable
        || samples.value == Some(0)
    {
        return Ok(SourcedStatistic::unavailable());
    }
    let value = ratio(
        total.value.ok_or(StatisticsError::InvalidInput)?,
        samples.value.ok_or(StatisticsError::InvalidInput)?,
        1,
    )?
    .ok_or(StatisticsError::InvalidInput)?;
    Ok(
        if total.availability == StatisticAvailability::Estimated
            || samples.availability == StatisticAvailability::Estimated
        {
            SourcedStatistic::estimated(value)
        } else {
            SourcedStatistic::exact(value)
        },
    )
}

fn ratio(numerator: u64, denominator: u64, scale: u64) -> Result<Option<u64>, StatisticsError> {
    if denominator == 0 {
        return Ok(None);
    }
    let rounded = u128::from(numerator)
        .checked_mul(u128::from(scale))
        .and_then(|value| value.checked_add(u128::from(denominator / 2)))
        .ok_or(StatisticsError::Overflow)?
        / u128::from(denominator);
    Ok(Some(
        u64::try_from(rounded).map_err(|_| StatisticsError::Overflow)?,
    ))
}

fn add_premium(use_counts: &mut PremiumUse, premium: Premium) -> Result<(), StatisticsError> {
    let target = match premium {
        Premium::Normal => return Ok(()),
        Premium::DoubleLetter => &mut use_counts.double_letter,
        Premium::TripleLetter => &mut use_counts.triple_letter,
        Premium::DoubleWord => &mut use_counts.double_word,
        Premium::TripleWord => &mut use_counts.triple_word,
    };
    *target = checked_add(*target, 1)?;
    Ok(())
}

fn add_premium_counts(target: &mut PremiumUse, value: PremiumUse) -> Result<(), StatisticsError> {
    target.double_letter = checked_add(target.double_letter, value.double_letter)?;
    target.triple_letter = checked_add(target.triple_letter, value.triple_letter)?;
    target.double_word = checked_add(target.double_word, value.double_word)?;
    target.triple_word = checked_add(target.triple_word, value.triple_word)?;
    Ok(())
}

fn event_result(kind: &GameEventKind) -> Option<&GameResult> {
    match kind {
        GameEventKind::MovePlayed { result, .. }
        | GameEventKind::Passed { result, .. }
        | GameEventKind::Exchanged { result, .. } => result.as_ref(),
        GameEventKind::Resigned { result, .. } => Some(result),
        GameEventKind::Created { .. } => None,
    }
}

fn event_result_is_consistent(kind: &GameEventKind) -> bool {
    match kind {
        GameEventKind::Created { .. } => true,
        GameEventKind::Resigned { player, result } => {
            result.reason == (TerminalReason::Resignation { resigned: *player })
        }
        GameEventKind::Passed { result, .. } | GameEventKind::Exchanged { result, .. } => result
            .as_ref()
            .is_none_or(|result| result.reason == TerminalReason::ScorelessTurns),
        GameEventKind::MovePlayed { result, .. } => result.as_ref().is_none_or(|result| {
            matches!(
                result.reason,
                TerminalReason::ScorelessTurns | TerminalReason::RackEmptied { .. }
            )
        }),
    }
}

const fn seat_index(seat: Seat) -> usize {
    match seat {
        Seat::One => 0,
        Seat::Two => 1,
    }
}

fn checked_add(left: u64, right: u64) -> Result<u64, StatisticsError> {
    left.checked_add(right).ok_or(StatisticsError::Overflow)
}

fn valid_scope(scope: &StatisticsScope) -> bool {
    valid_id(&scope.language)
        && valid_id(&scope.ruleset_id)
        && valid_sha256(&scope.ruleset_sha256)
        && valid_id(&scope.pack_id)
        && valid_id(&scope.pack_version)
        && valid_sha256(&scope.pack_sha256)
        && scope
            .agent_manifest_sha256
            .as_ref()
            .is_none_or(|digest| valid_sha256(digest))
        && scope
            .tournament_id
            .as_ref()
            .is_none_or(|identity| valid_id(identity))
        && valid_id(&scope.match_id)
        && valid_id(&scope.game_id)
        && valid_id(&scope.entrant_id)
        && scope.finished_at.0 >= 0
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

fn valid_word(value: &str) -> bool {
    valid_id(value)
        && value
            .chars()
            .all(|character| character.is_ascii_uppercase())
}

fn optional_matches(expected: Option<&String>, actual: &str) -> bool {
    expected.is_none_or(|value| value == actual)
}

fn optional_option_matches(expected: Option<&String>, actual: Option<&String>) -> bool {
    expected.is_none_or(|value| actual == Some(value))
}
