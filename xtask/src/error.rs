use std::{io, path::PathBuf, process::ExitStatus};

use thiserror::Error;
use word_arena_lexicon::{DataPathError, PackError};
use word_arena_lexicon_builder::{BuilderError, CurationError};

/// Actionable failures from local setup and lexicon management.
#[derive(Debug, Error)]
pub enum XtaskError {
    /// Command-line input does not match the supported grammar.
    #[error("{0}")]
    Usage(String),

    /// Registry file cannot be read.
    #[error("failed to read pack registry at {path}: {source}")]
    RegistryRead {
        /// Registry path.
        path: PathBuf,
        /// Underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// Registry TOML is malformed or has unknown fields.
    #[error("invalid pack registry TOML at {path}: {source}")]
    RegistrySyntax {
        /// Registry path.
        path: PathBuf,
        /// TOML decode failure.
        #[source]
        source: toml::de::Error,
    },

    /// Registry content violates the pinned schema.
    #[error("invalid pack registry field {field}: {value:?}; {reason}")]
    InvalidRegistry {
        /// Field path.
        field: &'static str,
        /// Rejected value.
        value: String,
        /// Required invariant.
        reason: &'static str,
    },

    /// Requested registry pack ID does not exist.
    #[error("pack {pack_id:?} is not present in the configured registry")]
    UnknownPack {
        /// Requested ID.
        pack_id: String,
    },

    /// Platform data/cache paths cannot be resolved.
    #[error(transparent)]
    DataPath(#[from] DataPathError),

    /// A contextual filesystem operation failed.
    #[error("failed to access {path}: {source}")]
    Io {
        /// Affected path.
        path: PathBuf,
        /// Underlying failure.
        #[source]
        source: io::Error,
    },

    /// A required local executable is unavailable.
    #[error("required tool {tool:?} is unavailable; {recovery}")]
    MissingTool {
        /// Executable name.
        tool: &'static str,
        /// Installation guidance.
        recovery: &'static str,
    },

    /// A required child command failed.
    #[error("command {command:?} failed with {status}: {stderr}")]
    ToolFailed {
        /// Human-readable command.
        command: String,
        /// Process status.
        status: ExitStatus,
        /// Captured stderr.
        stderr: String,
    },

    /// A remote artifact could not be downloaded completely.
    #[error(
        "failed to download {url} ({status}): {stderr}; the existing installation was not changed—retry when the network is available, or use --offline with a verified cache"
    )]
    DownloadFailed {
        /// Requested immutable artifact URL.
        url: String,
        /// Curl exit status.
        status: ExitStatus,
        /// Curl diagnostic.
        stderr: String,
    },

    /// Offline mode has no verified cached archive or installed pack.
    #[error(
        "pack {pack_id} is not installed and no verified archive is cached at {cache_path}; rerun without --offline when a network is available"
    )]
    OfflineArtifactUnavailable {
        /// Required pack.
        pack_id: String,
        /// Expected content-addressed cache path.
        cache_path: PathBuf,
    },

    /// Downloaded archive length differs from the registry.
    #[error(
        "artifact size mismatch for {pack_id}: registry declares {expected} bytes but download contains {actual} bytes"
    )]
    ArtifactSizeMismatch {
        /// Pack being installed.
        pack_id: String,
        /// Registry byte length.
        expected: u64,
        /// Downloaded length.
        actual: u64,
    },

    /// Downloaded archive checksum differs from the registry.
    #[error(
        "artifact checksum mismatch for {pack_id}: expected SHA-256 {expected}, calculated {actual}; the existing installation was not changed"
    )]
    ArtifactChecksumMismatch {
        /// Pack being installed.
        pack_id: String,
        /// Registry digest.
        expected: String,
        /// Download digest.
        actual: String,
    },

    /// Archive framing or entry type is unsafe or unsupported.
    #[error("invalid artifact archive for {pack_id} at {path}: {reason}")]
    InvalidArchive {
        /// Pack being installed.
        pack_id: String,
        /// Archive path.
        path: PathBuf,
        /// Rejection detail.
        reason: String,
    },

    /// A required compliance payload is missing or empty.
    #[error("pack {pack_id} has invalid compliance file {path:?}: {reason}")]
    InvalidComplianceFile {
        /// Pack ID.
        pack_id: String,
        /// Required relative path.
        path: &'static str,
        /// Rejection detail.
        reason: &'static str,
    },

    /// The archive contains a valid pack other than the registry identity.
    #[error("registry expects {expected}, but artifact contains {actual}")]
    ArtifactIdentityMismatch {
        /// Committed registry identity.
        expected: Box<word_arena_lexicon::PackIdentity>,
        /// Loaded artifact identity.
        actual: Box<word_arena_lexicon::PackIdentity>,
    },

    /// Artifact and manifest licenses disagree.
    #[error(
        "pack {pack_id} registry expects license {expected:?}, but manifest declares {actual:?}"
    )]
    LicenseMismatch {
        /// Pack ID.
        pack_id: String,
        /// Registry license ID.
        expected: String,
        /// Manifest license ID.
        actual: String,
    },

    /// A remove command cannot find the selected installation.
    #[error("pack {pack_id} is not installed at {path}")]
    NotInstalled {
        /// Pack ID.
        pack_id: String,
        /// Expected immutable path.
        path: PathBuf,
    },

    /// Pinned source registry does not contain the builder policy source ID.
    #[error("source {source_id:?} is not present in lexicons/sources.toml")]
    UnknownSource {
        /// Missing source ID.
        source_id: String,
    },

    /// A source-build stage disagrees with another versioned input.
    #[error("source-build contract mismatch: {message}")]
    BuildContract {
        /// Cross-input mismatch.
        message: String,
    },

    /// A release artifact output already exists and will not be overwritten.
    #[error("release artifact output already exists at {path}")]
    ArtifactOutputExists {
        /// Existing output path.
        path: PathBuf,
    },

    /// Rebuilt pack or archive identity differs from the committed registry.
    #[error(
        "rebuilt artifact for {pack_id} does not match registry: expected content {expected_content_sha256}, archive {expected_archive_sha256} ({expected_size_bytes} bytes); built content {actual_content_sha256}, archive {actual_archive_sha256} ({actual_size_bytes} bytes)"
    )]
    RegistryArtifactMismatch {
        /// Pack ID.
        pack_id: Box<str>,
        /// Registry content identity.
        expected_content_sha256: Box<str>,
        /// Registry archive digest.
        expected_archive_sha256: Box<str>,
        /// Registry archive length.
        expected_size_bytes: u64,
        /// Built content identity.
        actual_content_sha256: Box<str>,
        /// Built archive digest.
        actual_archive_sha256: Box<str>,
        /// Built archive length.
        actual_size_bytes: u64,
    },

    /// A generated manifest or source registry cannot be serialized.
    #[error("failed to serialize deterministic TOML: {0}")]
    Toml(#[from] toml::ser::Error),

    /// Complete pack validation failed.
    #[error(transparent)]
    Pack(#[from] PackError),

    /// Reproducible source/index building failed.
    #[error(transparent)]
    Builder(#[from] BuilderError),

    /// Reviewed curation application failed.
    #[error(transparent)]
    Curation(#[from] CurationError),
}
