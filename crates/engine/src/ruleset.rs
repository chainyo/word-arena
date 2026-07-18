use serde::{Deserialize, Serialize};
use word_arena_lexicon::{
    CompatibilityContext, ENGLISH_NORMALIZATION_PROFILE, FRENCH_NORMALIZATION_PROFILE,
    NORMALIZATION_ALGORITHM, NORMALIZATION_VERSION, NormalizationDescriptor, PackIdentity,
    ensure_exact_pack,
};

use crate::{GameError, Language};

/// Static ruleset schema recorded with games and replay bundles.
pub const RULESET_SCHEMA_VERSION: u32 = 1;

const ENGLISH_CONTENT_SHA256: &str =
    "27faaa6b78de526d7e7681bf1af45ce952cb0400897190c79eab7c67b278a54b";
const FRENCH_CONTENT_SHA256: &str =
    "c926a5f1ead63711d041277c9bfb3af23f3a460bb6edf57ff66408552c495193";

/// Stable identifier for one immutable game-rules generation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RulesetId {
    /// English V1 using the curated world-English pack.
    EnglishV1,
    /// French V1 using the curated Morphalou-derived pack.
    FrenchV1,
}

/// Versioned rules and exact lexicon identity required for a new game.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Ruleset {
    /// Ruleset schema version.
    pub schema_version: u32,
    /// Stable ruleset identity.
    pub id: RulesetId,
    /// Rules language.
    pub language: Language,
    /// Exact immutable pack allowed by this ruleset.
    pub lexicon: PackIdentity,
}

impl Ruleset {
    /// English V1 production ruleset and release-pack pin.
    #[must_use]
    pub fn english_v1() -> Self {
        Self::new(
            RulesetId::EnglishV1,
            Language::English,
            "word-arena-en-world-v1",
            "en",
            ENGLISH_NORMALIZATION_PROFILE,
            ENGLISH_CONTENT_SHA256,
        )
    }

    /// French V1 production ruleset and release-pack pin.
    #[must_use]
    pub fn french_v1() -> Self {
        Self::new(
            RulesetId::FrenchV1,
            Language::French,
            "word-arena-fr-v1",
            "fr",
            FRENCH_NORMALIZATION_PROFILE,
            FRENCH_CONTENT_SHA256,
        )
    }

    /// Returns the curated offline V1 ruleset for a supported language.
    ///
    /// # Errors
    ///
    /// Returns [`GameError::RulesetUnavailable`] until German or Spanish has a
    /// separately reviewed offline pack and immutable pin.
    pub fn for_language(language: Language) -> Result<Self, GameError> {
        match language {
            Language::English => Ok(Self::english_v1()),
            Language::French => Ok(Self::french_v1()),
            Language::German | Language::Spanish => Err(GameError::RulesetUnavailable { language }),
        }
    }

    /// Verifies every field against the immutable built-in definition.
    ///
    /// # Errors
    ///
    /// Returns [`GameError::InvalidRuleset`] when schema, language, pack ID,
    /// pack/format/normalization version, profile, or content checksum differs.
    pub fn validate(&self) -> Result<(), GameError> {
        let expected = match self.id {
            RulesetId::EnglishV1 => Self::english_v1(),
            RulesetId::FrenchV1 => Self::french_v1(),
        };
        if self == &expected {
            Ok(())
        } else {
            Err(GameError::InvalidRuleset { ruleset: self.id })
        }
    }

    pub(crate) fn ensure_lexicon(
        &self,
        context: CompatibilityContext,
        actual: &PackIdentity,
    ) -> Result<(), GameError> {
        self.validate()?;
        ensure_exact_pack(context, &self.lexicon, actual)?;
        Ok(())
    }

    pub(crate) fn letter_score(&self, letter: char) -> u32 {
        match self.language {
            Language::English => english_letter_score(letter),
            Language::French => french_letter_score(letter),
            Language::German | Language::Spanish => 0,
        }
    }

    fn new(
        id: RulesetId,
        language: Language,
        pack_id: &str,
        locale: &str,
        profile: &str,
        content_sha256: &str,
    ) -> Self {
        Self {
            schema_version: RULESET_SCHEMA_VERSION,
            id,
            language,
            lexicon: PackIdentity {
                pack_id: pack_id.to_owned(),
                pack_version: "1.0.0".to_owned(),
                format_version: 1,
                locale: locale.to_owned(),
                normalization: NormalizationDescriptor {
                    algorithm: NORMALIZATION_ALGORITHM.to_owned(),
                    version: NORMALIZATION_VERSION,
                    profile: profile.to_owned(),
                },
                content_sha256: content_sha256.to_owned(),
            },
        }
    }
}

fn english_letter_score(letter: char) -> u32 {
    match letter {
        'A' | 'E' | 'I' | 'L' | 'N' | 'O' | 'R' | 'S' | 'T' | 'U' => 1,
        'D' | 'G' => 2,
        'B' | 'C' | 'M' | 'P' => 3,
        'F' | 'H' | 'V' | 'W' | 'Y' => 4,
        'K' => 5,
        'J' | 'X' => 8,
        'Q' | 'Z' => 10,
        _ => 0,
    }
}

fn french_letter_score(letter: char) -> u32 {
    match letter {
        'A' | 'E' | 'I' | 'L' | 'N' | 'O' | 'R' | 'S' | 'T' | 'U' => 1,
        'D' | 'G' | 'M' => 2,
        'B' | 'C' | 'P' => 3,
        'F' | 'H' | 'V' => 4,
        'J' | 'Q' => 8,
        'K' | 'W' | 'X' | 'Y' | 'Z' => 10,
        _ => 0,
    }
}
