use std::{collections::BTreeSet, fs, path::Path};

use serde::{Deserialize, Serialize};
use word_arena_lexicon::ENGLISH_NORMALIZATION_PROFILE;

use crate::BuilderError;

const POLICY_SCHEMA_VERSION: u32 = 1;
const SUPPORTED_SCOWL_LEVELS: [u8; 9] = [10, 20, 35, 40, 50, 55, 60, 70, 80];
const KNOWN_SPELLING_CATEGORIES: [&str; 15] = [
    "english",
    "american",
    "british",
    "british_z",
    "canadian",
    "australian",
    "variant_1",
    "variant_2",
    "variant_3",
    "british_variant_1",
    "british_variant_2",
    "canadian_variant_1",
    "canadian_variant_2",
    "australian_variant_1",
    "australian_variant_2",
];

/// Executable English V1 source-selection and board-filter policy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnglishPolicy {
    /// Policy schema version.
    pub schema_version: u32,
    /// Stable filter-policy ID.
    pub id: String,
    /// Independent filter-policy version.
    pub version: u32,
    /// Pack ID produced by this policy.
    pub pack_id: String,
    /// Source registry ID.
    pub source_id: String,
    /// Full pinned source revision.
    pub source_revision: String,
    /// Pinned archive SHA-256.
    pub source_archive_sha256: String,
    /// Pinned archive byte length.
    pub source_archive_size_bytes: u64,
    /// Expected top-level directory inside the archive.
    pub source_archive_root: String,
    /// Input encoding emitted by `SCOWLv1` V1.
    pub source_encoding: String,
    /// Inclusive SCOWL size boundary.
    pub max_scowl_level: u8,
    /// Only this source subcategory may produce candidates.
    pub source_subcategory: String,
    /// Explicitly included SCOWL spelling categories.
    pub spelling_categories: Vec<String>,
    /// Independently versioned runtime normalization profile.
    pub normalization_profile: String,
    /// Allowed normalized board tokens.
    pub alphabet: String,
    /// Minimum normalized key length.
    pub min_word_length: usize,
    /// Maximum normalized key length.
    pub max_word_length: usize,
}

impl EnglishPolicy {
    /// Reads and validates a strict policy TOML file.
    ///
    /// # Errors
    ///
    /// Returns [`BuilderError`] for I/O, unknown/missing TOML fields, or an
    /// unsupported policy value.
    pub fn load(path: &Path) -> Result<Self, BuilderError> {
        let value = fs::read_to_string(path).map_err(|source| BuilderError::PolicyRead {
            path: path.to_path_buf(),
            source,
        })?;
        let policy: Self = toml::from_str(&value).map_err(|source| BuilderError::PolicySyntax {
            path: path.to_path_buf(),
            source,
        })?;
        policy.validate()?;
        Ok(policy)
    }

