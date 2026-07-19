use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::DriverClock;

/// Version of privacy-safe agent authority-denial audit events.
pub const AUTHORITY_BOUNDARY_AUDIT_SCHEMA_VERSION: u32 = 1;

const CAPABILITY_PREFIX: &[u8] = b"wa_cap_v1.";
const CAPABILITY_ID_BYTES: usize = 32;
const CAPABILITY_SECRET_BYTES: usize = 64;
const CAPABILITY_WIRE_BYTES: usize =
    CAPABILITY_PREFIX.len() + CAPABILITY_ID_BYTES + 1 + CAPABILITY_SECRET_BYTES;

/// Human-only authority that must never enter an autonomous-agent boundary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ForbiddenAuthorityKind {
    HumanSpectator,
    Administrator,
}

/// Agent boundary where forbidden authority was found.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityBoundarySurface {
    ProcessArgument,
    ProcessEnvironment,
    WorkspaceFile,
}

/// Privacy-safe denial emitted before an agent process can start.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorityBoundaryAuditEvent {
    pub schema_version: u32,
    pub run_id: String,
    pub seat_id: String,
    pub authority: ForbiddenAuthorityKind,
    pub surface: AuthorityBoundarySurface,
    pub occurred_at_unix_ms: i64,
    pub outcome: AuthorityBoundaryOutcome,
}

/// Stable outcome for an authority-boundary audit.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityBoundaryOutcome {
    DeniedBeforeSpawn,
}

/// Audit persistence failure without secret-bearing diagnostics.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("agent authority audit sink is unavailable")]
pub struct AuthorityAuditError;

/// Synchronous, privacy-safe sink used before filesystem/process mutation.
pub trait AuthorityBoundaryAuditSink: fmt::Debug + Send + Sync {
    /// Records one denial. Failure must prevent startup.
    ///
    /// # Errors
    ///
    /// Returns [`AuthorityAuditError`] when the event cannot be durably queued
    /// or recorded.
    fn record(&self, event: AuthorityBoundaryAuditEvent) -> Result<(), AuthorityAuditError>;
}

/// A digest-only fingerprint for one human-only bearer capability.
///
/// The raw token is consumed during construction and never retained,
/// serialized, cloned, or exposed through debug output.
pub struct ForbiddenAuthorityFingerprint {
    authority: ForbiddenAuthorityKind,
    digest: [u8; 32],
}

impl fmt::Debug for ForbiddenAuthorityFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ForbiddenAuthorityFingerprint")
            .field("authority", &self.authority)
            .field("digest", &"<redacted>")
            .finish()
    }
}

impl ForbiddenAuthorityFingerprint {
    /// Consumes a privileged bearer and retains only its collision-resistant
    /// fingerprint.
    ///
    /// # Errors
    ///
    /// Rejects malformed or noncanonical capability wire values.
    pub fn new(
        authority: ForbiddenAuthorityKind,
        mut raw_capability: String,
    ) -> Result<Self, AuthorityPolicyError> {
        if !is_capability(raw_capability.as_bytes()) {
            raw_capability.clear();
            return Err(AuthorityPolicyError::InvalidCapability);
        }
        let digest = Sha256::digest(raw_capability.as_bytes()).into();
        raw_capability.clear();
        Ok(Self { authority, digest })
    }
}

/// Immutable digest-only registry of human spectator and administrator tokens.
#[derive(Clone, Default)]
pub struct ForbiddenAuthorityPolicy {
    fingerprints: BTreeMap<[u8; 32], ForbiddenAuthorityKind>,
}

impl fmt::Debug for ForbiddenAuthorityPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ForbiddenAuthorityPolicy")
            .field("fingerprint_count", &self.fingerprints.len())
            .finish()
    }
}

impl ForbiddenAuthorityPolicy {
    /// Builds a registry without retaining any raw credential.
    ///
    /// # Errors
    ///
    /// Rejects duplicate fingerprints, including a token assigned to two
    /// different authority classes.
    pub fn new(
        fingerprints: impl IntoIterator<Item = ForbiddenAuthorityFingerprint>,
    ) -> Result<Self, AuthorityPolicyError> {
        let mut values = BTreeMap::new();
        for fingerprint in fingerprints {
            if values
                .insert(fingerprint.digest, fingerprint.authority)
                .is_some()
            {
                return Err(AuthorityPolicyError::DuplicateFingerprint);
            }
        }
        Ok(Self {
            fingerprints: values,
        })
    }

    pub(crate) fn find(&self, bytes: &[u8]) -> Option<ForbiddenAuthorityKind> {
        bytes
            .windows(CAPABILITY_WIRE_BYTES)
            .filter(|candidate| is_capability(candidate))
            .find_map(|candidate| {
                let digest: [u8; 32] = Sha256::digest(candidate).into();
                self.fingerprints.get(&digest).copied()
            })
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fingerprints.is_empty()
    }
}

/// Invalid digest-only authority policy input.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum AuthorityPolicyError {
    #[error("forbidden authority capability is malformed")]
    InvalidCapability,
    #[error("forbidden authority capability fingerprint is duplicated")]
    DuplicateFingerprint,
}

pub(crate) fn audit_denial(
    sink: &dyn AuthorityBoundaryAuditSink,
    clock: &dyn DriverClock,
    run_id: &str,
    seat_id: &str,
    authority: ForbiddenAuthorityKind,
    surface: AuthorityBoundarySurface,
) -> Result<(), AuthorityAuditError> {
    sink.record(AuthorityBoundaryAuditEvent {
        schema_version: AUTHORITY_BOUNDARY_AUDIT_SCHEMA_VERSION,
        run_id: run_id.to_owned(),
        seat_id: seat_id.to_owned(),
        authority,
        surface,
        occurred_at_unix_ms: clock.now_unix_ms(),
        outcome: AuthorityBoundaryOutcome::DeniedBeforeSpawn,
    })
}

pub(crate) fn forbidden_authority_marker(value: &str) -> Option<ForbiddenAuthorityKind> {
    let normalized = value.to_ascii_lowercase().replace(['-', ' ', '.'], "_");
    if normalized.contains("human_spectator")
        || normalized.contains("observe_spectator")
        || normalized.contains("spectator_credential")
        || normalized.contains("spectator_capability")
    {
        Some(ForbiddenAuthorityKind::HumanSpectator)
    } else if normalized.contains("administrator")
        || normalized.contains("observe_admin")
        || normalized.contains("admin_credential")
        || normalized.contains("admin_capability")
    {
        Some(ForbiddenAuthorityKind::Administrator)
    } else {
        None
    }
}

pub(crate) fn contains_capability_wire(bytes: &[u8]) -> bool {
    bytes.windows(CAPABILITY_WIRE_BYTES).any(is_capability)
}

fn is_capability(candidate: &[u8]) -> bool {
    candidate.len() == CAPABILITY_WIRE_BYTES
        && candidate.starts_with(CAPABILITY_PREFIX)
        && candidate[CAPABILITY_PREFIX.len() + CAPABILITY_ID_BYTES] == b'.'
        && candidate[CAPABILITY_PREFIX.len()..CAPABILITY_PREFIX.len() + CAPABILITY_ID_BYTES]
            .iter()
            .chain(&candidate[CAPABILITY_PREFIX.len() + CAPABILITY_ID_BYTES + 1..])
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}
