//! Deterministic game domain and rules engine for Word Arena.
//!
//! Transport, persistence, authentication, clocks, IDs, and random tile sources
//! belong outside this crate.

/// A language supported by the first Word Arena ruleset generation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
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
    }
}