    /// Validates policy invariants without reading source data.
    ///
    /// # Errors
    ///
    /// Returns [`BuilderError::InvalidPolicy`] when a field could make source
    /// selection ambiguous or incompatible with English normalization V1.
    pub fn validate(&self) -> Result<(), BuilderError> {
        require_equal(
            "schema_version",
            &self.schema_version,
            &POLICY_SCHEMA_VERSION,
            "only policy schema version 1 is supported",
        )?;
        require_nonempty("id", &self.id)?;
        if self.version == 0 {
            return invalid(
                "version",
                &self.version,
                "version must be greater than zero",
            );
        }
        require_nonempty("pack_id", &self.pack_id)?;
        require_nonempty("source_id", &self.source_id)?;
        require_nonempty("source_revision", &self.source_revision)?;
        require_sha256("source_archive_sha256", &self.source_archive_sha256)?;
        if self.source_archive_size_bytes == 0 {
            return invalid(
                "source_archive_size_bytes",
                &self.source_archive_size_bytes,
                "archive size must be greater than zero",
            );
        }
        require_nonempty("source_archive_root", &self.source_archive_root)?;
        require_string_equal(
            "source_encoding",
            &self.source_encoding,
            "iso-8859-1",
            "SCOWLv1 V1 final lists are decoded as ISO-8859-1",
        )?;
        if !SUPPORTED_SCOWL_LEVELS.contains(&self.max_scowl_level) {
            return invalid(
                "max_scowl_level",
                &self.max_scowl_level,
                "use one of 10, 20, 35, 40, 50, 55, 60, 70, or 80",
            );
        }
        require_string_equal(
            "source_subcategory",
            &self.source_subcategory,
            "words",
            "English V1 imports only SCOWL normal-word files",
        )?;
        require_string_equal(
            "normalization_profile",
            &self.normalization_profile,
            ENGLISH_NORMALIZATION_PROFILE,
            "English V1 requires the pinned board-key profile",
        )?;
        require_string_equal(
            "alphabet",
            &self.alphabet,
            "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
            "English V1 uses exactly the 26 uppercase basic Latin board tokens",
        )?;
        if self.min_word_length == 0 || self.min_word_length > self.max_word_length {
            return invalid(
                "min_word_length",
                &self.min_word_length,
                "minimum must be greater than zero and no larger than the maximum",
            );
        }
        if self.max_word_length > 15 {
            return invalid(
                "max_word_length",
                &self.max_word_length,
                "maximum cannot exceed the 15-square board dimension",
            );
        }

        let mut categories = BTreeSet::new();
        for category in &self.spelling_categories {
            if !KNOWN_SPELLING_CATEGORIES.contains(&category.as_str()) {
                return Err(BuilderError::InvalidPolicy {
                    field: "spelling_categories",
                    value: category.clone(),
                    reason: "category is not defined by the pinned SCOWLv1 naming contract",
                });
            }
            if !categories.insert(category.as_str()) {
                return Err(BuilderError::InvalidPolicy {
                    field: "spelling_categories",
                    value: category.clone(),
                    reason: "categories must be unique",
                });
            }
        }
        if !categories.contains("english") {
            return Err(BuilderError::InvalidPolicy {
                field: "spelling_categories",
                value: format!("{:?}", self.spelling_categories),
                reason: "the common `english` category is required",
            });
        }
        Ok(())
    }

    pub(crate) fn selects_spelling_category(&self, category: &str) -> bool {
        self.spelling_categories
            .iter()
            .any(|selected| selected == category)
    }
}

fn require_nonempty(field: &'static str, value: &str) -> Result<(), BuilderError> {
    if value.is_empty() || value.chars().any(char::is_control) {
        Err(BuilderError::InvalidPolicy {
            field,
            value: value.to_owned(),
            reason: "value must be non-empty and contain no control characters",
        })
    } else {
        Ok(())
    }
}

fn require_sha256(field: &'static str, value: &str) -> Result<(), BuilderError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(BuilderError::InvalidPolicy {
            field,
            value: value.to_owned(),
            reason: "use exactly 64 lowercase hexadecimal SHA-256 characters",
        })
    }
}

fn require_equal<T>(
    field: &'static str,
    actual: &T,
    expected: &T,
    reason: &'static str,
) -> Result<(), BuilderError>
where
    T: Eq + ToString,
{
    if actual == expected {
        Ok(())
    } else {
        invalid(field, actual, reason)
    }
}

fn require_string_equal(
    field: &'static str,
    actual: &str,
    expected: &str,
    reason: &'static str,
) -> Result<(), BuilderError> {
    if actual == expected {
        Ok(())
    } else {
        Err(BuilderError::InvalidPolicy {
            field,
            value: actual.to_owned(),
            reason,
        })
    }
}

fn invalid<T>(field: &'static str, value: &T, reason: &'static str) -> Result<(), BuilderError>
where
    T: ToString,
{
    Err(BuilderError::InvalidPolicy {
        field,
        value: value.to_string(),
        reason,
    })
}
