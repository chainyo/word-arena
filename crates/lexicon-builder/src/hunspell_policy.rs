use std::{collections::BTreeSet, fs, path::Path};

use serde::{Deserialize, Serialize};
use word_arena_lexicon::{GERMAN_NORMALIZATION_PROFILE, SPANISH_NORMALIZATION_PROFILE};

use crate::BuilderError;

const POLICY_SCHEMA_VERSION: u32 = 1;
const ALPHABET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// Executable policy shared by the German and Spanish Hunspell importers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HunspellPolicy {
    /// Policy schema version.
    pub schema_version: u32,
    /// Stable filter-policy ID.
    pub id: String,
    /// Independent filter-policy version.
    pub version: u32,
    /// Pack ID produced by this policy.
    pub pack_id: String,
    /// Lowercase language identifier.
    pub locale: String,
    /// Source registry ID.
    pub source_id: String,
    /// Full immutable source revision.
    pub source_revision: String,
    /// Pinned source archive SHA-256.
    pub source_archive_sha256: String,
    /// Pinned source archive byte length.
    pub source_archive_size_bytes: u64,
    /// Expected top-level directory inside the archive.
    pub source_archive_root: String,
    /// Relative Hunspell dictionary member path below the archive root.
    pub source_dictionary_path: String,
    /// Relative Hunspell affix member path below the archive root.
    pub source_affix_path: String,
    /// Source text encoding.
    pub source_encoding: String,
    /// Independently versioned runtime normalization profile.
    pub normalization_profile: String,
    /// Allowed normalized board tokens.
    pub alphabet: String,
    /// Minimum normalized key length.
    pub min_word_length: usize,
    /// Maximum normalized key length.
    pub max_word_length: usize,
    /// Portable review record relative to `lexicons/`.
    pub review_file: String,
    /// Human-readable forms intended for admission after review.
    pub included_forms: Vec<String>,
    /// Human-readable forms intended for exclusion after review.
    pub excluded_forms: Vec<String>,
    /// Mandatory review gate that must be resolved by release tooling.
    pub review_requirement: ReviewRequirement,
}

/// Native-language review conditions attached to a Hunspell policy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRequirement {
    /// Required state before importing the pinned upstream data.
    pub status: String,
    /// Required reviewer qualification.
    pub qualification: String,
    /// Policy and fixture scope requiring review.
    pub scope: String,
    /// Required evidence fields.
    pub evidence_required: Vec<String>,
}

impl HunspellPolicy {
    /// Loads and validates one strict Hunspell policy.
    ///
    /// # Errors
    ///
    /// Returns [`BuilderError`] for I/O, TOML syntax, unknown fields, or an
    /// unsupported policy value.
    pub fn load(path: &Path) -> Result<Self, BuilderError> {
        let encoded = fs::read_to_string(path).map_err(|source| BuilderError::PolicyRead {
            path: path.to_path_buf(),
            source,
        })?;
        let policy: Self =
            toml::from_str(&encoded).map_err(|source| BuilderError::PolicySyntax {
                path: path.to_path_buf(),
                source,
            })?;
        policy.validate()?;
        Ok(policy)
    }

    /// Validates every immutable source, normalization, and review-gate field.
    ///
    /// # Errors
    ///
    /// Returns [`BuilderError::InvalidPolicy`] when the policy is ambiguous,
    /// unsafe, or inconsistent with the selected language.
    pub fn validate(&self) -> Result<(), BuilderError> {
        self.validate_source_identity()?;
        self.validate_board_contract()?;
        self.validate_review_contract()
    }

    fn validate_source_identity(&self) -> Result<(), BuilderError> {
        require(
            self.schema_version == POLICY_SCHEMA_VERSION,
            "schema_version",
            self.schema_version.to_string(),
            "only policy schema version 1 is supported",
        )?;
        for (field, value) in [
            ("id", self.id.as_str()),
            ("pack_id", self.pack_id.as_str()),
            ("source_id", self.source_id.as_str()),
            ("source_revision", self.source_revision.as_str()),
            ("source_archive_root", self.source_archive_root.as_str()),
        ] {
            require_text(field, value)?;
        }
        require(
            self.version > 0,
            "version",
            self.version.to_string(),
            "version must be greater than zero",
        )?;
        require_sha256("source_archive_sha256", &self.source_archive_sha256)?;
        require(
            self.source_archive_size_bytes > 0,
            "source_archive_size_bytes",
            self.source_archive_size_bytes.to_string(),
            "archive size must be greater than zero",
        )?;
        require(
            self.source_encoding == "utf-8",
            "source_encoding",
            self.source_encoding.clone(),
            "the selected normalized Hunspell snapshots are UTF-8",
        )?;
        validate_member(
            "source_dictionary_path",
            &self.source_dictionary_path,
            "dic",
        )?;
        validate_member("source_affix_path", &self.source_affix_path, "aff")?;
        Ok(())
    }

