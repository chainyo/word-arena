use std::{io, path::PathBuf, string::FromUtf8Error};

use thiserror::Error;

use crate::{compatibility::CompatibilityContext, manifest::PackIdentity};

/// Failures while discovering a locally installed immutable pack.
#[derive(Debug, Error)]
pub enum InstalledPackError {
    /// No installed identity exists beneath the selected pack family.
    #[error(
        "lexicon pack {pack_id:?} is not installed beneath {path}; run `cargo xtask setup` while online"
    )]
    NotInstalled {
        /// Required pack family.
        pack_id: String,
        /// Searched installation root.
        path: PathBuf,
    },

    /// More than one immutable identity exists and none may be selected silently.
    #[error(
        "lexicon pack {pack_id:?} has multiple installed identities beneath {path}; select an exact identity before starting the game"
    )]
    Ambiguous {
        /// Ambiguous pack family.
        pack_id: String,
        /// Pack-family installation root.
        path: PathBuf,
    },

    /// Directory segments disagree with the verified internal identity.
    #[error(
        "installed lexicon path {path} does not match its verified manifest identity {identity}"
    )]
    IdentityPathMismatch {
        /// Mismatched pack root.
        path: PathBuf,
        /// Verified internal identity.
        identity: Box<PackIdentity>,
    },

    /// An exact persisted/ruleset identity differs from the verified pack.
    #[error("installed lexicon must be {expected}, but {actual} was loaded")]
    ExactIdentityMismatch {
        /// Required identity.
        expected: Box<PackIdentity>,
        /// Verified installed identity.
        actual: Box<PackIdentity>,
    },

    /// The installed tree contains an unsupported entry.
    #[error("invalid installed lexicon layout at {path}: {reason}")]
    InvalidLayout {
        /// Invalid entry path.
        path: PathBuf,
        /// Required invariant.
        reason: &'static str,
    },

    /// A directory traversal failed.
    #[error("failed to inspect installed lexicons at {path}: {source}")]
    Io {
        /// Directory being inspected.
        path: PathBuf,
        /// Underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// A discovered pack failed complete integrity validation.
    #[error("installed lexicon at {path} is invalid: {source}")]
    InvalidPack {
        /// Invalid immutable pack root.
        path: PathBuf,
        /// Pack validation failure.
        #[source]
        source: Box<PackError>,
    },
}

/// Failures resolving platform-local Word Arena data and cache directories.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum DataPathError {
    /// A required platform environment variable is absent or empty.
    #[error(
        "cannot determine the Word Arena {kind} directory because {variable} is unset; set WORD_ARENA_DATA_DIR explicitly"
    )]
    MissingPlatformDirectory {
        /// Human-readable directory purpose.
        kind: &'static str,
        /// Missing platform variable.
        variable: &'static str,
    },

    /// The explicit override was present but empty.
    #[error("WORD_ARENA_DATA_DIR cannot be empty")]
    EmptyOverride,
}

/// Failures raised while parsing or validating an immutable lexicon pack.
#[derive(Debug, Error)]
pub enum PackError {
    /// The pack root does not contain its required manifest.
    #[error("missing pack manifest at {path}; expected {expected_name} at the pack root")]
    MissingManifest {
        /// Expected manifest path.
        path: PathBuf,
        /// Expected filename.
        expected_name: &'static str,
    },

    /// The manifest is not valid TOML or does not match the strict schema.
    #[error("invalid pack manifest at {path}: {source}")]
    ManifestSyntax {
        /// Manifest path.
        path: PathBuf,
        /// TOML decoding failure.
        #[source]
        source: toml::de::Error,
    },

