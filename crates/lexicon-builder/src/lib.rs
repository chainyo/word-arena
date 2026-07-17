//! Reproducible and auditable source importers for Word Arena lexicon packs.
//!
//! Generated word data is always written outside the repository. This crate
//! contains build policy and deterministic transformations, not word lists.

mod english;
mod error;
mod french;
mod french_policy;
mod policy;
mod scowl;
mod util;

pub use english::{
    AuditDecision, AuditRecord, BuildMetadata, BuildReport, BuildSummary, RejectReason,
    SourceClass, SourceFileReport, build_english_from_archive, build_english_from_final,
};
pub use error::BuilderError;
pub use french::{
    FrenchAuditRecord, FrenchBuildReport, FrenchBuildSummary, FrenchFormKind, FrenchRejectReason,
    build_french_from_archive, build_french_from_xml,
};
pub use french_policy::FrenchPolicy;
pub use policy::EnglishPolicy;
pub use scowl::{PreparedScowl, prepare_scowl_archive};

/// Stable builder name recorded in generated metadata.
pub const BUILDER_NAME: &str = "word-arena-lexicon-builder";

/// Builder version recorded in generated metadata.
pub const BUILDER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Generated sorted-key filename.
pub const KEYS_FILE: &str = "keys.txt";

/// Generated per-source-row audit filename.
pub const AUDIT_FILE: &str = "audit.jsonl";

/// Generated aggregate filter report filename.
pub const FILTER_REPORT_FILE: &str = "filter-report.toml";

/// Generated reproducibility metadata filename.
pub const BUILD_METADATA_FILE: &str = "build.toml";
