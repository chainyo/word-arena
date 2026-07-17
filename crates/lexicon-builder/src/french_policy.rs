use std::{collections::BTreeSet, fs, path::Path};

use serde::{Deserialize, Serialize};
use word_arena_lexicon::FRENCH_NORMALIZATION_PROFILE;

use crate::BuilderError;

const POLICY_SCHEMA_VERSION: u32 = 1;
const REQUIRED_ACCEPTED_CATEGORIES: [&str; 10] = [
    "adjective",
    "adverb",
    "commonNoun",
    "conjunction",
    "determiner",
    "interjection",
    "numeral",
    "preposition",
    "pronoun",
    "verb",
];
const REQUIRED_PROPER_NAME_CATEGORIES: [&str; 2] = ["properName", "properNoun"];

/// Executable Morphalou 3.1 source-selection and French board-filter policy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FrenchPolicy {
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
    /// Full pinned source revision/version.
    pub source_revision: String,
    /// Pinned archive SHA-256.
    pub source_archive_sha256: String,
    /// Pinned archive byte length.
    pub source_archive_size_bytes: u64,
    /// Exact XML member path inside the ZIP archive.
    pub source_archive_data_path: String,
    /// Exact uncompressed XML byte length.
    pub source_data_size_bytes: u64,
    /// Declared source encoding.
    pub source_encoding: String,
    /// Grammatical categories accepted as standard lexical entries.
    pub accepted_grammatical_categories: Vec<String>,
    /// Defensive category names treated as proper names.
    pub proper_name_categories: Vec<String>,
    /// Morphalou subcategory used for abbreviations.
    pub abbreviation_subcategory: String,
    /// Morphalou value indicating that an entry is a locution.
    pub locution_value: String,
    /// A spelling variant supported only by these origins is nonstandard V1 data.
    pub excluded_variant_only_origins: Vec<String>,
    /// Independently versioned runtime normalization profile.
    pub normalization_profile: String,
    /// Allowed normalized board tokens.
    pub alphabet: String,
    /// Minimum normalized key length.
    pub min_word_length: usize,
    /// Maximum normalized key length.
    pub max_word_length: usize,
}

impl FrenchPolicy {
    /// Reads and validates a strict French policy TOML file.
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

    /// Validates every fixed French V1 source and board invariant.
    ///
    /// # Errors
    ///
    /// Returns [`BuilderError::InvalidPolicy`] when a field would make the
    /// source selection ambiguous or incompatible with French normalization V1.
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
        validate_zip_member_path(&self.source_archive_data_path)?;
        if self.source_data_size_bytes == 0 {
            return invalid(
                "source_data_size_bytes",
                &self.source_data_size_bytes,
                "XML size must be greater than zero",
            );
        }
        require_string_equal(
            "source_encoding",
            &self.source_encoding,
            "utf-8",
            "Morphalou 3.1 LMF data is decoded as UTF-8",
        )?;
        require_exact_set(
            "accepted_grammatical_categories",
            &self.accepted_grammatical_categories,
            &REQUIRED_ACCEPTED_CATEGORIES,
            "French V1 accepts every documented lexical category and no unknown category",
        )?;
        require_exact_set(
            "proper_name_categories",
            &self.proper_name_categories,
            &REQUIRED_PROPER_NAME_CATEGORIES,
            "French V1 defensively rejects the supported proper-name category spellings",
        )?;
        require_string_equal(
            "abbreviation_subcategory",
            &self.abbreviation_subcategory,
            "abbreviation",
            "use Morphalou's documented abbreviation subcategory",
        )?;
        require_string_equal(
            "locution_value",
            &self.locution_value,
            "oui",
            "use Morphalou's documented locution marker",
        )?;
        require_exact_set(
            "excluded_variant_only_origins",
            &self.excluded_variant_only_origins,
            &["lefff"],
            "French V1 excludes spelling variants whose only evidence is Lefff",
        )?;
        require_string_equal(
            "normalization_profile",
            &self.normalization_profile,
            FRENCH_NORMALIZATION_PROFILE,
            "French V1 requires the pinned accent-and-ligature folding profile",
        )?;
        require_string_equal(
            "alphabet",
            &self.alphabet,
            "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
            "French V1 uses exactly the 26 uppercase basic Latin board tokens",
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
        Ok(())
    }

    pub(crate) fn accepts_grammatical_category(&self, category: &str) -> bool {
        self.accepted_grammatical_categories
            .iter()
            .any(|accepted| accepted == category)
    }

    pub(crate) fn is_proper_name_category(&self, category: &str) -> bool {
        self.proper_name_categories
            .iter()
            .any(|proper| proper == category)
    }

    pub(crate) fn is_nonstandard_variant(
        &self,
        has_spelling_variant: bool,
        lemma_origins: &BTreeSet<String>,
    ) -> bool {
        has_spelling_variant
            && (lemma_origins.is_empty()
                || lemma_origins.iter().all(|origin| {
                    self.excluded_variant_only_origins
                        .iter()
                        .any(|excluded| excluded == origin)
                }))
    }
}

fn validate_zip_member_path(value: &str) -> Result<(), BuilderError> {
    require_nonempty("source_archive_data_path", value)?;
    if value.starts_with('/')
        || value.contains('\\')
        || !Path::new(value)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("xml"))
        || value
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        Err(BuilderError::InvalidPolicy {
            field: "source_archive_data_path",
            value: value.to_owned(),
            reason: "use a safe relative ZIP member path ending in .xml",
        })
    } else {
        Ok(())
    }
}

fn require_exact_set(
    field: &'static str,
    values: &[String],
    expected: &[&str],
    reason: &'static str,
) -> Result<(), BuilderError> {
    let value_count = values.len();
    let values = values.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    if value_count == expected.len() && values == expected {
        Ok(())
    } else {
        Err(BuilderError::InvalidPolicy {
            field,
            value: format!("{values:?}"),
            reason,
        })
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
