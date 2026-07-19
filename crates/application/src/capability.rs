use std::{collections::BTreeSet, fmt};

use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;
use word_arena_engine::Seat;

use crate::{
    AdministratorCredential, CompetitiveSeatCredential, GameId, HumanSpectatorCredential,
    PublicViewerCredential, RepositoryError, UnixMillis,
};

/// Current keyed capability-digest contract.
pub const CAPABILITY_DIGEST_VERSION: u16 = 1;
const TOKEN_PREFIX: &str = "wa_cap_v1";
const CAPABILITY_ID_BYTES: usize = 16;
const CAPABILITY_SECRET_BYTES: usize = 32;
const DIGEST_BYTES: usize = 32;
const MAX_AGENT_RUN_ID_BYTES: usize = 128;

/// Stable public identifier for one capability record.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct CapabilityId(String);

impl CapabilityId {
    /// Validates the canonical lowercase-hex identifier.
    ///
    /// # Errors
    ///
    /// Rejects values that are not exactly 128 bits of lowercase hex.
    pub fn new(value: impl Into<String>) -> Result<Self, CapabilityError> {
        let value = value.into();
        if value.len() == CAPABILITY_ID_BYTES * 2
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            Ok(Self(value))
        } else {
            Err(CapabilityError::InvalidRequest)
        }
    }

    /// Canonical public identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<CapabilityId> for String {
    fn from(value: CapabilityId) -> Self {
        value.0
    }
}

impl TryFrom<String> for CapabilityId {
    type Error = CapabilityError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

/// Optional stable link from a seat capability to one agent run.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct AgentRunId(String);

impl AgentRunId {
    /// Validates a bounded printable identifier.
    ///
    /// # Errors
    ///
    /// Rejects empty, oversized, whitespace, control, or non-ASCII values.
    pub fn new(value: impl Into<String>) -> Result<Self, CapabilityError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_AGENT_RUN_ID_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_graphic() && !byte.is_ascii_whitespace())
        {
            Err(CapabilityError::InvalidRequest)
        } else {
            Ok(Self(value))
        }
    }

    /// Canonical identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<AgentRunId> for String {
    fn from(value: AgentRunId) -> Self {
        value.0
    }
}

impl TryFrom<String> for AgentRunId {
    type Error = CapabilityError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

/// Application role permanently bound to a capability.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "seat")]
pub enum CapabilityRole {
    /// Public-only observer.
    Public,
    /// One fixed competitive seat.
    Seat(Seat),
    /// Trusted human-only both-rack observer.
    HumanSpectator,
    /// Trusted authoritative operator.
    Administrator,
}

/// Independently grantable application capability scope.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityScope {
    /// Read the public game projection.
    ObservePublic,
    /// Read the bound competitive seat projection.
    ObserveSeat,
    /// Submit actions for the bound competitive seat.
    Act,
    /// Read the human-only both-rack projection.
    ObserveHumanSpectator,
    /// Read the authoritative administrator projection.
    ObserveAdministrator,
}

/// Serializable, secret-free capability metadata.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityDescriptor {
    /// Public capability identifier.
    pub capability_id: CapabilityId,
    /// Bound game.
    pub game_id: GameId,
    /// Bound role and optional seat.
    pub role: CapabilityRole,
    /// Sorted unique granted scopes.
    pub scopes: BTreeSet<CapabilityScope>,
    /// Issuance time.
    pub issued_at: UnixMillis,
    /// Exclusive expiry time.
    pub expires_at: UnixMillis,
    /// Optional competitive agent-run binding.
    pub agent_run_id: Option<AgentRunId>,
}

/// Complete persisted capability state; it contains a digest, never a token.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityRecord {
    /// Secret-free metadata.
    pub descriptor: CapabilityDescriptor,
    /// Versioned keyed digest.
    pub token_digest: [u8; DIGEST_BYTES],
    /// Digest contract version.
    pub digest_version: u16,
    /// Immediate revocation time, when set.
    pub revoked_at: Option<UnixMillis>,
}

