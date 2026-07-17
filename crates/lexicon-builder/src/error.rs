use std::{io, path::PathBuf, process::ExitStatus};

use thiserror::Error;
use word_arena_lexicon::NormalizedKeyError;

/// Failures produced by source preparation and deterministic lexicon builds.
#[derive(Debug, Error)]
pub enum BuilderError {
    /// A policy file cannot be read.
    #[error("failed to read English build policy at {path}: {source}")]
    PolicyRead {
        /// Policy path.
        path: PathBuf,
        /// Underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// A policy file is not valid TOML or has an unknown field.
    #[error("invalid English build policy at {path}: {source}")]
    PolicySyntax {
        /// Policy path.
        path: PathBuf,
        /// TOML decode failure.
        #[source]
        source: toml::de::Error,
    },

    /// A typed policy value violates the supported contract.
    #[error("invalid English build policy field {field}: {value:?}; {reason}")]
    InvalidPolicy {
        /// Field name.
        field: &'static str,
        /// Rejected value.
        value: String,
        /// Required rule.
        reason: &'static str,
    },

    /// The pinned source archive has the wrong byte length.
    #[error(
        "SCOWLv1 archive size mismatch at {path}: expected {expected} bytes, found {actual} bytes"
    )]
    ArchiveSizeMismatch {
        /// Archive path.
        path: PathBuf,
        /// Policy value.
        expected: u64,
        /// Observed size.
        actual: u64,
    },

    /// The pinned source archive has the wrong digest.
    #[error(
        "SCOWLv1 archive checksum mismatch at {path}: expected SHA-256 {expected}, calculated {actual}"
    )]
    ArchiveChecksumMismatch {
        /// Archive path.
        path: PathBuf,
        /// Policy digest.
        expected: String,
        /// Observed digest.
        actual: String,
    },

    /// Archive extraction did not produce the pinned root.
    #[error("SCOWLv1 archive did not contain expected source root {expected:?} under {directory}")]
    MissingArchiveRoot {
        /// Extraction directory.
        directory: PathBuf,
        /// Pinned root name.
        expected: String,
    },

    /// A required upstream build tool is absent.
    #[error("SCOWLv1 source build requires executable {tool:?} on PATH; {recovery}")]
    MissingTool {
        /// Command name.
        tool: &'static str,
        /// Installation/recovery hint.
        recovery: &'static str,
    },

    /// The upstream V1 scripts require a Unix shell environment.
    #[error("SCOWLv1 source generation is supported only on Unix-like hosts")]
    UnsupportedSourceBuildPlatform,

    /// An upstream command returned a failure status.
    #[error("upstream SCOWLv1 command {command:?} failed with {status}: {stderr}")]
    UpstreamBuildFailed {
        /// Human-readable command.
        command: String,
        /// Process status.
        status: ExitStatus,
        /// Captured diagnostic tail.
        stderr: String,
    },

    /// The generated SCOWL directory is absent.
    #[error("SCOWLv1 final directory is missing at {path}; run source preparation first")]
    MissingFinalDirectory {
        /// Expected directory.
        path: PathBuf,
    },

    /// A final-directory file does not match the documented SCOWL naming contract.
    #[error("unrecognized SCOWLv1 final input file {path}; refusing to leave its rows unaccounted")]
    UnexpectedInputFile {
        /// Unknown file path.
        path: PathBuf,
    },

    /// A generated output destination already exists.
    #[error("build output already exists at {path}; choose a new empty destination")]
    OutputExists {
        /// Existing path.
        path: PathBuf,
    },

    /// English normalization rejected a candidate.
    #[error(transparent)]
    Normalization(#[from] NormalizedKeyError),

    /// JSON audit serialization failed.
    #[error("failed to serialize deterministic audit JSON: {0}")]
    Json(#[from] serde_json::Error),

    /// TOML output serialization failed.
    #[error("failed to serialize deterministic build TOML: {0}")]
    Toml(#[from] toml::ser::Error),

    /// A contextual filesystem operation failed.
    #[error("failed to access {path}: {source}")]
    Io {
        /// Path being accessed.
        path: PathBuf,
        /// Underlying failure.
        #[source]
        source: io::Error,
    },

    /// An internal accounting invariant failed before output publication.
    #[error("English build accounting invariant failed: {message}")]
    AccountingInvariant {
        /// Invariant details.
        message: String,
    },
}