    /// The pack format is newer, older, or otherwise unsupported.
    #[error(
        "unsupported pack format version {found}; this build supports exactly version {supported}"
    )]
    UnsupportedFormatVersion {
        /// Version declared by the pack.
        found: u32,
        /// Version implemented by this crate.
        supported: u32,
    },

    /// The normalization algorithm identifier is not implemented.
    #[error(
        "unsupported normalization algorithm {found:?}; expected {supported:?} for this runtime"
    )]
    UnsupportedNormalizationAlgorithm {
        /// Algorithm declared by the pack.
        found: String,
        /// Algorithm implemented by this crate.
        supported: &'static str,
    },

    /// The normalization algorithm version is not implemented.
    #[error(
        "unsupported normalization version {found}; this runtime supports exactly version {supported}"
    )]
    UnsupportedNormalizationVersion {
        /// Version declared by the pack.
        found: u32,
        /// Version implemented by this crate.
        supported: u32,
    },

    /// The named normalization profile is unknown to this runtime.
    #[error("unsupported normalization profile {profile:?} for locale {locale:?}")]
    UnsupportedNormalizationProfile {
        /// Locale declared by the pack.
        locale: String,
        /// Profile declared by the pack.
        profile: String,
    },

    /// A required manifest field has an invalid value.
    #[error("invalid manifest field {field}: {value:?}; {reason}")]
    InvalidManifestField {
        /// TOML field path.
        field: &'static str,
        /// Rejected value.
        value: String,
        /// Actionable validation rule.
        reason: &'static str,
    },

    /// A payload path occurs more than once in the manifest.
    #[error("manifest lists payload file {path:?} more than once")]
    DuplicateFileRecord {
        /// Duplicate canonical path.
        path: String,
    },

    /// A format-mandated payload is absent from the manifest.
    #[error("manifest does not list required payload file {path:?}")]
    MissingRequiredFileRecord {
        /// Missing canonical path.
        path: &'static str,
    },

    /// A manifest payload path could escape the pack root or vary by platform.
    #[error("unsafe or non-canonical payload path {path:?}: {reason}")]
    UnsafeFilePath {
        /// Rejected manifest path.
        path: String,
        /// Path safety rule.
        reason: &'static str,
    },

    /// A listed payload is absent on disk.
    #[error("manifest payload {relative_path:?} is missing at {path}")]
    MissingPayloadFile {
        /// Manifest-relative canonical path.
        relative_path: String,
        /// Resolved filesystem path.
        path: PathBuf,
    },

    /// A payload path is not a regular file.
    #[error(
        "pack entry {relative_path:?} at {path} must be a regular file, not a link or directory"
    )]
    PayloadNotRegularFile {
        /// Manifest-relative canonical path.
        relative_path: String,
        /// Resolved filesystem path.
        path: PathBuf,
    },

    /// A regular file exists in the pack but is not covered by its manifest.
    #[error("pack contains unlisted file {path:?}; add it to manifest files or remove it")]
    UnexpectedPayloadFile {
        /// Unlisted canonical relative path.
        path: String,
    },

    /// A filesystem path cannot be represented by the portable UTF-8 contract.
    #[error("pack contains a non-UTF-8 filesystem path at {path}")]
    NonUtf8Path {
        /// Path rejected during the recursive scan.
        path: PathBuf,
    },

    /// A payload byte count differs from the manifest.
    #[error(
        "size mismatch for {path:?}: manifest declares {expected} bytes but file contains {actual} bytes"
    )]
    FileSizeMismatch {
        /// Canonical payload path.
        path: String,
        /// Declared size.
        expected: u64,
        /// Observed size.
        actual: u64,
    },

    /// A payload SHA-256 differs from the manifest.
    #[error(
        "checksum mismatch for {path:?}: expected SHA-256 {expected}, calculated {actual}; reinstall or rebuild the pack"
    )]
    FileChecksumMismatch {
        /// Canonical payload path.
        path: String,
        /// Declared digest.
        expected: String,
        /// Observed digest.
        actual: String,
    },

    /// The deterministic checksum over every payload differs from the manifest.
    #[error(
        "pack content checksum mismatch: expected SHA-256 {expected}, calculated {actual}; the pack is incomplete or modified"
    )]
    ContentChecksumMismatch {
        /// Declared digest.
        expected: String,
        /// Observed digest.
        actual: String,
    },

    /// The checksummed runtime index is not a readable FST set.
    #[error("invalid runtime FST at {path}: {reason}")]
    InvalidIndex {
        /// Manifest-listed runtime index path.
        path: PathBuf,
        /// FST parser or structural diagnostic.
        reason: String,
    },

    /// One FST member is not a valid exact normalized key for this pack.
    #[error("invalid runtime FST key #{position} at {path}: {reason}")]
    InvalidIndexKey {
        /// Manifest-listed runtime index path.
        path: PathBuf,
        /// One-based key position in FST order.
        position: u64,
        /// UTF-8 or normalization diagnostic.
        reason: String,
    },

    /// The manifest count does not match complete FST enumeration.
    #[error(
        "runtime FST word-count mismatch: manifest declares {expected} keys but index contains {actual}"
    )]
    IndexWordCountMismatch {
        /// Manifest-declared key count.
        expected: u64,
        /// Fully enumerated key count.
        actual: u64,
    },

    /// A contextual filesystem operation failed.
    #[error("failed to access {path}: {source}")]
    Io {
        /// Path being accessed.
        path: PathBuf,
        /// Underlying I/O failure.
        #[source]
        source: io::Error,
    },
}

/// Failures raised while constructing a normalized exact-membership key.
#[derive(Debug, Error)]
pub enum NormalizedKeyError {
    /// Exact-membership keys cannot be empty.
    #[error("a normalized exact-membership key cannot be empty")]
    Empty,

    /// Raw FST bytes must decode as UTF-8.
    #[error("exact-membership key is not valid UTF-8: {0}")]
    InvalidUtf8(#[from] FromUtf8Error),

    /// Whitespace and control scalars are never valid normalized keys.
    #[error("exact-membership key contains forbidden character {character:?}")]
    ForbiddenCharacter {
        /// Rejected character.
        character: char,
    },

    /// The caller requested an unknown profile.
    #[error("normalization profile {profile:?} is not implemented")]
    UnsupportedProfile {
        /// Requested profile.
        profile: String,
    },

    /// A source form cannot be represented by the selected board profile.
    #[error("character {character:?} cannot be normalized by profile {profile:?}")]
    UnsupportedCharacter {
        /// Selected profile.
        profile: String,
        /// Rejected character.
        character: char,
    },
}

/// Failures raised when binding immutable pack identities to consumers or caches.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum CompatibilityError {
    /// Rulesets, replays, and active games require byte-identical pack content.
    #[error("{context} requires {expected}, but the available pack is {actual}")]
    ExactPackRequired {
        /// Consumer performing the check.
        context: CompatibilityContext,
        /// Pinned identity.
        expected: Box<PackIdentity>,
        /// Loaded identity.
        actual: Box<PackIdentity>,
    },

    /// Reusing a pack ID and version for different bytes is forbidden.
    #[error(
        "cache already contains {installed}; refusing different content for the same pack ID and version: {candidate}"
    )]
    ConflictingPackVersion {
        /// Existing immutable identity.
        installed: Box<PackIdentity>,
        /// Conflicting candidate identity.
        candidate: Box<PackIdentity>,
    },
}
