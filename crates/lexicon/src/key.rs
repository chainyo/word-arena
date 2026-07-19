use std::{fmt, ops::Deref};

use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

use crate::{
    ENGLISH_NORMALIZATION_PROFILE, FRENCH_NORMALIZATION_PROFILE, GERMAN_NORMALIZATION_PROFILE,
    NormalizedKeyError, SPANISH_NORMALIZATION_PROFILE,
};

/// A validated UTF-8 string compared by exact bytes in a compiled lexicon.
///
/// Construction does not guess a locale or silently normalize input. Builders
/// and query callers should use [`normalize_key`] with the profile pinned by the
/// pack and ruleset.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NormalizedKey(String);

impl NormalizedKey {
    /// Wraps an already-normalized UTF-8 string after checking generic key
    /// invariants.
    ///
    /// # Errors
    ///
    /// Returns [`NormalizedKeyError::Empty`] for an empty value or
    /// [`NormalizedKeyError::ForbiddenCharacter`] for whitespace and control
    /// characters.
    pub fn new(value: String) -> Result<Self, NormalizedKeyError> {
        if value.is_empty() {
            return Err(NormalizedKeyError::Empty);
        }
        if let Some(character) = value
            .chars()
            .find(|character| character.is_control() || character.is_whitespace())
        {
            return Err(NormalizedKeyError::ForbiddenCharacter { character });
        }
        Ok(Self(value))
    }

    /// Decodes an exact-membership key stored as bytes in a compiled pack.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid UTF-8, an empty key, whitespace, or control
    /// characters.
    pub fn from_utf8(value: Vec<u8>) -> Result<Self, NormalizedKeyError> {
        Self::new(String::from_utf8(value)?)
    }

    /// Returns the canonical UTF-8 bytes used for exact membership.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// Consumes the wrapper and returns the normalized string.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for NormalizedKey {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Deref for NormalizedKey {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Display for NormalizedKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<Vec<u8>> for NormalizedKey {
    type Error = NormalizedKeyError;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        Self::from_utf8(value)
    }
}

/// Applies normalization algorithm V1 for one supported locale profile.
///
/// English performs Unicode uppercasing and accepts only `A` through `Z`.
/// French, German, and Spanish additionally decompose and remove combining
/// marks, then accept only `A` through `Z`. French expands `Œ` to `OE` and `Æ`
/// to `AE`; Unicode uppercasing expands German `ß` to `SS`.
///
/// # Errors
///
/// Returns an error when the profile is unknown, the result is empty, or the
/// source contains a character that cannot be represented by that board.
pub fn normalize_key(profile: &str, source: &str) -> Result<NormalizedKey, NormalizedKeyError> {
    let normalized = match profile {
        ENGLISH_NORMALIZATION_PROFILE => uppercase_basic_latin(profile, source)?,
        FRENCH_NORMALIZATION_PROFILE => latin_fold(profile, source, true)?,
        GERMAN_NORMALIZATION_PROFILE | SPANISH_NORMALIZATION_PROFILE => {
            latin_fold(profile, source, false)?
        }
        _ => {
            return Err(NormalizedKeyError::UnsupportedProfile {
                profile: profile.to_owned(),
            });
        }
    };
    NormalizedKey::new(normalized)
}

fn uppercase_basic_latin(profile: &str, source: &str) -> Result<String, NormalizedKeyError> {
    let mut normalized = String::with_capacity(source.len());
    for character in source.chars().flat_map(char::to_uppercase) {
        push_basic_latin(profile, character, &mut normalized)?;
    }
    Ok(normalized)
}

fn latin_fold(
    profile: &str,
    source: &str,
    expand_ligatures: bool,
) -> Result<String, NormalizedKeyError> {
    let mut normalized = String::with_capacity(source.len());
    for source_character in source.chars() {
        match source_character {
            'œ' | 'Œ' if expand_ligatures => normalized.push_str("OE"),
            'æ' | 'Æ' if expand_ligatures => normalized.push_str("AE"),
            _ => {
                for upper in source_character.to_uppercase() {
                    for decomposed in std::iter::once(upper).nfd() {
                        if !is_combining_mark(decomposed) {
                            push_basic_latin(profile, decomposed, &mut normalized)?;
                        }
                    }
                }
            }
        }
    }
    Ok(normalized)
}

fn push_basic_latin(
    profile: &str,
    character: char,
    output: &mut String,
) -> Result<(), NormalizedKeyError> {
    if character.is_ascii_uppercase() {
        output.push(character);
        Ok(())
    } else {
        Err(NormalizedKeyError::UnsupportedCharacter {
            profile: profile.to_owned(),
            character,
        })
    }
}
