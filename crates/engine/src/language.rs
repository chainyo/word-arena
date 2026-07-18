use serde::{Deserialize, Serialize};

/// A language supported by Word Arena's static rule configuration.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// English.
    English,
    /// French.
    French,
    /// German.
    German,
    /// Spanish.
    Spanish,
}

impl Language {
    /// Every language planned for the first release.
    pub const ALL: [Self; 4] = [Self::English, Self::French, Self::German, Self::Spanish];

    /// Languages with curated offline V1 packs.
    pub const OFFLINE_V1: [Self; 2] = [Self::English, Self::French];

    /// Returns the stable BCP 47-compatible language code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::French => "fr",
            Self::German => "de",
            Self::Spanish => "es",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Language;

    #[test]
    fn initial_language_codes_are_stable() {
        assert_eq!(Language::ALL.map(Language::code), ["en", "fr", "de", "es"]);
        assert_eq!(Language::OFFLINE_V1.map(Language::code), ["en", "fr"]);
    }
}
