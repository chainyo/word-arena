use std::{collections::BTreeSet, fs, path::Path};

use semver::Version;
use serde::{Deserialize, Serialize};
use word_arena_lexicon::{
    CURRENT_FORMAT_VERSION, ENGLISH_NORMALIZATION_PROFILE, FRENCH_NORMALIZATION_PROFILE,
    NORMALIZATION_ALGORITHM, NORMALIZATION_VERSION, NormalizationDescriptor, PackIdentity,
};

use crate::XtaskError;

const REGISTRY_SCHEMA_VERSION: u32 = 1;

/// Strict committed registry of separately released lexicon artifacts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackRegistry {
    /// Registry schema contract.
    pub schema_version: u32,
    /// Required English and French release records.
    pub packs: Vec<PackRecord>,
}

/// One immutable downloadable pack archive and expected internal identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackRecord {
    /// Stable pack family ID.
    pub pack_id: String,
    /// Independent semantic pack version.
    pub pack_version: String,
    /// Pack format version.
    pub format_version: u32,
    /// Exact locale.
    pub locale: String,
    /// Normalization algorithm.
    pub normalization_algorithm: String,
    /// Normalization version.
    pub normalization_version: u32,
    /// Locale normalization profile.
    pub normalization_profile: String,
    /// Pack payload identity.
    pub content_sha256: String,
    /// HTTPS release artifact URL (loopback HTTP is allowed only for tests).
    pub artifact_url: String,
    /// Exact compressed artifact size.
    pub artifact_size_bytes: u64,
    /// Exact compressed artifact digest.
    pub artifact_sha256: String,
    /// License ID expected inside the pack manifest.
    pub license_id: String,
}

impl PackRegistry {
    /// Reads and strictly validates a registry file.
    ///
    /// # Errors
    ///
    /// Returns [`XtaskError`] for I/O, TOML, schema, identity, URL, digest, or
    /// duplicate-record failures.
    pub fn load(path: &Path) -> Result<Self, XtaskError> {
        let value = fs::read_to_string(path).map_err(|source| XtaskError::RegistryRead {
            path: path.to_path_buf(),
            source,
        })?;
        let registry: Self =
            toml::from_str(&value).map_err(|source| XtaskError::RegistrySyntax {
                path: path.to_path_buf(),
                source,
            })?;
        registry.validate()?;
        Ok(registry)
    }

    /// Finds one pinned pack record.
    ///
    /// # Errors
    ///
    /// Returns [`XtaskError::UnknownPack`] when the ID is absent.
    pub fn require(&self, pack_id: &str) -> Result<&PackRecord, XtaskError> {
        self.packs
            .iter()
            .find(|record| record.pack_id == pack_id)
            .ok_or_else(|| XtaskError::UnknownPack {
                pack_id: pack_id.to_owned(),
            })
    }

    fn validate(&self) -> Result<(), XtaskError> {
        if self.schema_version != REGISTRY_SCHEMA_VERSION {
            return invalid(
                "schema_version",
                self.schema_version.to_string(),
                "only registry schema version 1 is supported",
            );
        }
        let mut identities = BTreeSet::new();
        let mut pack_ids = BTreeSet::new();
        for record in &self.packs {
            record.validate()?;
            if !identities.insert((record.pack_id.as_str(), record.pack_version.as_str())) {
                return invalid(
                    "packs",
                    format!("{}@{}", record.pack_id, record.pack_version),
                    "pack ID and version pairs must be unique",
                );
            }
            if !pack_ids.insert(record.pack_id.as_str()) {
                return invalid(
                    "packs.pack_id",
                    record.pack_id.clone(),
                    "V1 registry contains exactly one pinned release per pack ID",
                );
            }
        }
        for required in ["word-arena-en-world-v1", "word-arena-fr-v1"] {
            if !pack_ids.contains(required) {
                return invalid(
                    "packs.pack_id",
                    required.to_owned(),
                    "the English and French V1 packs are both required",
                );
            }
        }
        Ok(())
    }
}

