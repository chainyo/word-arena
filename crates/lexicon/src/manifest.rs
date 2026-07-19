use std::{collections::BTreeSet, fmt};

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::{
    CURRENT_FORMAT_VERSION, ENGLISH_NORMALIZATION_PROFILE, FRENCH_NORMALIZATION_PROFILE,
    GERMAN_NORMALIZATION_PROFILE, MANIFEST_FILE, NORMALIZATION_ALGORITHM, NORMALIZATION_VERSION,
    PackError, REQUIRED_PAYLOAD_FILES, SPANISH_NORMALIZATION_PROFILE,
};

/// Strict, versioned metadata stored in `manifest.toml` at each pack root.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackManifest {
    /// Pack contract version.
    pub format_version: u32,
    /// Stable pack family identifier.
    pub pack_id: String,
    /// Semantic version of this pack release.
    pub pack_version: String,
    /// Lowercase BCP 47 language tag supported by format V1.
    pub locale: String,
    /// Number of normalized exact-membership keys in `lexicon.fst`.
    pub word_count: u64,
    /// Deterministic SHA-256 over all listed payload paths and bytes.
    pub content_sha256: String,
    /// Independently versioned board-key normalization contract.
    pub normalization: NormalizationDescriptor,
    /// Upstream data provenance and license.
    pub source: SourceDescriptor,
    /// Filtering and curation policy version.
    pub policy: PolicyDescriptor,
    /// Reproducible builder identity.
    pub builder: BuilderDescriptor,
    /// Every regular payload file, including required and optional artifacts.
    pub files: Vec<FileDescriptor>,
}

/// Board-key normalization identity, independent from source and builder versions.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizationDescriptor {
    /// Stable algorithm identifier.
    pub algorithm: String,
    /// Algorithm contract version.
    pub version: u32,
    /// Locale-specific profile implemented by that algorithm version.
    pub profile: String,
}

/// Provenance fields that bind a pack to one pinned source archive.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceDescriptor {
    /// Source registry ID from `lexicons/sources.toml`.
    pub id: String,
    /// Full immutable source revision or release version.
    pub revision: String,
    /// SHA-256 of the exact upstream archive.
    pub archive_sha256: String,
    /// SPDX identifier or project `LicenseRef-*` value.
    pub license_id: String,
}

/// Versioned import, exclusion, and curation policy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDescriptor {
    /// Stable policy identifier.
    pub id: String,
    /// Policy contract version, independent from the pack version.
    pub version: u32,
}

/// Tool and version used to reproduce the pack.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuilderDescriptor {
    /// Stable builder name.
    pub name: String,
    /// Semantic builder version.
    pub version: String,
}

/// Integrity metadata for one pack payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileDescriptor {
    /// Portable UTF-8 path relative to the pack root, using `/` separators.
    pub path: String,
    /// Exact file length in bytes.
    pub size_bytes: u64,
    /// SHA-256 over the exact file bytes.
    pub sha256: String,
}

/// Immutable identity persisted by rulesets, games, and replays.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackIdentity {
    /// Stable pack family identifier.
    pub pack_id: String,
    /// Semantic pack release version.
    pub pack_version: String,
    /// Pack contract version.
    pub format_version: u32,
    /// Exact language tag.
    pub locale: String,
    /// Exact normalization identity.
    pub normalization: NormalizationDescriptor,
    /// Deterministic payload checksum.
    pub content_sha256: String,
}

impl PackManifest {
    /// Validates schema semantics before any payload bytes are trusted.
    ///
    /// # Errors
    ///
    /// Returns an actionable [`PackError`] for unsupported versions, malformed
    /// identifiers or digests, unsafe paths, duplicate paths, and absent
    /// format-mandated file records.
    pub fn validate_schema(&self) -> Result<(), PackError> {
        if self.format_version != CURRENT_FORMAT_VERSION {
            return Err(PackError::UnsupportedFormatVersion {
                found: self.format_version,
                supported: CURRENT_FORMAT_VERSION,
            });
        }

        validate_slug("pack_id", &self.pack_id)?;
        validate_semver("pack_version", &self.pack_version)?;
        validate_normalization(&self.locale, &self.normalization)?;
        validate_source_id(&self.source.id)?;
        validate_nonempty("source.revision", &self.source.revision)?;
        validate_sha256("source.archive_sha256", &self.source.archive_sha256)?;
        validate_nonempty("source.license_id", &self.source.license_id)?;
        validate_slug("policy.id", &self.policy.id)?;
        if self.policy.version == 0 {
            return Err(PackError::InvalidManifestField {
                field: "policy.version",
                value: self.policy.version.to_string(),
                reason: "version must be greater than zero",
            });
        }
        validate_slug("builder.name", &self.builder.name)?;
        validate_semver("builder.version", &self.builder.version)?;
        validate_sha256("content_sha256", &self.content_sha256)?;

        let mut paths = BTreeSet::new();
        for file in &self.files {
            validate_file_path(&file.path)?;
            validate_sha256("files.sha256", &file.sha256)?;
            if !paths.insert(file.path.as_str()) {
                return Err(PackError::DuplicateFileRecord {
                    path: file.path.clone(),
                });
            }
        }

        for required in REQUIRED_PAYLOAD_FILES {
            if !paths.contains(required) {
                return Err(PackError::MissingRequiredFileRecord { path: required });
            }
        }

        Ok(())
    }

