use std::fmt;

use crate::{CompatibilityError, PackIdentity};

/// Consumer whose immutable pack pin is being checked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompatibilityContext {
    /// Starting a game from a versioned ruleset.
    Ruleset,
    /// Reconstructing a previously recorded game.
    Replay,
    /// Continuing a game that has already started.
    ActiveGame,
}

impl fmt::Display for CompatibilityContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Ruleset => "ruleset",
            Self::Replay => "replay",
            Self::ActiveGame => "active game",
        })
    }
}

/// Safe action for an immutable local pack cache.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheDecision {
    /// The exact immutable identity is already installed.
    AlreadyInstalled,
    /// Install as a distinct entry without replacing any existing version.
    InstallAlongside,
}

/// Requires every identity field to match for rulesets, replays, or active games.
///
/// This intentionally forbids compatible-looking substitutions and hot swaps.
/// A new pack release can be selected only when creating a new ruleset/game pin.
///
/// # Errors
///
/// Returns [`CompatibilityError::ExactPackRequired`] with both identities when
/// any immutable field differs.
pub fn ensure_exact_pack(
    context: CompatibilityContext,
    expected: &PackIdentity,
    actual: &PackIdentity,
) -> Result<(), CompatibilityError> {
    if expected == actual {
        Ok(())
    } else {
        Err(CompatibilityError::ExactPackRequired {
            context,
            expected: Box::new(expected.clone()),
            actual: Box::new(actual.clone()),
        })
    }
}

/// Plans an immutable cache installation.
///
/// Exact identities are idempotent. New versions install side by side. Reusing
/// the same pack ID and semantic version for different metadata or bytes is a
/// hard conflict so an active game or replay can never observe changed content.
///
/// # Errors
///
/// Returns [`CompatibilityError::ConflictingPackVersion`] when an installed
/// entry has the same `pack_id` and `pack_version` but a different identity.
pub fn plan_cache_install(
    installed: &[PackIdentity],
    candidate: &PackIdentity,
) -> Result<CacheDecision, CompatibilityError> {
    if installed.iter().any(|identity| identity == candidate) {
        return Ok(CacheDecision::AlreadyInstalled);
    }

    if let Some(conflict) = installed.iter().find(|identity| {
        identity.pack_id == candidate.pack_id && identity.pack_version == candidate.pack_version
    }) {
        return Err(CompatibilityError::ConflictingPackVersion {
            installed: Box::new(conflict.clone()),
            candidate: Box::new(candidate.clone()),
        });
    }

    Ok(CacheDecision::InstallAlongside)
}
