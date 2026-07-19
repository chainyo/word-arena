use std::collections::BTreeSet;
use std::io::Write;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use word_arena_engine::{
    EventVisibility, GameEventKind, GameResult, PUBLIC_REPLAY_SCHEMA_VERSION, PublicReplayBundle,
    REPLAY_SCHEMA_VERSION, ReplayBundle,
};

use crate::{
    OperatorStatistics, PublicStatistics, RatingPool, RatingValue, STATISTICS_SCHEMA_VERSION,
    UnixMillis,
};

pub const EXPORT_SCHEMA_VERSION: u32 = 1;
pub const PUBLIC_REPLAY_EXPORT_SCHEMA_VERSION: u32 = 1;
pub const OPERATOR_REPLAY_EXPORT_SCHEMA_VERSION: u32 = 1;
pub const TOURNAMENT_RESULT_EXPORT_SCHEMA_VERSION: u32 = 1;
pub const STANDINGS_EXPORT_SCHEMA_VERSION: u32 = 1;
pub const RATING_EXPORT_SCHEMA_VERSION: u32 = 1;
pub const ANALYTICS_EXPORT_SCHEMA_VERSION: u32 = 1;
pub const EXPORT_POLICY_VERSION: u32 = 1;
pub const DEFAULT_MAX_EXPORT_RECORD_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAX_EXPORT_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
pub const JSONL_EXPORT_CONTENT_TYPE: &str = "application/vnd.word-arena.export+jsonl;version=1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportAudience {
    Public,
    Operator,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExportProvenance {
    pub producer: String,
    pub generated_at: UnixMillis,
    pub source_ids: Vec<String>,
    pub source_sha256s: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExportPolicy {
    pub policy_version: u32,
    pub audience: ExportAudience,
    pub redacted: bool,
    pub omitted_fields: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicReplayExport {
    pub schema_version: u32,
    pub game_id: String,
    pub replay: PublicReplayBundle,
}

impl PublicReplayExport {
    /// Removes every seat-private transition from a complete operator replay.
    ///
    /// # Errors
    ///
    /// Rejects an unsupported or incomplete replay.
    pub fn from_complete(replay: &ReplayBundle) -> Result<Self, ExportError> {
        if replay.schema_version != REPLAY_SCHEMA_VERSION {
            return Err(ExportError::IncompatibleSchema);
        }
        let game_id = replay_game_id(&replay.events)?;
        Ok(Self {
            schema_version: PUBLIC_REPLAY_EXPORT_SCHEMA_VERSION,
            game_id,
            replay: PublicReplayBundle::from(replay),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorReplayExport {
    pub schema_version: u32,
    pub game_id: String,
    pub replay: ReplayBundle,
}

impl OperatorReplayExport {
    /// Wraps the complete post-game artifact for an authorized operator.
    ///
    /// # Errors
    ///
    /// Rejects an unsupported or incomplete replay.
    pub fn from_complete(replay: ReplayBundle) -> Result<Self, ExportError> {
        if replay.schema_version != REPLAY_SCHEMA_VERSION {
            return Err(ExportError::IncompatibleSchema);
        }
        let game_id = replay_game_id(&replay.events)?;
        Ok(Self {
            schema_version: OPERATOR_REPLAY_EXPORT_SCHEMA_VERSION,
            game_id,
            replay,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TournamentMatchExport {
    pub sequence: u64,
    pub match_id: String,
    pub series_id: String,
    pub series_game_number: u16,
    pub seat_one_entrant_id: String,
    pub seat_two_entrant_id: String,
    pub result: GameResult,
    pub public_replay_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TournamentResultExport {
    pub schema_version: u32,
    pub tournament_id: String,
    pub format_identity_sha256: String,
    pub matches: Vec<TournamentMatchExport>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StandingRowExport {
    pub rank: u32,
    pub entrant_id: String,
    pub played: u64,
    pub wins: u64,
    pub losses: u64,
    pub ties: u64,
    pub spread: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StandingsExport {
    pub schema_version: u32,
    pub tournament_id: String,
    pub rows: Vec<StandingRowExport>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RatingRowExport {
    pub entrant_id: String,
    pub value: RatingValue,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RatingExport {
    pub schema_version: u32,
    pub period_sequence: u64,
    pub pool: RatingPool,
    pub rows: Vec<RatingRowExport>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicAnalyticsExport {
    pub schema_version: u32,
    pub statistics: PublicStatistics,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorAnalyticsExport {
    pub schema_version: u32,
    pub statistics: OperatorStatistics,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ExportRecord {
    PublicReplay(PublicReplayExport),
    OperatorReplay(OperatorReplayExport),
    TournamentResult(TournamentResultExport),
    Standings(StandingsExport),
    Ratings(RatingExport),
    PublicAnalytics(PublicAnalyticsExport),
    OperatorAnalytics(OperatorAnalyticsExport),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExportEnvelope {
    pub schema_version: u32,
    pub content_type: String,
    pub provenance: ExportProvenance,
    pub policy: ExportPolicy,
    pub content_sha256: String,
    pub record: ExportRecord,
}

#[derive(Serialize)]
struct ChecksummedContent<'a> {
    schema_version: u32,
    content_type: &'a str,
    provenance: &'a ExportProvenance,
    policy: &'a ExportPolicy,
    record: &'a ExportRecord,
}

impl ExportEnvelope {
    /// Builds a checked export envelope with explicit audience policy.
    ///
    /// # Errors
    ///
    /// Rejects malformed schemas, ordering, provenance, audience mismatches,
    /// public privacy violations, or serialization failure.
    pub fn new(
        record: ExportRecord,
        provenance: ExportProvenance,
        audience: ExportAudience,
    ) -> Result<Self, ExportError> {
        validate_provenance(&provenance)?;
        validate_record(&record)?;
        validate_audience(&record, audience)?;
        let policy = policy(audience);
        let content_type = record_content_type(&record, audience).to_owned();
        let content_sha256 = content_digest(
            EXPORT_SCHEMA_VERSION,
            &content_type,
            &provenance,
            &policy,
            &record,
        )?;
        let envelope = Self {
            schema_version: EXPORT_SCHEMA_VERSION,
            content_type,
            provenance,
            policy,
            content_sha256,
            record,
        };
        envelope.verify()?;
        Ok(envelope)
    }

    /// Verifies schema, policy, privacy, deterministic shape, and checksum.
    ///
    /// # Errors
    ///
    /// Returns a stable category for any invalid or modified envelope.
    pub fn verify(&self) -> Result<(), ExportError> {
        if self.schema_version != EXPORT_SCHEMA_VERSION {
            return Err(ExportError::IncompatibleSchema);
        }
        validate_provenance(&self.provenance)?;
        validate_record(&self.record)?;
        validate_audience(&self.record, self.policy.audience)?;
        if self.policy != policy(self.policy.audience)
            || self.content_type != record_content_type(&self.record, self.policy.audience)
        {
            return Err(ExportError::InvalidInput);
        }
        let expected = content_digest(
            self.schema_version,
            &self.content_type,
            &self.provenance,
            &self.policy,
            &self.record,
        )?;
        if self.content_sha256 != expected {
            return Err(ExportError::ChecksumMismatch);
        }
        if self.policy.audience == ExportAudience::Public {
            let value = serde_json::to_value(self).map_err(|_| ExportError::Serialization)?;
            validate_public_value(&value)?;
        }
        Ok(())
    }

    /// Stable JSONL ordering identity.
    ///
    /// # Errors
    ///
    /// Returns if the record identity cannot be derived.
    pub fn sort_key(&self) -> Result<String, ExportError> {
        record_sort_key(&self.record)
    }
}

#[derive(Debug)]
pub struct JsonlExporter<W> {
    writer: W,
    max_record_bytes: usize,
    max_total_bytes: u64,
    total_bytes: u64,
    record_count: u64,
    last_sort_key: Option<String>,
    digest: Sha256,
}

impl<W: Write> JsonlExporter<W> {
    /// Creates a bounded deterministic JSONL stream.
    ///
    /// # Errors
    ///
    /// Rejects zero limits or limits above the documented hard ceilings.
    pub fn new(
        writer: W,
        max_record_bytes: usize,
        max_total_bytes: u64,
    ) -> Result<Self, ExportError> {
        if max_record_bytes == 0
            || max_record_bytes > DEFAULT_MAX_EXPORT_RECORD_BYTES
            || max_total_bytes == 0
            || max_total_bytes > DEFAULT_MAX_EXPORT_TOTAL_BYTES
        {
            return Err(ExportError::InvalidInput);
        }
        Ok(Self {
            writer,
            max_record_bytes,
            max_total_bytes,
            total_bytes: 0,
            record_count: 0,
            last_sort_key: None,
            digest: Sha256::new(),
        })
    }

    /// Writes one verified envelope in strictly increasing identity order.
    ///
    /// # Errors
    ///
    /// Rejects invalid envelopes, duplicate/out-of-order records, size limits,
    /// serialization errors, or writer failures.
    pub fn write(&mut self, envelope: &ExportEnvelope) -> Result<(), ExportError> {
        envelope.verify()?;
        let sort_key = envelope.sort_key()?;
        if self
            .last_sort_key
            .as_ref()
            .is_some_and(|previous| previous >= &sort_key)
        {
            return Err(ExportError::OutOfOrder);
        }
        let mut bytes = serde_json::to_vec(envelope).map_err(|_| ExportError::Serialization)?;
        if bytes.len() > self.max_record_bytes {
            return Err(ExportError::RecordTooLarge);
        }
        bytes.push(b'\n');
        let length = u64::try_from(bytes.len()).map_err(|_| ExportError::TotalTooLarge)?;
        let total = self
            .total_bytes
            .checked_add(length)
            .ok_or(ExportError::TotalTooLarge)?;
        if total > self.max_total_bytes {
            return Err(ExportError::TotalTooLarge);
        }
        self.writer.write_all(&bytes).map_err(|_| ExportError::Io)?;
        self.digest.update(&bytes);
        self.total_bytes = total;
        self.record_count = self
            .record_count
            .checked_add(1)
            .ok_or(ExportError::TotalTooLarge)?;
        self.last_sort_key = Some(sort_key);
        Ok(())
    }

    /// Flushes the stream and returns its writer plus deterministic summary.
    ///
    /// # Errors
    ///
    /// Returns when the writer cannot be flushed.
    pub fn finish(mut self) -> Result<(W, ExportSummary), ExportError> {
        self.writer.flush().map_err(|_| ExportError::Io)?;
        let checksum_sha256 = hex_digest(self.digest.finalize().as_slice());
        Ok((
            self.writer,
            ExportSummary {
                schema_version: EXPORT_SCHEMA_VERSION,
                content_type: JSONL_EXPORT_CONTENT_TYPE.to_owned(),
                record_count: self.record_count,
                byte_count: self.total_bytes,
                checksum_sha256,
            },
        ))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExportSummary {
    pub schema_version: u32,
    pub content_type: String,
    pub record_count: u64,
    pub byte_count: u64,
    pub checksum_sha256: String,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ExportError {
    #[error("export input is invalid")]
    InvalidInput,
    #[error("export schema is incompatible")]
    IncompatibleSchema,
    #[error("export audience or privacy policy was violated")]
    PrivacyViolation,
    #[error("export checksum does not match content")]
    ChecksumMismatch,
    #[error("export records are duplicate or out of order")]
    OutOfOrder,
    #[error("one export record exceeds its byte limit")]
    RecordTooLarge,
    #[error("export stream exceeds its byte limit")]
    TotalTooLarge,
    #[error("export serialization failed")]
    Serialization,
    #[error("export writer failed")]
    Io,
}

fn validate_record(record: &ExportRecord) -> Result<(), ExportError> {
    match record {
        ExportRecord::PublicReplay(export) => validate_public_replay(export),
        ExportRecord::OperatorReplay(export) => validate_operator_replay(export),
        ExportRecord::TournamentResult(export) => validate_tournament(export),
        ExportRecord::Standings(export) => validate_standings(export),
        ExportRecord::Ratings(export) => validate_ratings(export),
        ExportRecord::PublicAnalytics(export) => {
            if export.schema_version != ANALYTICS_EXPORT_SCHEMA_VERSION
                || export.statistics.schema_version != STATISTICS_SCHEMA_VERSION
            {
                Err(ExportError::IncompatibleSchema)
            } else {
                Ok(())
            }
        }
        ExportRecord::OperatorAnalytics(export) => {
            if export.schema_version != ANALYTICS_EXPORT_SCHEMA_VERSION
                || export.statistics.public.schema_version != STATISTICS_SCHEMA_VERSION
                || !is_sorted_unique(&export.statistics.source_ids)
            {
                Err(ExportError::IncompatibleSchema)
            } else {
                Ok(())
            }
        }
    }
}

fn validate_public_replay(export: &PublicReplayExport) -> Result<(), ExportError> {
    if export.schema_version != PUBLIC_REPLAY_EXPORT_SCHEMA_VERSION
        || export.replay.schema_version != PUBLIC_REPLAY_SCHEMA_VERSION
        || export.game_id != replay_game_id(&export.replay.events)?
        || export.replay.ruleset_identity != export.replay.ruleset.identity()
        || export.replay.lexicon != export.replay.ruleset.lexicon
        || export
            .replay
            .events
            .iter()
            .any(|event| event.lexicon != export.replay.lexicon)
    {
        return Err(ExportError::IncompatibleSchema);
    }
    Ok(())
}

fn validate_operator_replay(export: &OperatorReplayExport) -> Result<(), ExportError> {
    if export.schema_version != OPERATOR_REPLAY_EXPORT_SCHEMA_VERSION
        || export.replay.schema_version != REPLAY_SCHEMA_VERSION
        || export.game_id != replay_game_id(&export.replay.events)?
        || export.replay.ruleset_identity != export.replay.ruleset.identity()
        || export.replay.lexicon != export.replay.ruleset.lexicon
        || export
            .replay
            .events
            .iter()
            .any(|event| event.lexicon != export.replay.lexicon)
    {
        return Err(ExportError::IncompatibleSchema);
    }
    Ok(())
}

fn validate_tournament(export: &TournamentResultExport) -> Result<(), ExportError> {
    if export.schema_version != TOURNAMENT_RESULT_EXPORT_SCHEMA_VERSION
        || !valid_id(&export.tournament_id)
        || !valid_sha256(&export.format_identity_sha256)
    {
        return Err(ExportError::IncompatibleSchema);
    }
    let mut identities = BTreeSet::new();
    for (index, game) in export.matches.iter().enumerate() {
        if game.sequence != u64::try_from(index).map_err(|_| ExportError::InvalidInput)?
            || !valid_id(&game.match_id)
            || !valid_id(&game.series_id)
            || game.series_game_number == 0
            || !valid_id(&game.seat_one_entrant_id)
            || !valid_id(&game.seat_two_entrant_id)
            || game.seat_one_entrant_id == game.seat_two_entrant_id
            || !valid_sha256(&game.public_replay_sha256)
            || !identities.insert(&game.match_id)
        {
            return Err(ExportError::InvalidInput);
        }
    }
    Ok(())
}

fn validate_standings(export: &StandingsExport) -> Result<(), ExportError> {
    if export.schema_version != STANDINGS_EXPORT_SCHEMA_VERSION || !valid_id(&export.tournament_id)
    {
        return Err(ExportError::IncompatibleSchema);
    }
    let mut entrants = BTreeSet::new();
    for (index, row) in export.rows.iter().enumerate() {
        if row.rank == 0
            || !valid_id(&row.entrant_id)
            || !entrants.insert(&row.entrant_id)
            || row
                .wins
                .checked_add(row.losses)
                .and_then(|value| value.checked_add(row.ties))
                != Some(row.played)
            || export
                .rows
                .get(index.wrapping_sub(1))
                .is_some_and(|previous| {
                    (previous.rank, &previous.entrant_id) > (row.rank, &row.entrant_id)
                })
        {
            return Err(ExportError::InvalidInput);
        }
    }
    Ok(())
}

fn validate_ratings(export: &RatingExport) -> Result<(), ExportError> {
    if export.schema_version != RATING_EXPORT_SCHEMA_VERSION || export.pool.key().is_err() {
        return Err(ExportError::IncompatibleSchema);
    }
    if export.rows.iter().any(|row| {
        !valid_id(&row.entrant_id)
            || row.value.deviation_milli == 0
            || row.value.volatility_nano == 0
    }) || !export
        .rows
        .windows(2)
        .all(|rows| rows[0].entrant_id < rows[1].entrant_id)
    {
        return Err(ExportError::InvalidInput);
    }
    Ok(())
}

fn validate_provenance(provenance: &ExportProvenance) -> Result<(), ExportError> {
    if !valid_id(&provenance.producer)
        || provenance.generated_at.0 < 0
        || !is_sorted_unique(&provenance.source_ids)
        || !is_sorted_unique(&provenance.source_sha256s)
        || provenance
            .source_sha256s
            .iter()
            .any(|digest| !valid_sha256(digest))
    {
        return Err(ExportError::InvalidInput);
    }
    Ok(())
}

fn validate_audience(record: &ExportRecord, audience: ExportAudience) -> Result<(), ExportError> {
    if audience == ExportAudience::Public
        && matches!(
            record,
            ExportRecord::OperatorReplay(_) | ExportRecord::OperatorAnalytics(_)
        )
    {
        Err(ExportError::PrivacyViolation)
    } else {
        Ok(())
    }
}

fn policy(audience: ExportAudience) -> ExportPolicy {
    let omitted_fields = match audience {
        ExportAudience::Public => vec![
            "capabilities",
            "diagnostics",
            "lexicon_contents",
            "private_events",
            "racks",
            "tool_arguments",
            "tool_results",
            "transcripts",
            "word_frequencies",
        ],
        ExportAudience::Operator => {
            vec!["capabilities", "hidden_reasoning", "provider_credentials"]
        }
    }
    .into_iter()
    .map(str::to_owned)
    .collect();
    ExportPolicy {
        policy_version: EXPORT_POLICY_VERSION,
        audience,
        redacted: audience == ExportAudience::Public,
        omitted_fields,
    }
}

fn record_content_type(record: &ExportRecord, audience: ExportAudience) -> &'static str {
    match (record, audience) {
        (ExportRecord::PublicReplay(_), ExportAudience::Public) => {
            "application/vnd.word-arena.public-replay+json;version=1"
        }
        (
            ExportRecord::OperatorReplay(_) | ExportRecord::PublicReplay(_),
            ExportAudience::Operator,
        ) => "application/vnd.word-arena.operator-replay+json;version=1",
        (ExportRecord::TournamentResult(_), _) => {
            "application/vnd.word-arena.tournament-result+json;version=1"
        }
        (ExportRecord::Standings(_), _) => "application/vnd.word-arena.standings+json;version=1",
        (ExportRecord::Ratings(_), _) => "application/vnd.word-arena.ratings+json;version=1",
        (ExportRecord::PublicAnalytics(_), ExportAudience::Public) => {
            "application/vnd.word-arena.public-analytics+json;version=1"
        }
        (
            ExportRecord::OperatorAnalytics(_) | ExportRecord::PublicAnalytics(_),
            ExportAudience::Operator,
        ) => "application/vnd.word-arena.operator-analytics+json;version=1",
        (
            ExportRecord::OperatorReplay(_) | ExportRecord::OperatorAnalytics(_),
            ExportAudience::Public,
        ) => "invalid",
    }
}

fn record_sort_key(record: &ExportRecord) -> Result<String, ExportError> {
    Ok(match record {
        ExportRecord::PublicReplay(export) => format!("01:public-replay:{}", export.game_id),
        ExportRecord::OperatorReplay(export) => format!("02:operator-replay:{}", export.game_id),
        ExportRecord::TournamentResult(export) => {
            format!("03:tournament-result:{}", export.tournament_id)
        }
        ExportRecord::Standings(export) => format!("04:standings:{}", export.tournament_id),
        ExportRecord::Ratings(export) => format!(
            "05:ratings:{}:{:020}",
            export.pool.key().map_err(|_| ExportError::InvalidInput)?,
            export.period_sequence
        ),
        ExportRecord::PublicAnalytics(export) => format!(
            "06:public-analytics:{}",
            sha256(
                &serde_json::to_vec(&export.statistics.filter)
                    .map_err(|_| ExportError::Serialization)?
            )
        ),
        ExportRecord::OperatorAnalytics(export) => format!(
            "07:operator-analytics:{}",
            sha256(
                &serde_json::to_vec(&export.statistics.public.filter)
                    .map_err(|_| ExportError::Serialization)?
            )
        ),
    })
}

fn content_digest(
    schema_version: u32,
    content_type: &str,
    provenance: &ExportProvenance,
    policy: &ExportPolicy,
    record: &ExportRecord,
) -> Result<String, ExportError> {
    let bytes = serde_json::to_vec(&ChecksummedContent {
        schema_version,
        content_type,
        provenance,
        policy,
        record,
    })
    .map_err(|_| ExportError::Serialization)?;
    Ok(sha256(&bytes))
}

fn replay_game_id(events: &[word_arena_engine::GameEvent]) -> Result<String, ExportError> {
    let first = events.first().ok_or(ExportError::InvalidInput)?;
    let GameEventKind::Created { game_id, .. } = &first.kind else {
        return Err(ExportError::InvalidInput);
    };
    if first.sequence != 0 || !valid_id(game_id) {
        return Err(ExportError::InvalidInput);
    }
    for (index, event) in events.iter().enumerate() {
        if event.sequence != u64::try_from(index).map_err(|_| ExportError::InvalidInput)?
            || event.visibility != EventVisibility::Public
        {
            return Err(ExportError::InvalidInput);
        }
    }
    let terminal = match &events.last().ok_or(ExportError::InvalidInput)?.kind {
        GameEventKind::MovePlayed { result, .. }
        | GameEventKind::Passed { result, .. }
        | GameEventKind::Exchanged { result, .. } => result.is_some(),
        GameEventKind::Resigned { .. } => true,
        GameEventKind::Created { .. } => false,
    };
    if !terminal {
        return Err(ExportError::InvalidInput);
    }
    Ok(game_id.clone())
}

fn validate_public_value(value: &Value) -> Result<(), ExportError> {
    const FORBIDDEN_KEYS: [&str; 12] = [
        "capability",
        "credentials",
        "diagnostics",
        "private_events",
        "provider_secret",
        "rack",
        "rack_after",
        "tool_arguments",
        "tool_results",
        "transcript",
        "visible_input",
        "visible_output",
    ];
    match value {
        Value::Object(map) => {
            for (key, nested) in map {
                let lower = key.to_ascii_lowercase();
                if FORBIDDEN_KEYS.iter().any(|forbidden| lower == *forbidden) {
                    return Err(ExportError::PrivacyViolation);
                }
                validate_public_value(nested)?;
            }
        }
        Value::Array(values) => {
            for nested in values {
                validate_public_value(nested)?;
            }
        }
        Value::String(text)
            if text.contains("wa_cap_v1") || text.to_ascii_lowercase().contains("bearer ") =>
        {
            return Err(ExportError::PrivacyViolation);
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn is_sorted_unique(values: &[String]) -> bool {
    values.iter().all(|value| valid_id(value)) && values.windows(2).all(|pair| pair[0] < pair[1])
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.trim() == value
        && value.chars().all(|character| !character.is_control())
        && !value.contains("wa_cap_v1")
        && !value.to_ascii_lowercase().contains("bearer ")
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn sha256(bytes: &[u8]) -> String {
    hex_digest(&Sha256::digest(bytes))
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().fold(
        String::with_capacity(bytes.len() * 2),
        |mut output, byte| {
            use std::fmt::Write;
            write!(&mut output, "{byte:02x}").expect("writing a digest to String cannot fail");
            output
        },
    )
}