    /// Returns the immutable reference consumers must persist and compare.
    #[must_use]
    pub fn identity(&self) -> PackIdentity {
        PackIdentity {
            pack_id: self.pack_id.clone(),
            pack_version: self.pack_version.clone(),
            format_version: self.format_version,
            locale: self.locale.clone(),
            normalization: self.normalization.clone(),
            content_sha256: self.content_sha256.clone(),
        }
    }
}

impl fmt::Display for PackIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}@{} [format {}, locale {}, normalization {}@{}/{}, sha256 {}]",
            self.pack_id,
            self.pack_version,
            self.format_version,
            self.locale,
            self.normalization.algorithm,
            self.normalization.version,
            self.normalization.profile,
            self.content_sha256
        )
    }
}

pub(crate) fn validate_file_path(path: &str) -> Result<(), PackError> {
    if path == MANIFEST_FILE {
        return Err(PackError::UnsafeFilePath {
            path: path.to_owned(),
            reason: "manifest.toml cannot list or checksum itself",
        });
    }
    if path.is_empty() {
        return Err(PackError::UnsafeFilePath {
            path: path.to_owned(),
            reason: "path cannot be empty",
        });
    }
    if path.starts_with('/') || path.contains('\\') {
        return Err(PackError::UnsafeFilePath {
            path: path.to_owned(),
            reason: "use a relative path with canonical `/` separators",
        });
    }
    if path
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(PackError::UnsafeFilePath {
            path: path.to_owned(),
            reason: "empty, `.` and `..` components are forbidden",
        });
    }
    if path.chars().any(char::is_control) {
        return Err(PackError::UnsafeFilePath {
            path: path.to_owned(),
            reason: "control characters are forbidden",
        });
    }
    Ok(())
}

fn validate_normalization(
    locale: &str,
    normalization: &NormalizationDescriptor,
) -> Result<(), PackError> {
    if normalization.algorithm != NORMALIZATION_ALGORITHM {
        return Err(PackError::UnsupportedNormalizationAlgorithm {
            found: normalization.algorithm.clone(),
            supported: NORMALIZATION_ALGORITHM,
        });
    }
    if normalization.version != NORMALIZATION_VERSION {
        return Err(PackError::UnsupportedNormalizationVersion {
            found: normalization.version,
            supported: NORMALIZATION_VERSION,
        });
    }

    let supported = matches!(
        (locale, normalization.profile.as_str()),
        ("en", ENGLISH_NORMALIZATION_PROFILE)
            | ("fr", FRENCH_NORMALIZATION_PROFILE)
            | ("de", GERMAN_NORMALIZATION_PROFILE)
            | ("es", SPANISH_NORMALIZATION_PROFILE)
    );
    if !supported {
        return Err(PackError::UnsupportedNormalizationProfile {
            locale: locale.to_owned(),
            profile: normalization.profile.clone(),
        });
    }
    Ok(())
}

fn validate_slug(field: &'static str, value: &str) -> Result<(), PackError> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
        && !value.contains("--");
    if valid {
        Ok(())
    } else {
        Err(PackError::InvalidManifestField {
            field,
            value: value.to_owned(),
            reason: "use 1-128 lowercase ASCII letters, digits, or single hyphens, beginning and ending with a letter or digit",
        })
    }
}

fn validate_source_id(value: &str) -> Result<(), PackError> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'.'
        })
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
        && !value.contains("--")
        && !value.contains("..")
        && !value.contains("-.")
        && !value.contains(".-");
    if valid {
        Ok(())
    } else {
        Err(PackError::InvalidManifestField {
            field: "source.id",
            value: value.to_owned(),
            reason: "use the exact lowercase source registry ID with ASCII letters, digits, dots, or single hyphens",
        })
    }
}

fn validate_semver(field: &'static str, value: &str) -> Result<(), PackError> {
    Version::parse(value)
        .map(|_| ())
        .map_err(|_| PackError::InvalidManifestField {
            field,
            value: value.to_owned(),
            reason: "use a valid Semantic Version such as `1.0.0`",
        })
}

fn validate_nonempty(field: &'static str, value: &str) -> Result<(), PackError> {
    if !value.is_empty() && !value.chars().any(char::is_control) {
        Ok(())
    } else {
        Err(PackError::InvalidManifestField {
            field,
            value: value.to_owned(),
            reason: "value must be non-empty and contain no control characters",
        })
    }
}

fn validate_sha256(field: &'static str, value: &str) -> Result<(), PackError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(PackError::InvalidManifestField {
            field,
            value: value.to_owned(),
            reason: "use exactly 64 lowercase hexadecimal SHA-256 characters",
        })
    }
}