/// Raw token returned exactly once from issuance or rotation.
///
/// This type is deliberately neither `Clone` nor serializable. Transport code
/// must explicitly consume it with [`Self::into_secret`] for the one response
/// that delivers it to its owner.
pub struct CapabilityToken(String);

impl CapabilityToken {
    pub(crate) fn from_material(material: &CapabilityMaterial) -> Self {
        Self(format!(
            "{TOKEN_PREFIX}.{}.{}",
            encode_hex(&material.capability_id),
            encode_hex(&material.secret)
        ))
    }

    pub(crate) fn parse(value: &str) -> Result<CapabilityId, CapabilityError> {
        let mut parts = value.split('.');
        let prefix = parts.next();
        let identifier = parts.next();
        let secret = parts.next();
        if prefix != Some(TOKEN_PREFIX)
            || parts.next().is_some()
            || secret.is_none_or(|part| !is_lower_hex(part, CAPABILITY_SECRET_BYTES * 2))
        {
            return Err(CapabilityError::Unauthorized);
        }
        CapabilityId::new(identifier.unwrap_or_default()).map_err(|_| CapabilityError::Unauthorized)
    }

    /// Exposes the raw bearer secret to its one issuance response.
    #[must_use]
    pub fn into_secret(self) -> String {
        self.0
    }

    pub(crate) fn secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CapabilityToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CapabilityToken([REDACTED])")
    }
}

/// One capability issuance result containing the one-time raw token.
#[derive(Debug)]
pub struct IssuedCapability {
    /// Secret-free record metadata.
    pub descriptor: CapabilityDescriptor,
    /// One-time bearer token.
    pub token: CapabilityToken,
}

/// Operator issuance request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueCapabilityRequest {
    /// Bound game.
    pub game_id: GameId,
    /// Exact role and optional seat.
    pub role: CapabilityRole,
    /// Requested scopes.
    pub scopes: BTreeSet<CapabilityScope>,
    /// Exclusive expiry time.
    pub expires_at: UnixMillis,
    /// Optional agent-run binding; valid for seat roles only.
    pub agent_run_id: Option<AgentRunId>,
}

/// Fresh opaque material supplied by an injected source.
#[derive(Eq, PartialEq)]
pub struct CapabilityMaterial {
    capability_id: [u8; CAPABILITY_ID_BYTES],
    secret: [u8; CAPABILITY_SECRET_BYTES],
}

impl CapabilityMaterial {
    /// Constructs fresh material from an injected secure source.
    #[must_use]
    pub const fn new(
        capability_id: [u8; CAPABILITY_ID_BYTES],
        secret: [u8; CAPABILITY_SECRET_BYTES],
    ) -> Self {
        Self {
            capability_id,
            secret,
        }
    }

    pub(crate) const fn capability_id(&self) -> &[u8; CAPABILITY_ID_BYTES] {
        &self.capability_id
    }
}

impl fmt::Debug for CapabilityMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CapabilityMaterial([REDACTED])")
    }
}

/// Injected source for deterministic tests and OS-backed production entropy.
pub trait CapabilityTokenSource: fmt::Debug + Send + Sync {
    /// Returns fresh identifier and bearer-secret bytes.
    ///
    /// # Errors
    ///
    /// Fails closed when secure entropy is unavailable.
    fn next_material(&self) -> Result<CapabilityMaterial, CapabilityError>;
}

/// Operating-system entropy source for production capability issuance.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemCapabilityTokenSource;

impl CapabilityTokenSource for SystemCapabilityTokenSource {
    fn next_material(&self) -> Result<CapabilityMaterial, CapabilityError> {
        let mut capability_id = [0_u8; CAPABILITY_ID_BYTES];
        let mut secret = [0_u8; CAPABILITY_SECRET_BYTES];
        getrandom::fill(&mut capability_id).map_err(|_| CapabilityError::EntropyUnavailable)?;
        getrandom::fill(&mut secret).map_err(|_| CapabilityError::EntropyUnavailable)?;
        Ok(CapabilityMaterial::new(capability_id, secret))
    }
}

