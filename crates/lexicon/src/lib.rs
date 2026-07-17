//! Versioned lexicon pack contracts, integrity validation, and board-key
//! normalization for Word Arena.
//!
//! This crate treats a lexicon as an immutable, separately licensed data pack.
//! It does not contain game rules, persistence, network access, or word data.

mod compatibility;
mod error;
mod index;
mod key;
mod manifest;
mod pack;

pub use compatibility::{
    CacheDecision, CompatibilityContext, ensure_exact_pack, plan_cache_install,
};
pub use error::{CompatibilityError, NormalizedKeyError, PackError};
pub use index::{LoadedLexicon, load_lexicon};
pub use key::{NormalizedKey, normalize_key};
pub use manifest::{
    BuilderDescriptor, FileDescriptor, NormalizationDescriptor, PackIdentity, PackManifest,
    PolicyDescriptor, SourceDescriptor,
};
pub use pack::{ValidatedPack, calculate_content_sha256, validate_pack};

/// The only pack manifest format understood by this crate.
pub const CURRENT_FORMAT_VERSION: u32 = 1;

/// The independently versioned algorithm used to create exact-membership keys.
pub const NORMALIZATION_ALGORITHM: &str = "word-arena-board-key";

/// The normalization algorithm version implemented by this crate.
pub const NORMALIZATION_VERSION: u32 = 1;

/// English V1 keeps only upper-case basic Latin board tokens.
pub const ENGLISH_NORMALIZATION_PROFILE: &str = "en-basic-latin-v1";

/// French V1 folds accents and supported ligatures to basic Latin board tokens.
pub const FRENCH_NORMALIZATION_PROFILE: &str = "fr-basic-latin-fold-v1";

/// The manifest filename at the root of every pack.
pub const MANIFEST_FILE: &str = "manifest.toml";

/// The compact exact-membership FST filename in every V1 pack.
pub const INDEX_FILE: &str = "lexicon.fst";

/// Payload files required by pack format V1.
pub const REQUIRED_PAYLOAD_FILES: [&str; 6] = [
    INDEX_FILE,
    "curation/additions.toml",
    "curation/removals.toml",
    "LICENSE",
    "SOURCE.md",
    "THIRD_PARTY_NOTICES",
];
