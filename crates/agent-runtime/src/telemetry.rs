use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::{
    AgentManifestIdentity, DRIVER_TELEMETRY_SCHEMA_VERSION, DiagnosticStream, DriverLifecycleState,
    DriverTelemetry, LifecycleTransition, VisibleToolCall,
};

pub const RUN_TELEMETRY_SCHEMA_VERSION: u32 = 1;
pub const TELEMETRY_REDACTION_POLICY_VERSION: u32 = 1;
pub const MAX_TELEMETRY_TEXT_BYTES: usize = 16_384;
pub const MAX_TELEMETRY_JSON_BYTES: usize = 65_536;
pub const MAX_TELEMETRY_TURNS: usize = 1_024;
pub const MAX_TELEMETRY_TOOL_CALLS_PER_TURN: usize = 256;
pub const MAX_TELEMETRY_DIAGNOSTICS: usize = 2_048;
pub const MAX_TELEMETRY_LIFECYCLE_EVENTS: usize = 4_096;
pub const REDACTION_MARKER: &str = "[REDACTED]";
pub const TRUNCATION_MARKER: &str = "[TRUNCATED]";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryAvailability {
    Exact,
    Estimated,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourcedU64 {
    pub availability: TelemetryAvailability,
    pub value: Option<u64>,
    pub source: String,
}

impl SourcedU64 {
    /// Creates a source-labelled metric and validates availability semantics.
    ///
    /// # Errors
    ///
    /// Rejects an empty/unsafe source, a missing available value, or a value
    /// presented as unavailable.
    pub fn new(
        availability: TelemetryAvailability,
        value: Option<u64>,
        source: impl Into<String>,
    ) -> Result<Self, RunTelemetryError> {
        let metric = Self {
            availability,
            value,
            source: source.into(),
        };
        metric.validate()?;
        Ok(metric)
    }

    /// Adds source measurements without silently overflowing cost/token totals.
    ///
    /// The result is unavailable when no source exposes a value and estimated
    /// when at least one contributing value is estimated.
    ///
    /// # Errors
    ///
    /// Rejects malformed samples or arithmetic overflow.
    pub fn checked_sum(
        samples: &[Self],
        source: impl Into<String>,
    ) -> Result<Self, RunTelemetryError> {
        let mut total = 0_u64;
        let mut any = false;
        let mut estimated = false;
        for sample in samples {
            sample.validate()?;
            match sample.availability {
                TelemetryAvailability::Exact => {
                    total = total
                        .checked_add(sample.value.ok_or(RunTelemetryError::Invalid)?)
                        .ok_or(RunTelemetryError::Overflow)?;
                    any = true;
                }
                TelemetryAvailability::Estimated => {
                    total = total
                        .checked_add(sample.value.ok_or(RunTelemetryError::Invalid)?)
                        .ok_or(RunTelemetryError::Overflow)?;
                    any = true;
                    estimated = true;
                }
                TelemetryAvailability::Unavailable => {}
            }
        }
        Self::new(
            if !any {
                TelemetryAvailability::Unavailable
            } else if estimated {
                TelemetryAvailability::Estimated
            } else {
                TelemetryAvailability::Exact
            },
            any.then_some(total),
            source,
        )
    }

    fn validate(&self) -> Result<(), RunTelemetryError> {
        validate_label(&self.source)?;
        match (self.availability, self.value) {
            (TelemetryAvailability::Unavailable, None)
            | (TelemetryAvailability::Exact | TelemetryAvailability::Estimated, Some(_)) => Ok(()),
            _ => Err(RunTelemetryError::Invalid),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunUsageTelemetry {
    pub input_tokens: SourcedU64,
    pub output_tokens: SourcedU64,
    pub cost_microusd: SourcedU64,
}

impl RunUsageTelemetry {
    /// Marks all provider-dependent usage dimensions unavailable explicitly.
    ///
    /// # Errors
    ///
    /// Rejects an invalid source label.
    pub fn unavailable(source: &str) -> Result<Self, RunTelemetryError> {
        Ok(Self {
            input_tokens: SourcedU64::new(TelemetryAvailability::Unavailable, None, source)?,
            output_tokens: SourcedU64::new(TelemetryAvailability::Unavailable, None, source)?,
            cost_microusd: SourcedU64::new(TelemetryAvailability::Unavailable, None, source)?,
        })
    }

    fn validate(&self) -> Result<(), RunTelemetryError> {
        self.input_tokens.validate()?;
        self.output_tokens.validate()?;
        self.cost_microusd.validate()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunTelemetryCorrelation {
    pub tournament_id: Option<String>,
    pub match_id: Option<String>,
    pub game_id: String,
    pub run_id: String,
    pub seat_number: u8,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryRetentionKind {
    Retain,
    Expire,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryRetentionPolicy {
    pub kind: TelemetryRetentionKind,
    pub expires_at_unix_ms: Option<i64>,
}

impl TelemetryRetentionPolicy {
    #[must_use]
    pub const fn retain() -> Self {
        Self {
            kind: TelemetryRetentionKind::Retain,
            expires_at_unix_ms: None,
        }
    }

    #[must_use]
    pub const fn expire_at(expires_at_unix_ms: i64) -> Self {
        Self {
            kind: TelemetryRetentionKind::Expire,
            expires_at_unix_ms: Some(expires_at_unix_ms),
        }
    }

    fn validate(&self, captured_at_unix_ms: i64) -> Result<(), RunTelemetryError> {
        match (self.kind, self.expires_at_unix_ms) {
            (TelemetryRetentionKind::Retain, None) => Ok(()),
            (TelemetryRetentionKind::Expire, Some(value))
                if value >= captured_at_unix_ms && captured_at_unix_ms >= 0 =>
            {
                Ok(())
            }
            _ => Err(RunTelemetryError::Invalid),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetrySanitization {
    pub policy_version: u32,
    pub redacted_values: u64,
    pub truncated_values: u64,
    pub replaced_control_characters: u64,
    pub replaced_invalid_utf8_sequences: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunTelemetrySources {
    pub transcripts: String,
    pub tool_calls: String,
    pub timings: String,
    pub retries: String,
    pub failures: String,
}

impl RunTelemetrySources {
    fn driver_v1() -> Self {
        Self {
            transcripts: "driver_visible_protocol".to_owned(),
            tool_calls: "driver_visible_tool_calls".to_owned(),
            timings: "injected_driver_clock".to_owned(),
            retries: "driver_checkpoint".to_owned(),
            failures: "driver_diagnostics".to_owned(),
        }
    }

    fn validate(&self) -> Result<(), RunTelemetryError> {
        for source in [
            &self.transcripts,
            &self.tool_calls,
            &self.timings,
            &self.retries,
            &self.failures,
        ] {
            validate_label(source)?;
        }
        if self == &Self::driver_v1() {
            Ok(())
        } else {
            Err(RunTelemetryError::Invalid)
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArchivedToolCall {
    pub tool: String,
    pub arguments: Value,
    pub result: Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArchivedTurnTelemetry {
    pub sequence: u64,
    pub turn_id: String,
    pub started_at_unix_ms: i64,
    pub completed_at_unix_ms: i64,
    pub duration_ms: u64,
    pub visible_input: String,
    pub visible_output: String,
    pub tool_calls: Vec<ArchivedToolCall>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArchivedDiagnostic {
    pub sequence: u64,
    pub at_unix_ms: i64,
    pub stream: DiagnosticStream,
    pub code: String,
    pub visible_text: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunTelemetryArchive {
    pub schema_version: u32,
    pub driver_telemetry_schema_version: u32,
    pub correlation: RunTelemetryCorrelation,
    pub manifest: AgentManifestIdentity,
    pub captured_at_unix_ms: i64,
    pub retention: TelemetryRetentionPolicy,
    pub sources: RunTelemetrySources,
    pub retry_count: SourcedU64,
    pub usage: RunUsageTelemetry,
    pub lifecycle: Vec<LifecycleTransition>,
    pub turns: Vec<ArchivedTurnTelemetry>,
    pub diagnostics: Vec<ArchivedDiagnostic>,
    pub sanitization: TelemetrySanitization,
}

impl RunTelemetryArchive {
    /// Converts typed visible driver telemetry into a bounded, sanitized archive.
    ///
    /// # Errors
    ///
    /// Rejects identity, correlation, schema, ordering, time, metric, or
    /// retention drift before any archive bytes can be persisted.
    pub fn capture(
        correlation: RunTelemetryCorrelation,
        manifest: AgentManifestIdentity,
        driver: &DriverTelemetry,
        usage: RunUsageTelemetry,
        retention: TelemetryRetentionPolicy,
        captured_at_unix_ms: i64,
        sanitizer: &TelemetrySanitizer,
    ) -> Result<Self, RunTelemetryError> {
        validate_correlation(&correlation)?;
        validate_identity(&manifest)?;
        usage.validate()?;
        retention.validate(captured_at_unix_ms)?;
        if captured_at_unix_ms < 0
            || driver.schema_version != DRIVER_TELEMETRY_SCHEMA_VERSION
            || driver.run_id != correlation.run_id
            || driver.manifest != manifest
            || driver
                .lifecycle
                .iter()
                .enumerate()
                .any(|(sequence, event)| event.sequence != sequence as u64 || event.at_unix_ms < 0)
            || driver
                .diagnostics
                .iter()
                .enumerate()
                .any(|(sequence, event)| event.sequence != sequence as u64 || event.at_unix_ms < 0)
            || driver.turns.iter().any(|turn| {
                turn.started_at_unix_ms < 0 || turn.completed_at_unix_ms < turn.started_at_unix_ms
            })
        {
            return Err(RunTelemetryError::Invalid);
        }

        let mut stats = TelemetrySanitization {
            policy_version: TELEMETRY_REDACTION_POLICY_VERSION,
            ..TelemetrySanitization::default()
        };
        let lifecycle = archive_lifecycle(driver, sanitizer, &mut stats);
        let turns = archive_turns(driver, sanitizer, &mut stats);
        let diagnostics = archive_diagnostics(driver, sanitizer, &mut stats);

        let archive = Self {
            schema_version: RUN_TELEMETRY_SCHEMA_VERSION,
            driver_telemetry_schema_version: driver.schema_version,
            correlation,
            manifest,
            captured_at_unix_ms,
            retention,
            sources: RunTelemetrySources::driver_v1(),
            retry_count: SourcedU64::new(
                TelemetryAvailability::Exact,
                Some(u64::from(driver.restarts)),
                "driver_checkpoint",
            )?,
            usage,
            lifecycle,
            turns,
            diagnostics,
            sanitization: stats,
        };
        archive.validate()?;
        Ok(archive)
    }

    /// Produces the only public analytics/export representation.
    ///
    /// Transcript text, diagnostic text, tool arguments, and tool results are
    /// structurally absent rather than relying on a later redaction pass.
    #[must_use]
    pub fn public_projection(&self) -> PublicRunTelemetry {
        PublicRunTelemetry {
            schema_version: RUN_TELEMETRY_SCHEMA_VERSION,
            correlation: self.correlation.clone(),
            manifest: self.manifest.clone(),
            captured_at_unix_ms: self.captured_at_unix_ms,
            retry_count: self.retry_count.clone(),
            usage: self.usage.clone(),
            sources: self.sources.clone(),
            turns: self
                .turns
                .iter()
                .map(|turn| PublicTurnTelemetry {
                    sequence: turn.sequence,
                    turn_id: turn.turn_id.clone(),
                    started_at_unix_ms: turn.started_at_unix_ms,
                    completed_at_unix_ms: turn.completed_at_unix_ms,
                    duration_ms: turn.duration_ms,
                    tools: turn
                        .tool_calls
                        .iter()
                        .map(|call| call.tool.clone())
                        .collect(),
                })
                .collect(),
            failure_codes: self
                .diagnostics
                .iter()
                .map(|record| record.code.clone())
                .collect(),
            privacy: PublicTelemetryPrivacy {
                source_schema_version: self.schema_version,
                redaction_policy_version: self.sanitization.policy_version,
                content_fields_omitted: true,
            },
        }
    }

    /// Revalidates deserialized storage bytes before they are exposed.
    ///
    /// # Errors
    ///
    /// Rejects schema, identity, source, sequence, retention, or timing drift.
    pub fn validate(&self) -> Result<(), RunTelemetryError> {
        validate_correlation(&self.correlation)?;
        validate_identity(&self.manifest)?;
        self.retry_count.validate()?;
        self.usage.validate()?;
        self.sources.validate()?;
        self.retention.validate(self.captured_at_unix_ms)?;
        if self.schema_version != RUN_TELEMETRY_SCHEMA_VERSION
            || self.driver_telemetry_schema_version != DRIVER_TELEMETRY_SCHEMA_VERSION
            || self.captured_at_unix_ms < 0
            || self.sanitization.policy_version != TELEMETRY_REDACTION_POLICY_VERSION
            || self.retry_count.availability != TelemetryAvailability::Exact
            || self.retry_count.source != "driver_checkpoint"
            || self.lifecycle.len() > MAX_TELEMETRY_LIFECYCLE_EVENTS
            || self.turns.len() > MAX_TELEMETRY_TURNS
            || self.diagnostics.len() > MAX_TELEMETRY_DIAGNOSTICS
            || self.lifecycle.iter().enumerate().any(|(sequence, event)| {
                event.sequence != sequence as u64
                    || event.at_unix_ms < 0
                    || matches!(
                        &event.state,
                        DriverLifecycleState::TurnRunning { turn_id }
                            if !is_sanitized_text(turn_id)
                    )
            })
            || self.turns.iter().enumerate().any(|(sequence, turn)| {
                turn.sequence != sequence as u64
                    || turn.started_at_unix_ms < 0
                    || turn.completed_at_unix_ms < turn.started_at_unix_ms
                    || turn.duration_ms
                        != u64::try_from(turn.completed_at_unix_ms - turn.started_at_unix_ms)
                            .unwrap_or(u64::MAX)
                    || !is_sanitized_text(&turn.turn_id)
                    || !is_sanitized_text(&turn.visible_input)
                    || !is_sanitized_text(&turn.visible_output)
                    || turn.tool_calls.len() > MAX_TELEMETRY_TOOL_CALLS_PER_TURN
                    || turn.tool_calls.iter().any(|call| {
                        !is_sanitized_text(&call.tool)
                            || !is_sanitized_json(&call.arguments, 0)
                            || !is_sanitized_json(&call.result, 0)
                    })
            })
            || self
                .diagnostics
                .iter()
                .enumerate()
                .any(|(sequence, record)| {
                    record.sequence != sequence as u64
                        || record.at_unix_ms < 0
                        || !is_sanitized_text(&record.code)
                        || !is_sanitized_text(&record.visible_text)
                })
        {
            return Err(RunTelemetryError::Invalid);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicTurnTelemetry {
    pub sequence: u64,
    pub turn_id: String,
    pub started_at_unix_ms: i64,
    pub completed_at_unix_ms: i64,
    pub duration_ms: u64,
    pub tools: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicTelemetryPrivacy {
    pub source_schema_version: u32,
    pub redaction_policy_version: u32,
    pub content_fields_omitted: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicRunTelemetry {
    pub schema_version: u32,
    pub correlation: RunTelemetryCorrelation,
    pub manifest: AgentManifestIdentity,
    pub captured_at_unix_ms: i64,
    pub retry_count: SourcedU64,
    pub usage: RunUsageTelemetry,
    pub sources: RunTelemetrySources,
    pub turns: Vec<PublicTurnTelemetry>,
    pub failure_codes: Vec<String>,
    pub privacy: PublicTelemetryPrivacy,
}

#[derive(Clone, Error, Debug, Eq, PartialEq)]
pub enum RunTelemetryError {
    #[error("run telemetry input is invalid")]
    Invalid,
    #[error("run telemetry arithmetic overflowed")]
    Overflow,
}

#[derive(Clone)]
pub struct TelemetrySanitizer {
    secrets: Vec<Vec<u8>>,
}

impl fmt::Debug for TelemetrySanitizer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TelemetrySanitizer")
            .field("secret_count", &self.secrets.len())
            .finish()
    }
}

impl TelemetrySanitizer {
    /// Creates a sanitizer from trusted raw secret material.
    ///
    /// Empty secrets are ignored and debug output never exposes values.
    #[must_use]
    pub fn new(secrets: impl IntoIterator<Item = Vec<u8>>) -> Self {
        Self {
            secrets: secrets
                .into_iter()
                .filter(|secret| !secret.is_empty())
                .collect(),
        }
    }

    /// Sanitizes an untrusted process byte sequence for a visible diagnostic.
    ///
    /// Invalid UTF-8, control characters, configured secrets, common bearer
    /// token forms, and size limits are handled before returning text.
    #[must_use]
    pub fn sanitize_untrusted_bytes(&self, value: &[u8]) -> (String, TelemetrySanitization) {
        let mut stats = TelemetrySanitization {
            policy_version: TELEMETRY_REDACTION_POLICY_VERSION,
            ..TelemetrySanitization::default()
        };
        let value = self.sanitize_bytes(value, &mut stats);
        (value, stats)
    }

    fn sanitize_text(&self, value: &str, stats: &mut TelemetrySanitization) -> String {
        self.sanitize_bytes(value.as_bytes(), stats)
    }

    fn sanitize_bytes(&self, value: &[u8], stats: &mut TelemetrySanitization) -> String {
        let mut bytes = value.to_vec();
        for secret in &self.secrets {
            replace_bytes(&mut bytes, secret, REDACTION_MARKER.as_bytes(), stats);
        }
        for prefix in [b"wa_cap_v1.".as_slice(), b"sk-", b"ghp_", b"Bearer "] {
            redact_prefixed_tokens(&mut bytes, prefix, stats);
        }
        let lossy = String::from_utf8_lossy(&bytes);
        stats.replaced_invalid_utf8_sequences = stats
            .replaced_invalid_utf8_sequences
            .saturating_add(lossy.matches('\u{fffd}').count() as u64);
        let mut clean = String::with_capacity(lossy.len());
        for character in lossy.chars() {
            if character.is_control() && !matches!(character, '\n' | '\t') {
                clean.push('\u{fffd}');
                stats.replaced_control_characters =
                    stats.replaced_control_characters.saturating_add(1);
            } else {
                clean.push(character);
            }
        }
        truncate_utf8(clean, MAX_TELEMETRY_TEXT_BYTES, stats)
    }

    fn sanitize_json(&self, value: &Value, stats: &mut TelemetrySanitization) -> Value {
        let mut sanitized = self.sanitize_json_at_depth(value, stats, 0);
        if serde_json::to_vec(&sanitized)
            .map_or(true, |bytes| bytes.len() > MAX_TELEMETRY_JSON_BYTES)
        {
            stats.truncated_values = stats.truncated_values.saturating_add(1);
            sanitized = Value::String(TRUNCATION_MARKER.to_owned());
        }
        sanitized
    }

    fn sanitize_json_at_depth(
        &self,
        value: &Value,
        stats: &mut TelemetrySanitization,
        depth: usize,
    ) -> Value {
        if depth >= 32 {
            stats.truncated_values = stats.truncated_values.saturating_add(1);
            return Value::String(TRUNCATION_MARKER.to_owned());
        }
        match value {
            Value::String(value) => Value::String(self.sanitize_text(value, stats)),
            Value::Array(values) => {
                if values.len() > 1_024 {
                    stats.truncated_values = stats.truncated_values.saturating_add(1);
                }
                Value::Array(
                    values
                        .iter()
                        .take(1_024)
                        .map(|value| self.sanitize_json_at_depth(value, stats, depth + 1))
                        .collect(),
                )
            }
            Value::Object(values) => Value::Object(
                values
                    .iter()
                    .map(|(key, value)| {
                        let key = self.sanitize_text(key, stats);
                        let value = if is_sensitive_key(&key) && !value.is_null() {
                            stats.redacted_values = stats.redacted_values.saturating_add(1);
                            Value::String(REDACTION_MARKER.to_owned())
                        } else {
                            self.sanitize_json_at_depth(value, stats, depth + 1)
                        };
                        (key, value)
                    })
                    .collect(),
            ),
            _ => value.clone(),
        }
    }
}

fn sanitize_lifecycle(
    event: &LifecycleTransition,
    sanitizer: &TelemetrySanitizer,
    sanitization: &mut TelemetrySanitization,
) -> LifecycleTransition {
    let lifecycle_state = match &event.state {
        DriverLifecycleState::TurnRunning { turn_id } => DriverLifecycleState::TurnRunning {
            turn_id: sanitizer.sanitize_text(turn_id, sanitization),
        },
        current => current.clone(),
    };
    LifecycleTransition {
        sequence: event.sequence,
        at_unix_ms: event.at_unix_ms,
        state: lifecycle_state,
    }
}

fn archive_lifecycle(
    driver: &DriverTelemetry,
    sanitizer: &TelemetrySanitizer,
    sanitization: &mut TelemetrySanitization,
) -> Vec<LifecycleTransition> {
    let archived = driver
        .lifecycle
        .iter()
        .take(MAX_TELEMETRY_LIFECYCLE_EVENTS)
        .map(|event| sanitize_lifecycle(event, sanitizer, sanitization))
        .collect();
    if driver.lifecycle.len() > MAX_TELEMETRY_LIFECYCLE_EVENTS {
        sanitization.truncated_values = sanitization.truncated_values.saturating_add(1);
    }
    archived
}

fn archive_turns(
    driver: &DriverTelemetry,
    sanitizer: &TelemetrySanitizer,
    sanitization: &mut TelemetrySanitization,
) -> Vec<ArchivedTurnTelemetry> {
    let archived = driver
        .turns
        .iter()
        .take(MAX_TELEMETRY_TURNS)
        .enumerate()
        .map(|(sequence, turn)| {
            let tool_calls = turn
                .tool_calls
                .iter()
                .take(MAX_TELEMETRY_TOOL_CALLS_PER_TURN)
                .map(|call| sanitize_tool_call(call, sanitizer, sanitization))
                .collect();
            if turn.tool_calls.len() > MAX_TELEMETRY_TOOL_CALLS_PER_TURN {
                sanitization.truncated_values = sanitization.truncated_values.saturating_add(1);
            }
            ArchivedTurnTelemetry {
                sequence: sequence as u64,
                turn_id: sanitizer.sanitize_text(&turn.turn_id, sanitization),
                started_at_unix_ms: turn.started_at_unix_ms,
                completed_at_unix_ms: turn.completed_at_unix_ms,
                duration_ms: u64::try_from(turn.completed_at_unix_ms - turn.started_at_unix_ms)
                    .unwrap_or(u64::MAX),
                visible_input: sanitizer.sanitize_text(&turn.visible_input, sanitization),
                visible_output: sanitizer.sanitize_text(&turn.visible_output, sanitization),
                tool_calls,
            }
        })
        .collect();
    if driver.turns.len() > MAX_TELEMETRY_TURNS {
        sanitization.truncated_values = sanitization.truncated_values.saturating_add(1);
    }
    archived
}

fn archive_diagnostics(
    driver: &DriverTelemetry,
    sanitizer: &TelemetrySanitizer,
    sanitization: &mut TelemetrySanitization,
) -> Vec<ArchivedDiagnostic> {
    let archived = driver
        .diagnostics
        .iter()
        .take(MAX_TELEMETRY_DIAGNOSTICS)
        .map(|record| ArchivedDiagnostic {
            sequence: record.sequence,
            at_unix_ms: record.at_unix_ms,
            stream: record.stream,
            code: sanitizer.sanitize_text(&record.code, sanitization),
            visible_text: sanitizer.sanitize_text(&record.visible_text, sanitization),
        })
        .collect();
    if driver.diagnostics.len() > MAX_TELEMETRY_DIAGNOSTICS {
        sanitization.truncated_values = sanitization.truncated_values.saturating_add(1);
    }
    archived
}

fn sanitize_tool_call(
    call: &VisibleToolCall,
    sanitizer: &TelemetrySanitizer,
    stats: &mut TelemetrySanitization,
) -> ArchivedToolCall {
    ArchivedToolCall {
        tool: sanitizer.sanitize_text(&call.tool, stats),
        arguments: sanitizer.sanitize_json(&call.arguments, stats),
        result: sanitizer.sanitize_json(&call.result, stats),
    }
}

fn replace_bytes(
    value: &mut Vec<u8>,
    needle: &[u8],
    replacement: &[u8],
    stats: &mut TelemetrySanitization,
) {
    let mut offset = 0_usize;
    while offset.saturating_add(needle.len()) <= value.len() {
        if value[offset..].starts_with(needle) {
            value.splice(offset..offset + needle.len(), replacement.iter().copied());
            stats.redacted_values = stats.redacted_values.saturating_add(1);
            offset = offset.saturating_add(replacement.len());
        } else {
            offset = offset.saturating_add(1);
        }
    }
}

fn redact_prefixed_tokens(value: &mut Vec<u8>, prefix: &[u8], stats: &mut TelemetrySanitization) {
    let mut offset = 0_usize;
    while offset.saturating_add(prefix.len()) <= value.len() {
        if !value[offset..].starts_with(prefix) {
            offset = offset.saturating_add(1);
            continue;
        }
        let token_start = offset.saturating_add(prefix.len());
        let mut end = token_start;
        while end < value.len()
            && !matches!(
                value[end],
                b' ' | b'\t' | b'\r' | b'\n' | b'"' | b'\'' | b',' | b';' | b')' | b']' | b'}'
            )
        {
            end = end.saturating_add(1);
        }
        if end > token_start {
            value.splice(
                token_start..end,
                REDACTION_MARKER.as_bytes().iter().copied(),
            );
            stats.redacted_values = stats.redacted_values.saturating_add(1);
            offset = token_start.saturating_add(REDACTION_MARKER.len());
        } else {
            offset = token_start;
        }
    }
}

fn truncate_utf8(mut value: String, limit: usize, stats: &mut TelemetrySanitization) -> String {
    if value.len() <= limit {
        return value;
    }
    let content_limit = limit.saturating_sub(TRUNCATION_MARKER.len());
    let boundary = value
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= content_limit)
        .last()
        .unwrap_or_default();
    value.truncate(boundary);
    value.push_str(TRUNCATION_MARKER);
    stats.truncated_values = stats.truncated_values.saturating_add(1);
    value
}

fn is_sensitive_key(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "authorization"
            | "credential"
            | "password"
            | "secret"
            | "token"
            | "api_key"
            | "apikey"
            | "access_token"
            | "refresh_token"
    )
}

fn is_sanitized_text(value: &str) -> bool {
    value.len() <= MAX_TELEMETRY_TEXT_BYTES
        && !value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
}

fn is_sanitized_json(value: &Value, depth: usize) -> bool {
    if depth > 32 {
        return false;
    }
    if depth == 32 {
        return value.as_str() == Some(TRUNCATION_MARKER);
    }
    if serde_json::to_vec(value).map_or(true, |bytes| bytes.len() > MAX_TELEMETRY_JSON_BYTES) {
        return false;
    }
    match value {
        Value::String(value) => is_sanitized_text(value),
        Value::Array(values) => {
            values.len() <= 1_024
                && values
                    .iter()
                    .all(|value| is_sanitized_json(value, depth + 1))
        }
        Value::Object(values) => values.iter().all(|(key, value)| {
            is_sanitized_text(key)
                && if is_sensitive_key(key) && !value.is_null() {
                    value.as_str() == Some(REDACTION_MARKER)
                } else {
                    is_sanitized_json(value, depth + 1)
                }
        }),
        _ => true,
    }
}

fn validate_correlation(value: &RunTelemetryCorrelation) -> Result<(), RunTelemetryError> {
    validate_identifier(&value.game_id)?;
    validate_identifier(&value.run_id)?;
    if let Some(value) = &value.match_id {
        validate_identifier(value)?;
    }
    if let Some(value) = &value.tournament_id {
        validate_identifier(value)?;
    }
    if !matches!(value.seat_number, 1 | 2)
        || value.tournament_id.is_some() && value.match_id.is_none()
    {
        return Err(RunTelemetryError::Invalid);
    }
    Ok(())
}

fn validate_identity(value: &AgentManifestIdentity) -> Result<(), RunTelemetryError> {
    if value.manifest_sha256.len() != 64
        || !value
            .manifest_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || value.hash_algorithm.is_empty()
        || value.hash_algorithm.chars().any(char::is_control)
    {
        return Err(RunTelemetryError::Invalid);
    }
    Ok(())
}

fn validate_identifier(value: &str) -> Result<(), RunTelemetryError> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        Err(RunTelemetryError::Invalid)
    } else {
        Ok(())
    }
}

fn validate_label(value: &str) -> Result<(), RunTelemetryError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
    {
        Err(RunTelemetryError::Invalid)
    } else {
        Ok(())
    }
}