/// Secret server-side HMAC key; debug output is always redacted.
#[derive(Clone)]
pub struct CapabilityDigestKey([u8; DIGEST_BYTES]);

impl CapabilityDigestKey {
    /// Constructs an injected 256-bit key.
    #[must_use]
    pub const fn new(bytes: [u8; DIGEST_BYTES]) -> Self {
        Self(bytes)
    }

    pub(crate) fn digest(&self, token: &str) -> [u8; DIGEST_BYTES] {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.0).expect("HMAC accepts keys of every length");
        mac.update(token.as_bytes());
        mac.finalize().into_bytes().into()
    }

    pub(crate) fn verifies(&self, token: &str, expected: &[u8; DIGEST_BYTES]) -> bool {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.0).expect("HMAC accepts keys of every length");
        mac.update(token.as_bytes());
        mac.verify_slice(expected).is_ok()
    }
}

impl fmt::Debug for CapabilityDigestKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CapabilityDigestKey([REDACTED])")
    }
}

/// Credential produced only after complete capability authentication.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthenticatedCredential {
    /// Public projection credential.
    Public(PublicViewerCredential),
    /// One competitive seat credential.
    Seat(CompetitiveSeatCredential),
    /// Human-only spectator credential.
    HumanSpectator(HumanSpectatorCredential),
    /// Administrator credential.
    Administrator(AdministratorCredential),
}

impl AuthenticatedCredential {
    /// Bound game identifier shared by every authenticated role.
    #[must_use]
    pub const fn game_id(&self) -> &GameId {
        match self {
            Self::Public(credential) => credential.game_id(),
            Self::Seat(credential) => credential.game_id(),
            Self::HumanSpectator(credential) => credential.game_id(),
            Self::Administrator(credential) => credential.game_id(),
        }
    }

    /// Derives the public-view credential after a public scope was verified.
    #[must_use]
    pub fn public_viewer(&self) -> PublicViewerCredential {
        PublicViewerCredential::new(self.game_id())
    }
}

/// Privacy-safe actor stored in the audit log.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "seat")]
pub enum AuditActor {
    /// Unauthenticated/system path.
    System,
    /// Authenticated public viewer.
    Public,
    /// Authenticated seat.
    Seat(Seat),
    /// Trusted human spectator.
    HumanSpectator,
    /// Trusted administrator.
    Administrator,
}

/// Audited capability operation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    /// Issue a capability.
    Issue,
    /// Authenticate a capability.
    Authenticate,
    /// Revoke a capability.
    Revoke,
    /// Replace a capability atomically.
    Rotate,
    /// Read a privileged spectator or administrator projection.
    PrivilegedAccess,
}

/// Privacy-safe audit outcome with no token or game payload.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    /// Operation succeeded.
    Success,
    /// Input token did not have the canonical shape.
    DeniedMalformed,
    /// No matching record or digest was found.
    DeniedUnknown,
    /// Capability expired.
    DeniedExpired,
    /// Capability was revoked.
    DeniedRevoked,
    /// Capability belongs to another game.
    DeniedGame,
    /// Capability does not grant the requested scope.
    DeniedScope,
}

/// One structured audit row guaranteed not to contain bearer secrets or racks.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuditRecord {
    /// Game when safely known.
    pub game_id: Option<GameId>,
    /// Authenticated actor, or system for denied/issuance paths.
    pub actor: AuditActor,
    /// Audited operation.
    pub action: AuditAction,
    /// Stable privacy-safe outcome.
    pub outcome: AuditOutcome,
    /// Public capability identifier when canonical and known.
    pub capability_id: Option<CapabilityId>,
    /// Requested scope, if authentication-related.
    pub scope: Option<CapabilityScope>,
    /// Audit timestamp.
    pub occurred_at: UnixMillis,
}

