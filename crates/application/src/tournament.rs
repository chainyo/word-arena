use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use word_arena_lexicon::PackIdentity;

use crate::{BoxFuture, RepositoryError, UnixMillis};

pub const TOURNAMENT_FORMAT_SCHEMA_VERSION: u32 = 1;
pub const TOURNAMENT_SCHEDULE_SCHEMA_VERSION: u32 = 1;
pub const TOURNAMENT_LIFECYCLE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TournamentEntrant {
    pub entrant_id: String,
    pub seed_number: u32,
    pub manifest_sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TournamentGameProfile {
    pub language: String,
    pub ruleset_id: String,
    pub ruleset_sha256: String,
    pub lexicon: PackIdentity,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SeriesSeatPolicy {
    Alternate,
    PairedSwap,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SwissRematchPolicy {
    Avoid,
    AllowWhenRequired,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TournamentFormat {
    RoundRobin {
        cycles: u16,
    },
    PairedSeatSwap {
        cycles: u16,
    },
    Swiss {
        rounds: u16,
        games_per_series: u16,
        rematches: SwissRematchPolicy,
    },
    Series {
        cycles: u16,
        games_per_series: u16,
        seat_policy: SeriesSeatPolicy,
    },
}

impl TournamentFormat {
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::RoundRobin { .. } => "round_robin",
            Self::PairedSeatSwap { .. } => "paired_seat_swap",
            Self::Swiss { .. } => "swiss",
            Self::Series { .. } => "series",
        }
    }

    const fn games_per_series(&self) -> u16 {
        match self {
            Self::RoundRobin { .. } => 1,
            Self::PairedSeatSwap { .. } => 2,
            Self::Swiss {
                games_per_series, ..
            }
            | Self::Series {
                games_per_series, ..
            } => *games_per_series,
        }
    }

    const fn static_cycles(&self) -> Option<u16> {
        match self {
            Self::RoundRobin { cycles }
            | Self::PairedSeatSwap { cycles }
            | Self::Series { cycles, .. } => Some(*cycles),
            Self::Swiss { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TournamentSpec {
    pub schema_version: u32,
    pub tournament_id: String,
    pub format: TournamentFormat,
    pub entrants: Vec<TournamentEntrant>,
    pub profiles: Vec<TournamentGameProfile>,
    pub game_seed_commitments: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TournamentFormatIdentity {
    pub schema_version: u32,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduledSeries {
    pub series_id: String,
    pub round_number: u16,
    pub table_number: u16,
    pub entrant_a: String,
    pub entrant_b: String,
    pub match_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduledMatch {
    pub match_id: String,
    pub series_id: String,
    pub sequence: u64,
    pub round_number: u16,
    pub table_number: u16,
    pub series_game_number: u16,
    pub seat_one_entrant_id: String,
    pub seat_two_entrant_id: String,
    pub profile: TournamentGameProfile,
    pub game_seed_commitment_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TournamentBye {
    pub round_number: u16,
    pub entrant_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TournamentSchedule {
    pub schema_version: u32,
    pub tournament_id: String,
    pub format_identity: TournamentFormatIdentity,
    pub series: Vec<ScheduledSeries>,
    pub matches: Vec<ScheduledMatch>,
    pub byes: Vec<TournamentBye>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EntrantPairing {
    pub entrant_a: String,
    pub entrant_b: String,
}

impl EntrantPairing {
    fn new(first: &str, second: &str) -> Self {
        if first <= second {
            Self {
                entrant_a: first.to_owned(),
                entrant_b: second.to_owned(),
            }
        } else {
            Self {
                entrant_a: second.to_owned(),
                entrant_b: first.to_owned(),
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SwissStanding {
    pub entrant_id: String,
    pub match_points: i64,
    pub spread: i64,
    pub wins: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeatBalance {
    pub entrant_id: String,
    pub seat_one_games: u32,
    pub seat_two_games: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SwissProgress {
    pub completed_rounds: u16,
    pub standings: Vec<SwissStanding>,
    pub prior_pairings: BTreeSet<EntrantPairing>,
    pub prior_byes: BTreeSet<String>,
    pub seat_balance: Vec<SeatBalance>,
    pub next_seed_index: usize,
    pub next_match_sequence: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TournamentLifecycleState {
    Draft,
    Scheduled,
    Running,
    Paused,
    Finished,
    Cancelled,
}

impl TournamentLifecycleState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Scheduled => "scheduled",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Finished => "finished",
            Self::Cancelled => "cancelled",
        }
    }

    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Draft, Self::Scheduled | Self::Cancelled)
                | (
                    Self::Scheduled | Self::Paused,
                    Self::Running | Self::Cancelled
                )
                | (
                    Self::Running,
                    Self::Paused | Self::Finished | Self::Cancelled
                )
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TournamentLifecycleEvent {
    pub schema_version: u32,
    pub sequence: u64,
    pub state: TournamentLifecycleState,
    pub occurred_at: UnixMillis,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StoredTournament {
    pub spec: TournamentSpec,
    pub schedule: TournamentSchedule,
    pub swiss_progress: Option<SwissProgress>,
    pub lifecycle: Vec<TournamentLifecycleEvent>,
}

impl StoredTournament {
    /// Revalidates the complete immutable schedule and ordered lifecycle.
    ///
    /// # Errors
    ///
    /// Rejects format/schedule/progress drift or invalid lifecycle transitions.
    pub fn validate(&self) -> Result<(), TournamentError> {
        self.spec.validate()?;
        let expected = match (&self.spec.format, &self.swiss_progress) {
            (TournamentFormat::Swiss { .. }, Some(progress)) => {
                self.spec.schedule_swiss_round(progress)?
            }
            (TournamentFormat::Swiss { .. }, None) => {
                return Err(TournamentError::InvalidSwissProgress);
            }
            (_, None) => self.spec.schedule()?,
            (_, Some(_)) => return Err(TournamentError::InvalidSwissProgress),
        };
        if self.schedule != expected || self.lifecycle.is_empty() {
            return Err(TournamentError::InvalidSpec);
        }
        for (sequence, event) in self.lifecycle.iter().enumerate() {
            if event.schema_version != TOURNAMENT_LIFECYCLE_SCHEMA_VERSION
                || event.sequence != sequence as u64
                || event.occurred_at.0 < 0
                || sequence > 0
                    && (!self.lifecycle[sequence - 1]
                        .state
                        .can_transition_to(event.state)
                        || event.occurred_at < self.lifecycle[sequence - 1].occurred_at)
            {
                return Err(TournamentError::InvalidSpec);
            }
        }
        if self.lifecycle[0].state != TournamentLifecycleState::Scheduled {
            return Err(TournamentError::InvalidSpec);
        }
        Ok(())
    }
}

pub trait TournamentRepository: std::fmt::Debug + Send + Sync {
    fn insert(&self, tournament: StoredTournament) -> BoxFuture<'_, Result<(), RepositoryError>>;

    fn load<'a>(
        &'a self,
        tournament_id: &'a str,
    ) -> BoxFuture<'a, Result<StoredTournament, RepositoryError>>;

    fn transition<'a>(
        &'a self,
        tournament_id: &'a str,
        expected_sequence: u64,
        state: TournamentLifecycleState,
        occurred_at: UnixMillis,
    ) -> BoxFuture<'a, Result<TournamentLifecycleEvent, RepositoryError>>;
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum TournamentError {
    #[error("tournament specification is invalid")]
    InvalidSpec,
    #[error("tournament format is progressive and requires Swiss state")]
    SwissProgressRequired,
    #[error("Swiss state is invalid or complete")]
    InvalidSwissProgress,
    #[error("Swiss rematches are unavoidable under the configured policy")]
    SwissPairingUnavailable,
    #[error("not enough exact game seed commitments were supplied")]
    MissingGameSeeds,
    #[error("unused game seed commitments make the schedule ambiguous")]
    ExtraGameSeeds,
    #[error("tournament schedule arithmetic overflowed")]
    Overflow,
    #[error("tournament serialization failed")]
    Serialization,
}

#[derive(Serialize)]
struct FormatIdentityInput<'a> {
    schema_version: u32,
    format: &'a TournamentFormat,
    profiles: &'a [TournamentGameProfile],
}

impl TournamentSpec {
    /// Returns the immutable SHA-256 identity of format and game-profile policy.
    ///
    /// # Errors
    ///
    /// Rejects invalid specifications or serialization failure.
    pub fn format_identity(&self) -> Result<TournamentFormatIdentity, TournamentError> {
        self.validate()?;
        let bytes = serde_json::to_vec(&FormatIdentityInput {
            schema_version: TOURNAMENT_FORMAT_SCHEMA_VERSION,
            format: &self.format,
            profiles: &self.profiles,
        })
        .map_err(|_| TournamentError::Serialization)?;
        let digest = Sha256::digest(bytes);
        let mut sha256 = String::with_capacity(64);
        for byte in digest {
            write!(&mut sha256, "{byte:02x}").expect("writing a digest to String cannot fail");
        }
        Ok(TournamentFormatIdentity {
            schema_version: TOURNAMENT_FORMAT_SCHEMA_VERSION,
            sha256,
        })
    }

    /// Generates a complete static round-robin, paired, or series schedule.
    ///
    /// # Errors
    ///
    /// Rejects invalid inputs, Swiss formats, seed-count drift, or overflow.
    pub fn schedule(&self) -> Result<TournamentSchedule, TournamentError> {
        self.validate()?;
        let cycles = self
            .format
            .static_cycles()
            .ok_or(TournamentError::SwissProgressRequired)?;
        let ordered_entrants = self.ordered_entrant_ids();
        let entrant_ranks = ordered_entrants
            .iter()
            .enumerate()
            .map(|(rank, entrant)| (entrant.clone(), rank))
            .collect::<BTreeMap<_, _>>();
        let rounds = circle_rounds(&ordered_entrants);
        let mut builder = ScheduleBuilder::new(self, 0, 0)?;
        let mut seat_balance = BTreeMap::new();
        for cycle in 0..cycles {
            for (round_index, round) in rounds.iter().enumerate() {
                let round_offset =
                    u16::try_from(round_index).map_err(|_| TournamentError::Overflow)?;
                let round_number = cycle
                    .checked_mul(
                        u16::try_from(rounds.len()).map_err(|_| TournamentError::Overflow)?,
                    )
                    .and_then(|value| value.checked_add(round_offset))
                    .and_then(|value| value.checked_add(1))
                    .ok_or(TournamentError::Overflow)?;
                let oriented =
                    orient_static_round(round, &entrant_ranks, ordered_entrants.len(), cycle);
                builder.add_round(round_number, &oriented, &mut seat_balance, true)?;
            }
        }
        builder.finish(true)
    }

    /// Generates exactly the next deterministic Swiss round.
    ///
    /// # Errors
    ///
    /// Rejects non-Swiss formats, invalid standings/history, unavoidable
    /// rematches under `avoid`, missing seeds, or completed rounds.
    pub fn schedule_swiss_round(
        &self,
        progress: &SwissProgress,
    ) -> Result<TournamentSchedule, TournamentError> {
        self.validate()?;
        let TournamentFormat::Swiss {
            rounds, rematches, ..
        } = self.format
        else {
            return Err(TournamentError::InvalidSwissProgress);
        };
        validate_swiss_progress(self, progress, rounds)?;
        let ordered = ordered_swiss_entrants(self, progress);
        let (bye, active) = choose_swiss_bye(ordered, &progress.prior_byes);
        let pairs = swiss_pairs(&active, &progress.prior_pairings, rematches)?;
        let round_number = progress
            .completed_rounds
            .checked_add(1)
            .ok_or(TournamentError::Overflow)?;
        let round = RoundPairings { pairs, bye };
        let mut balance = progress
            .seat_balance
            .iter()
            .map(|value| {
                (
                    value.entrant_id.clone(),
                    (value.seat_one_games, value.seat_two_games),
                )
            })
            .collect();
        let mut builder =
            ScheduleBuilder::new(self, progress.next_seed_index, progress.next_match_sequence)?;
        builder.add_round(round_number, &round, &mut balance, false)?;
        builder.finish(false)
    }

    fn ordered_entrant_ids(&self) -> Vec<String> {
        let mut entrants = self.entrants.clone();
        entrants.sort_by(|left, right| {
            left.seed_number
                .cmp(&right.seed_number)
                .then_with(|| left.entrant_id.cmp(&right.entrant_id))
        });
        entrants
            .into_iter()
            .map(|entrant| entrant.entrant_id)
            .collect()
    }

    /// Validates schema, IDs, entrants, profiles, seed commitments, and format bounds.
    ///
    /// # Errors
    ///
    /// Returns [`TournamentError::InvalidSpec`] for any malformed or ambiguous input.
    pub fn validate(&self) -> Result<(), TournamentError> {
        if self.schema_version != TOURNAMENT_FORMAT_SCHEMA_VERSION
            || !valid_id(&self.tournament_id)
            || self.entrants.len() < 2
            || self.profiles.is_empty()
            || self.entrants.iter().any(|entrant| !valid_entrant(entrant))
            || self.profiles.iter().any(|profile| !valid_profile(profile))
            || self
                .game_seed_commitments
                .iter()
                .any(|value| !valid_sha256(value))
        {
            return Err(TournamentError::InvalidSpec);
        }
        let entrant_ids = self
            .entrants
            .iter()
            .map(|entrant| &entrant.entrant_id)
            .collect::<BTreeSet<_>>();
        let seeds = self
            .entrants
            .iter()
            .map(|entrant| entrant.seed_number)
            .collect::<BTreeSet<_>>();
        if entrant_ids.len() != self.entrants.len() || seeds.len() != self.entrants.len() {
            return Err(TournamentError::InvalidSpec);
        }
        match self.format {
            TournamentFormat::RoundRobin { cycles }
            | TournamentFormat::PairedSeatSwap { cycles } => {
                if cycles == 0 {
                    return Err(TournamentError::InvalidSpec);
                }
            }
            TournamentFormat::Swiss {
                rounds,
                games_per_series,
                ..
            } => {
                if rounds == 0 || games_per_series == 0 {
                    return Err(TournamentError::InvalidSpec);
                }
            }
            TournamentFormat::Series {
                cycles,
                games_per_series,
                seat_policy,
            } => {
                if cycles == 0
                    || games_per_series == 0
                    || seat_policy == SeriesSeatPolicy::PairedSwap
                        && !games_per_series.is_multiple_of(2)
                {
                    return Err(TournamentError::InvalidSpec);
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct RoundPairings {
    pairs: Vec<(String, String)>,
    bye: Option<String>,
}

struct ScheduleBuilder<'a> {
    spec: &'a TournamentSpec,
    format_identity: TournamentFormatIdentity,
    seed_index: usize,
    next_sequence: u64,
    series: Vec<ScheduledSeries>,
    matches: Vec<ScheduledMatch>,
    byes: Vec<TournamentBye>,
    profile_exposure: BTreeMap<(String, usize), u32>,
    entrant_ranks: BTreeMap<String, usize>,
    static_rounds_per_cycle: u16,
}

impl<'a> ScheduleBuilder<'a> {
    fn new(
        spec: &'a TournamentSpec,
        seed_index: usize,
        next_sequence: u64,
    ) -> Result<Self, TournamentError> {
        let ordered_entrants = spec.ordered_entrant_ids();
        let static_round_count = if ordered_entrants.len().is_multiple_of(2) {
            ordered_entrants.len().saturating_sub(1)
        } else {
            ordered_entrants.len()
        };
        Ok(Self {
            spec,
            format_identity: spec.format_identity()?,
            seed_index,
            next_sequence,
            series: Vec::new(),
            matches: Vec::new(),
            byes: Vec::new(),
            profile_exposure: BTreeMap::new(),
            entrant_ranks: ordered_entrants
                .into_iter()
                .enumerate()
                .map(|(rank, entrant)| (entrant, rank))
                .collect(),
            static_rounds_per_cycle: u16::try_from(static_round_count)
                .map_err(|_| TournamentError::Overflow)?,
        })
    }

    fn add_round(
        &mut self,
        round_number: u16,
        round: &RoundPairings,
        seat_balance: &mut BTreeMap<String, (u32, u32)>,
        respect_pair_order: bool,
    ) -> Result<(), TournamentError> {
        if let Some(entrant_id) = &round.bye {
            self.byes.push(TournamentBye {
                round_number,
                entrant_id: entrant_id.clone(),
            });
        }
        for (table_index, (first, second)) in round.pairs.iter().enumerate() {
            self.add_series(
                round_number,
                u16::try_from(table_index + 1).map_err(|_| TournamentError::Overflow)?,
                first,
                second,
                seat_balance,
                respect_pair_order,
            )?;
        }
        Ok(())
    }

    fn add_series(
        &mut self,
        round_number: u16,
        table_number: u16,
        first: &str,
        second: &str,
        seat_balance: &mut BTreeMap<String, (u32, u32)>,
        respect_pair_order: bool,
    ) -> Result<(), TournamentError> {
        let series_id = format!(
            "{}-r{round_number:04}-t{table_number:04}",
            self.spec.tournament_id
        );
        let (base_one, base_two) = if respect_pair_order {
            (first.to_owned(), second.to_owned())
        } else {
            preferred_orientation(first, second, seat_balance)
        };
        let games = self.spec.format.games_per_series();
        let profile_index = if respect_pair_order {
            self.static_profile(first, second, round_number)
        } else {
            self.preferred_profile(first, second)
        };
        let mut match_ids = Vec::with_capacity(usize::from(games));
        for game_index in 0..games {
            let series_game_number = game_index.checked_add(1).ok_or(TournamentError::Overflow)?;
            let match_id = format!("{series_id}-g{series_game_number:04}");
            let (seat_one, seat_two) = if game_index % 2 == 0 {
                (&base_one, &base_two)
            } else {
                (&base_two, &base_one)
            };
            let seed = self
                .spec
                .game_seed_commitments
                .get(self.seed_index)
                .ok_or(TournamentError::MissingGameSeeds)?
                .clone();
            self.seed_index = self
                .seed_index
                .checked_add(1)
                .ok_or(TournamentError::Overflow)?;
            let sequence = self.next_sequence;
            self.next_sequence = self
                .next_sequence
                .checked_add(1)
                .ok_or(TournamentError::Overflow)?;
            match_ids.push(match_id.clone());
            self.matches.push(ScheduledMatch {
                match_id,
                series_id: series_id.clone(),
                sequence,
                round_number,
                table_number,
                series_game_number,
                seat_one_entrant_id: seat_one.clone(),
                seat_two_entrant_id: seat_two.clone(),
                profile: self.spec.profiles[profile_index].clone(),
                game_seed_commitment_sha256: seed,
            });
            update_seat_balance(seat_balance, seat_one, seat_two);
        }
        for entrant in [first, second] {
            let exposure = self
                .profile_exposure
                .entry((entrant.to_owned(), profile_index))
                .or_default();
            *exposure = exposure.saturating_add(u32::from(games));
        }
        self.series.push(ScheduledSeries {
            series_id,
            round_number,
            table_number,
            entrant_a: first.to_owned(),
            entrant_b: second.to_owned(),
            match_ids,
        });
        Ok(())
    }

    fn preferred_profile(&self, first: &str, second: &str) -> usize {
        let profile_count = self.spec.profiles.len();
        let start = self.series.len() % profile_count;
        (0..profile_count)
            .map(|offset| (start + offset) % profile_count)
            .min_by_key(|profile| {
                let first_count = self
                    .profile_exposure
                    .get(&(first.to_owned(), *profile))
                    .copied()
                    .unwrap_or_default();
                let second_count = self
                    .profile_exposure
                    .get(&(second.to_owned(), *profile))
                    .copied()
                    .unwrap_or_default();
                (
                    first_count.saturating_add(second_count),
                    first_count.max(second_count),
                    (*profile + profile_count - start) % profile_count,
                )
            })
            .expect("validated tournament profiles are non-empty")
    }

    fn static_profile(&self, first: &str, second: &str, round_number: u16) -> usize {
        let first_rank = self.entrant_ranks[first];
        let second_rank = self.entrant_ranks[second];
        let direct_distance = first_rank.abs_diff(second_rank);
        let entrant_count = self.entrant_ranks.len();
        let circular_distance = direct_distance.min(entrant_count - direct_distance);
        let cycle = usize::from((round_number - 1) / self.static_rounds_per_cycle);
        (circular_distance - 1 + cycle) % self.spec.profiles.len()
    }

    fn finish(self, require_all_seeds: bool) -> Result<TournamentSchedule, TournamentError> {
        if require_all_seeds && self.seed_index != self.spec.game_seed_commitments.len() {
            return Err(TournamentError::ExtraGameSeeds);
        }
        Ok(TournamentSchedule {
            schema_version: TOURNAMENT_SCHEDULE_SCHEMA_VERSION,
            tournament_id: self.spec.tournament_id.clone(),
            format_identity: self.format_identity,
            series: self.series,
            matches: self.matches,
            byes: self.byes,
        })
    }
}

fn circle_rounds(entrants: &[String]) -> Vec<RoundPairings> {
    let mut slots = entrants.iter().cloned().map(Some).collect::<Vec<_>>();
    if !slots.len().is_multiple_of(2) {
        slots.push(None);
    }
    let mut rounds = Vec::with_capacity(slots.len().saturating_sub(1));
    for _ in 0..slots.len().saturating_sub(1) {
        let mut pairs = Vec::new();
        let mut bye = None;
        for table in 0..slots.len() / 2 {
            let left = slots[table].clone();
            let right = slots[slots.len() - 1 - table].clone();
            match (left, right) {
                (Some(first), Some(second)) => pairs.push((first, second)),
                (Some(entrant), None) | (None, Some(entrant)) => bye = Some(entrant),
                (None, None) => {}
            }
        }
        rounds.push(RoundPairings { pairs, bye });
        let last = slots.pop().expect("validated entrant count creates slots");
        slots.insert(1, last);
    }
    rounds
}

fn orient_static_round(
    round: &RoundPairings,
    entrant_ranks: &BTreeMap<String, usize>,
    entrant_count: usize,
    cycle: u16,
) -> RoundPairings {
    let pairs = round
        .pairs
        .iter()
        .map(|(first, second)| {
            let first_rank = entrant_ranks[first];
            let second_rank = entrant_ranks[second];
            let difference = (second_rank + entrant_count - first_rank) % entrant_count;
            let first_is_seat_one =
                if entrant_count.is_multiple_of(2) && difference == entrant_count / 2 {
                    first_rank.min(second_rank).is_multiple_of(2) == (first_rank < second_rank)
                } else {
                    difference <= entrant_count / 2
                };
            if first_is_seat_one ^ !cycle.is_multiple_of(2) {
                (first.clone(), second.clone())
            } else {
                (second.clone(), first.clone())
            }
        })
        .collect();
    RoundPairings {
        pairs,
        bye: round.bye.clone(),
    }
}

fn preferred_orientation(
    first: &str,
    second: &str,
    balance: &BTreeMap<String, (u32, u32)>,
) -> (String, String) {
    let first_balance = balance.get(first).copied().unwrap_or_default();
    let second_balance = balance.get(second).copied().unwrap_or_default();
    let first_one = orientation_penalty(first_balance, second_balance);
    let second_one = orientation_penalty(second_balance, first_balance);
    if first_one < second_one || first_one == second_one && first <= second {
        (first.to_owned(), second.to_owned())
    } else {
        (second.to_owned(), first.to_owned())
    }
}

fn orientation_penalty(seat_one: (u32, u32), seat_two: (u32, u32)) -> u64 {
    u64::from(seat_one.0.saturating_add(1).abs_diff(seat_one.1))
        + u64::from(seat_two.0.abs_diff(seat_two.1.saturating_add(1)))
}

fn update_seat_balance(balance: &mut BTreeMap<String, (u32, u32)>, seat_one: &str, seat_two: &str) {
    balance.entry(seat_one.to_owned()).or_default().0 = balance
        .get(seat_one)
        .copied()
        .unwrap_or_default()
        .0
        .saturating_add(1);
    balance.entry(seat_two.to_owned()).or_default().1 = balance
        .get(seat_two)
        .copied()
        .unwrap_or_default()
        .1
        .saturating_add(1);
}

fn validate_swiss_progress(
    spec: &TournamentSpec,
    progress: &SwissProgress,
    rounds: u16,
) -> Result<(), TournamentError> {
    if progress.completed_rounds >= rounds
        || progress.standings.len() != spec.entrants.len()
        || progress.next_seed_index > spec.game_seed_commitments.len()
    {
        return Err(TournamentError::InvalidSwissProgress);
    }
    let expected = spec
        .entrants
        .iter()
        .map(|entrant| entrant.entrant_id.as_str())
        .collect::<BTreeSet<_>>();
    let actual = progress
        .standings
        .iter()
        .map(|standing| standing.entrant_id.as_str())
        .collect::<BTreeSet<_>>();
    if actual != expected || actual.len() != progress.standings.len() {
        return Err(TournamentError::InvalidSwissProgress);
    }
    Ok(())
}

fn ordered_swiss_entrants(spec: &TournamentSpec, progress: &SwissProgress) -> Vec<String> {
    let seeds = spec
        .entrants
        .iter()
        .map(|entrant| (entrant.entrant_id.as_str(), entrant.seed_number))
        .collect::<BTreeMap<_, _>>();
    let mut standings = progress.standings.clone();
    standings.sort_by(|left, right| {
        right
            .match_points
            .cmp(&left.match_points)
            .then_with(|| right.spread.cmp(&left.spread))
            .then_with(|| right.wins.cmp(&left.wins))
            .then_with(|| seeds[&left.entrant_id.as_str()].cmp(&seeds[&right.entrant_id.as_str()]))
            .then_with(|| left.entrant_id.cmp(&right.entrant_id))
    });
    standings
        .into_iter()
        .map(|standing| standing.entrant_id)
        .collect()
}

fn choose_swiss_bye(
    mut ordered: Vec<String>,
    prior_byes: &BTreeSet<String>,
) -> (Option<String>, Vec<String>) {
    if ordered.len().is_multiple_of(2) {
        return (None, ordered);
    }
    let index = ordered
        .iter()
        .rposition(|entrant| !prior_byes.contains(entrant))
        .unwrap_or(ordered.len() - 1);
    let bye = ordered.remove(index);
    (Some(bye), ordered)
}

fn swiss_pairs(
    ordered: &[String],
    prior: &BTreeSet<EntrantPairing>,
    policy: SwissRematchPolicy,
) -> Result<Vec<(String, String)>, TournamentError> {
    if let Some(pairs) = pair_without_rematches(ordered, prior) {
        return Ok(pairs);
    }
    if policy == SwissRematchPolicy::AllowWhenRequired {
        Ok(ordered
            .chunks_exact(2)
            .map(|pair| (pair[0].clone(), pair[1].clone()))
            .collect())
    } else {
        Err(TournamentError::SwissPairingUnavailable)
    }
}

fn pair_without_rematches(
    ordered: &[String],
    prior: &BTreeSet<EntrantPairing>,
) -> Option<Vec<(String, String)>> {
    let Some((first, rest)) = ordered.split_first() else {
        return Some(Vec::new());
    };
    for partner_index in 0..rest.len() {
        let partner = &rest[partner_index];
        if prior.contains(&EntrantPairing::new(first, partner)) {
            continue;
        }
        let mut remaining = rest.to_vec();
        let partner = remaining.remove(partner_index);
        if let Some(mut pairs) = pair_without_rematches(&remaining, prior) {
            pairs.insert(0, (first.clone(), partner));
            return Some(pairs);
        }
    }
    None
}

fn valid_entrant(entrant: &TournamentEntrant) -> bool {
    valid_id(&entrant.entrant_id)
        && entrant.seed_number > 0
        && entrant.manifest_sha256.as_deref().is_none_or(valid_sha256)
}

fn valid_profile(profile: &TournamentGameProfile) -> bool {
    valid_id(&profile.language)
        && valid_id(&profile.ruleset_id)
        && valid_sha256(&profile.ruleset_sha256)
        && valid_id(&profile.lexicon.pack_id)
        && valid_id(&profile.lexicon.pack_version)
        && valid_sha256(&profile.lexicon.content_sha256)
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