    fn validate_board_contract(&self) -> Result<(), BuilderError> {
        let expected_profile = match self.locale.as_str() {
            "de" => GERMAN_NORMALIZATION_PROFILE,
            "es" => SPANISH_NORMALIZATION_PROFILE,
            _ => {
                return invalid("locale", &self.locale, "Hunspell V1 supports only de or es");
            }
        };
        require(
            self.pack_id == format!("word-arena-{}-v1", self.locale),
            "pack_id",
            self.pack_id.clone(),
            "pack ID must identify the selected V1 locale",
        )?;
        require(
            self.normalization_profile == expected_profile,
            "normalization_profile",
            self.normalization_profile.clone(),
            "profile must match the selected locale",
        )?;
        require(
            self.alphabet == ALPHABET,
            "alphabet",
            self.alphabet.clone(),
            "V1 uses exactly the 26 uppercase basic Latin board tokens",
        )?;
        require(
            self.min_word_length > 0 && self.min_word_length <= self.max_word_length,
            "min_word_length",
            self.min_word_length.to_string(),
            "minimum must be positive and no larger than the maximum",
        )?;
        require(
            self.max_word_length <= 15,
            "max_word_length",
            self.max_word_length.to_string(),
            "maximum cannot exceed the 15-square board dimension",
        )?;
        Ok(())
    }

    fn validate_review_contract(&self) -> Result<(), BuilderError> {
        validate_portable_review_path(&self.review_file)?;
        validate_unique_text("included_forms", &self.included_forms)?;
        validate_unique_text("excluded_forms", &self.excluded_forms)?;
        require(
            self.review_requirement.status == "required-before-import",
            "review_requirement.status",
            self.review_requirement.status.clone(),
            "pinned source data remains gated until native-language review",
        )?;
        require_text(
            "review_requirement.qualification",
            &self.review_requirement.qualification,
        )?;
        require_text("review_requirement.scope", &self.review_requirement.scope)?;
        let expected = [
            "reviewer identity",
            "review date",
            "decision",
            "rationale",
            "source-policy linkage",
        ];
        require(
            self.review_requirement.evidence_required == expected.map(str::to_owned),
            "review_requirement.evidence_required",
            format!("{:?}", self.review_requirement.evidence_required),
            "all native-review evidence fields are mandatory in stable order",
        )?;
        Ok(())
    }
}

fn validate_member(field: &'static str, value: &str, extension: &str) -> Result<(), BuilderError> {
    require_text(field, value)?;
    let safe = !value.starts_with('/')
        && !value.contains('\\')
        && value
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
        && Path::new(value)
            .extension()
            .is_some_and(|actual| actual.eq_ignore_ascii_case(extension));
    require(
        safe,
        field,
        value.to_owned(),
        "use a safe relative archive member with the required extension",
    )
}

fn validate_portable_review_path(value: &str) -> Result<(), BuilderError> {
    let safe = value.starts_with("reviews/")
        && value
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..");
    require(
        safe,
        "review_file",
        value.to_owned(),
        "use a safe path below lexicons/reviews",
    )
}

fn validate_unique_text(field: &'static str, values: &[String]) -> Result<(), BuilderError> {
    let unique = values.iter().map(String::as_str).collect::<BTreeSet<_>>();
    require(
        !values.is_empty()
            && unique.len() == values.len()
            && values.iter().all(|value| !value.trim().is_empty()),
        field,
        format!("{values:?}"),
        "provide unique non-empty policy statements",
    )
}

fn require_text(field: &'static str, value: &str) -> Result<(), BuilderError> {
    require(
        !value.trim().is_empty() && !value.chars().any(char::is_control),
        field,
        value.to_owned(),
        "value must be non-empty and contain no control characters",
    )
}

fn require_sha256(field: &'static str, value: &str) -> Result<(), BuilderError> {
    require(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        field,
        value.to_owned(),
        "use exactly 64 lowercase hexadecimal SHA-256 characters",
    )
}

fn require(
    condition: bool,
    field: &'static str,
    value: String,
    reason: &'static str,
) -> Result<(), BuilderError> {
    if condition {
        Ok(())
    } else {
        Err(BuilderError::InvalidPolicy {
            field,
            value,
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