/// Stable capability repository failures.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CapabilityRepositoryError {
    /// Capability ID does not exist.
    #[error("capability not found")]
    NotFound,
    /// Capability ID or digest already exists.
    #[error("capability already exists")]
    AlreadyExists,
    /// Stored capability bytes or relationships are invalid.
    #[error("stored capability is corrupt")]
    Corrupt,
    /// Concurrent update invalidated the requested operation.
    #[error("capability update conflict")]
    Conflict,
    /// Adapter cannot currently complete the operation.
    #[error("capability repository is unavailable")]
    Unavailable,
}

/// Public capability failure; authentication details remain deliberately terse.
#[derive(Debug, Error)]
pub enum CapabilityError {
    /// Issuance or rotation request violates the role/scope/time contract.
    #[error("invalid capability request")]
    InvalidRequest,
    /// Secure operating-system entropy is unavailable.
    #[error("secure capability entropy is unavailable")]
    EntropyUnavailable,
    /// Authentication failed closed without revealing why.
    #[error("capability is unauthorized")]
    Unauthorized,
    /// The referenced game cannot be used.
    #[error(transparent)]
    Game(#[from] RepositoryError),
    /// Capability persistence or auditing failed.
    #[error(transparent)]
    Repository(#[from] CapabilityRepositoryError),
}

pub(crate) fn validate_issue(
    request: &IssueCapabilityRequest,
    now: UnixMillis,
) -> Result<(), CapabilityError> {
    if request.expires_at <= now || request.scopes.is_empty() {
        return Err(CapabilityError::InvalidRequest);
    }
    if request.agent_run_id.is_some() && !matches!(request.role, CapabilityRole::Seat(_)) {
        return Err(CapabilityError::InvalidRequest);
    }
    if request
        .scopes
        .iter()
        .any(|scope| !role_allows(request.role, *scope))
    {
        return Err(CapabilityError::InvalidRequest);
    }
    Ok(())
}

pub(crate) const fn role_allows(role: CapabilityRole, scope: CapabilityScope) -> bool {
    match role {
        CapabilityRole::Public => matches!(scope, CapabilityScope::ObservePublic),
        CapabilityRole::Seat(_) => matches!(
            scope,
            CapabilityScope::ObservePublic | CapabilityScope::ObserveSeat | CapabilityScope::Act
        ),
        CapabilityRole::HumanSpectator => matches!(
            scope,
            CapabilityScope::ObservePublic | CapabilityScope::ObserveHumanSpectator
        ),
        CapabilityRole::Administrator => matches!(
            scope,
            CapabilityScope::ObservePublic | CapabilityScope::ObserveAdministrator
        ),
    }
}

pub(crate) fn credential(record: &CapabilityRecord) -> AuthenticatedCredential {
    let game_id = &record.descriptor.game_id;
    match record.descriptor.role {
        CapabilityRole::Public => {
            AuthenticatedCredential::Public(PublicViewerCredential::new(game_id))
        }
        CapabilityRole::Seat(seat) => {
            AuthenticatedCredential::Seat(CompetitiveSeatCredential::new(game_id, seat))
        }
        CapabilityRole::HumanSpectator => {
            AuthenticatedCredential::HumanSpectator(HumanSpectatorCredential::new(game_id))
        }
        CapabilityRole::Administrator => {
            AuthenticatedCredential::Administrator(AdministratorCredential::new(game_id))
        }
    }
}

pub(crate) const fn actor(role: CapabilityRole) -> AuditActor {
    match role {
        CapabilityRole::Public => AuditActor::Public,
        CapabilityRole::Seat(seat) => AuditActor::Seat(seat),
        CapabilityRole::HumanSpectator => AuditActor::HumanSpectator,
        CapabilityRole::Administrator => AuditActor::Administrator,
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn is_lower_hex(value: &str, expected_len: usize) -> bool {
    value.len() == expected_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