impl PackRecord {
    /// Complete identity expected after archive extraction.
    #[must_use]
    pub fn identity(&self) -> PackIdentity {
        PackIdentity {
            pack_id: self.pack_id.clone(),
            pack_version: self.pack_version.clone(),
            format_version: self.format_version,
            locale: self.locale.clone(),
            normalization: NormalizationDescriptor {
                algorithm: self.normalization_algorithm.clone(),
                version: self.normalization_version,
                profile: self.normalization_profile.clone(),
            },
            content_sha256: self.content_sha256.clone(),
        }
    }

    fn validate(&self) -> Result<(), XtaskError> {
        let expected_profile = match self.locale.as_str() {
            "en" => ENGLISH_NORMALIZATION_PROFILE,
            "fr" => FRENCH_NORMALIZATION_PROFILE,
            _ => {
                return invalid(
                    "packs.locale",
                    self.locale.clone(),
                    "V1 registry supports only en and fr",
                );
            }
        };
        require_slug("packs.pack_id", &self.pack_id)?;
        Version::parse(&self.pack_version).map_err(|_| XtaskError::InvalidRegistry {
            field: "packs.pack_version",
            value: self.pack_version.clone(),
            reason: "use Semantic Versioning",
        })?;
        require_equal(
            "packs.format_version",
            self.format_version,
            CURRENT_FORMAT_VERSION,
        )?;
        require_equal_text(
            "packs.normalization_algorithm",
            &self.normalization_algorithm,
            NORMALIZATION_ALGORITHM,
        )?;
        require_equal(
            "packs.normalization_version",
            self.normalization_version,
            NORMALIZATION_VERSION,
        )?;
        require_equal_text(
            "packs.normalization_profile",
            &self.normalization_profile,
            expected_profile,
        )?;
        require_sha256("packs.content_sha256", &self.content_sha256)?;
        require_sha256("packs.artifact_sha256", &self.artifact_sha256)?;
        if self.artifact_size_bytes == 0 {
            return invalid(
                "packs.artifact_size_bytes",
                "0".to_owned(),
                "artifact size must be greater than zero",
            );
        }
        if !valid_artifact_url(&self.artifact_url) {
            return invalid(
                "packs.artifact_url",
                self.artifact_url.clone(),
                "use HTTPS; loopback HTTP is accepted only for local integration tests",
            );
        }
        require_text("packs.license_id", &self.license_id)
    }
}

fn valid_artifact_url(url: &str) -> bool {
    url.starts_with("https://")
        || url.starts_with("http://127.0.0.1:")
        || url.starts_with("http://[::1]:")
        || url.starts_with("http://localhost:")
}

fn require_text(field: &'static str, value: &str) -> Result<(), XtaskError> {
    if !value.is_empty() && value == value.trim() && !value.chars().any(char::is_control) {
        Ok(())
    } else {
        invalid(
            field,
            value.to_owned(),
            "value must be nonempty trimmed single-line text",
        )
    }
}

fn require_slug(field: &'static str, value: &str) -> Result<(), XtaskError> {
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
        invalid(
            field,
            value.to_owned(),
            "use a portable lowercase ASCII slug",
        )
    }
}

fn require_sha256(field: &'static str, value: &str) -> Result<(), XtaskError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        invalid(
            field,
            value.to_owned(),
            "use exactly 64 lowercase hexadecimal SHA-256 characters",
        )
    }
}

fn require_equal(field: &'static str, actual: u32, expected: u32) -> Result<(), XtaskError> {
    if actual == expected {
        Ok(())
    } else {
        invalid(
            field,
            actual.to_string(),
            "value is unsupported by this application build",
        )
    }
}

fn require_equal_text(field: &'static str, actual: &str, expected: &str) -> Result<(), XtaskError> {
    if actual == expected {
        Ok(())
    } else {
        invalid(
            field,
            actual.to_owned(),
            "value is unsupported for this locale and application build",
        )
    }
}

fn invalid<T>(field: &'static str, value: String, reason: &'static str) -> Result<T, XtaskError> {
    Err(XtaskError::InvalidRegistry {
        field,
        value,
        reason,
    })
}
