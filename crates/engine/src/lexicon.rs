use std::fmt::Debug;

use word_arena_lexicon::{LoadedLexicon, NormalizedKey, PackIdentity};

/// Query-only exact-membership boundary injected into deterministic gameplay.
///
/// Implementations must be immutable for the game lifetime. Source parsing,
/// installation, and network access are deliberately absent from this trait.
pub trait WordValidator: Debug + Send + Sync {
    /// Exact immutable identity used by this lookup instance.
    fn identity(&self) -> &PackIdentity;

    /// Tests one key already normalized with the identity's pinned profile.
    fn contains(&self, key: &NormalizedKey) -> bool;
}

impl WordValidator for LoadedLexicon {
    fn identity(&self) -> &PackIdentity {
        self.identity()
    }

    fn contains(&self, key: &NormalizedKey) -> bool {
        self.contains(key)
    }
}
