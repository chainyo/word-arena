use std::{
    collections::BTreeSet,
    fmt::{self, Write as _},
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use word_arena_lexicon::normalize_key;

use crate::{KEYS_FILE, util::sha256_file};

const CURATION_SCHEMA_VERSION: u32 = 1;
const BASELINE_POLICY_VERSION: u32 = 1;
const BASELINE_NORMALIZATION_VERSION: u32 = 1;
const MIN_WORD_LENGTH: usize = 2;
const MAX_WORD_LENGTH: usize = 15;
const ADDITIONS_FILE: &str = "additions.toml";
const REMOVALS_FILE: &str = "removals.toml";
const GOVERNANCE_FILE: &str = "governance.toml";

/// Deterministic Markdown changelog emitted by the curation stage.
pub const CURATION_CHANGELOG_FILE: &str = "curation-changelog.md";

/// Checksummed curation-stage report emitted beside the curated keys.
pub const CURATION_REPORT_FILE: &str = "curation-report.toml";

const OPEN_EVIDENCE_LICENSES: [&str; 8] = [
    "CC0-1.0",
    "CC-BY-4.0",
    "CC-BY-SA-4.0",
    "ODC-BY-1.0",
    "ODbL-1.0",
    "PDDL-1.0",
    "LicenseRef-LGPLLR",
    "LicenseRef-SCOWL-v1",
];

/// Typed action carried by every individual curation override.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CurationAction {
    /// Add a key absent from the generated source set.
    Add,
    /// Remove a key present in the generated source set.
    Remove,
}

impl fmt::Display for CurationAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Add => "add",
            Self::Remove => "remove",
        })
    }
}

/// One fully attributable source-controlled word override.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CurationOverride {
    /// Exact normalized runtime key.
    pub normalized_word: String,
    /// Explicit add/remove action.
    pub action: CurationAction,
    /// Human-readable justification for changing the generated word set.
    pub reason: String,
    /// Title of the openly usable supporting source.
    pub supporting_source_title: String,
    /// HTTPS location of the supporting evidence.
    pub supporting_source_url: String,
    /// Allowlisted open-data or linguistic-resource license ID.
    pub supporting_source_license: String,
    /// Identity proposing the override.
    pub author: String,
    /// Independent reviewing identity.
    pub reviewer: String,
    /// Review date in `YYYY-MM-DD` form.
    pub date: String,
}

/// Strict additions or removals document for one pack.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CurationDocument {
    /// Document schema version.
    pub schema_version: u32,
    /// Pack receiving the overrides.
    pub pack_id: String,
    /// Normalization profile used to validate exact keys.
    pub normalization_profile: String,
    /// Action required for every row in this file.
    pub document_action: CurationAction,
    /// Individually documented changes.
    pub overrides: Vec<CurationOverride>,
}

/// Kind of versioned change requiring explicit two-person approval.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HighImpactKind {
    /// A broad source-selection or filtering-policy change.
    BroadFilter,
    /// A board-key normalization change.
    Normalization,
}

impl fmt::Display for HighImpactKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::BroadFilter => "broad_filter",
            Self::Normalization => "normalization",
        })
    }
}

/// Two-person approval record for one high-impact version change.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HighImpactApproval {
    /// Protected input being changed.
    pub kind: HighImpactKind,
    /// New policy or normalization version approved by this record.
    pub version: u32,
    /// Scope and rationale for the change.
    pub summary: String,
    /// HTTPS issue or pull-request URL containing the review record.
    pub tracking_url: String,
    /// Identity proposing the change.
    pub author: String,
    /// Independent reviewing identity.
    pub reviewer: String,
    /// Approval date in `YYYY-MM-DD` form.
    pub date: String,
}

/// Version and approval metadata required for every curation directory.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CurationGovernance {
    /// Document schema version.
    pub schema_version: u32,
    /// Pack governed by this document.
    pub pack_id: String,
    /// Current filter-policy version.
    pub policy_version: u32,
    /// Current normalization algorithm version.
    pub normalization_version: u32,
    /// Current normalization profile.
    pub normalization_profile: String,
    /// Required approvals beyond the V1 baseline.
    pub approvals: Vec<HighImpactApproval>,
}

/// Validated additions, removals, and high-impact governance for one pack.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CurationBundle {
    /// Typed additions document.
    pub additions: CurationDocument,
    /// Typed removals document.
    pub removals: CurationDocument,
    /// Version and approval metadata.
    pub governance: CurationGovernance,
}

/// Reproducibility report for one applied curation set.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CurationReport {
    /// Report schema version.
    pub schema_version: u32,
    /// Curated pack ID.
    pub pack_id: String,
    /// Normalization profile.
    pub normalization_profile: String,
    /// Filter-policy version.
    pub policy_version: u32,
    /// Normalization algorithm version.
    pub normalization_version: u32,
    /// Generated source-set key count.
    pub base_word_count: u64,
    /// Applied additions.
    pub added_word_count: u64,
    /// Applied removals.
    pub removed_word_count: u64,
    /// Final curated key count.
    pub final_word_count: u64,
    /// Input generated-key checksum.
    pub base_keys_sha256: String,
    /// Additions document checksum.
    pub additions_sha256: String,
    /// Removals document checksum.
    pub removals_sha256: String,
    /// Governance document checksum.
    pub governance_sha256: String,
    /// Curated key checksum.
    pub curated_keys_sha256: String,
    /// Deterministic changelog checksum.
    pub changelog_sha256: String,
}

/// Result of an atomically published curation stage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CurationSummary {
    /// Final output directory.
    pub output_directory: PathBuf,
    /// Applied additions in key order.
    pub additions: Vec<CurationOverride>,
    /// Applied removals in key order.
    pub removals: Vec<CurationOverride>,
    /// Checksummed stage report.
    pub report: CurationReport,
}

/// Failures produced by strict curation loading and application.
#[derive(Debug, Error)]
pub enum CurationError {
    /// A curation TOML document cannot be read.
    #[error("failed to read curation document at {path}: {source}")]
    Read {
        /// Document path.
        path: PathBuf,
        /// Underlying I/O failure.
        #[source]
        source: std::io::Error,
    },

    /// A curation TOML document is malformed or contains unknown fields.
    #[error("invalid curation TOML at {path}: {source}")]
    Syntax {
        /// Document path.
        path: PathBuf,
        /// TOML decode failure.
        #[source]
        source: toml::de::Error,
    },

    /// A document-level contract value is unsupported.
    #[error("invalid curation document {path} field {field}: {value:?}; {reason}")]
    InvalidDocument {
        /// Document path.
        path: PathBuf,
        /// Field name.
        field: &'static str,
        /// Rejected value.
        value: String,
        /// Required rule.
        reason: &'static str,
    },

    /// One override lacks valid, open, attributable evidence.
    #[error("invalid override #{index} in {path}, field {field}: {value:?}; {reason}")]
    InvalidOverride {
        /// Document path.
        path: PathBuf,
        /// One-based row index.
        index: usize,
        /// Field name.
        field: &'static str,
        /// Rejected value.
        value: String,
        /// Required rule.
        reason: &'static str,
    },

    /// One high-impact approval record is malformed.
    #[error("invalid high-impact approval #{index} in {path}, field {field}: {value:?}; {reason}")]
    InvalidApproval {
        /// Governance path.
        path: PathBuf,
        /// One-based record index.
        index: usize,
        /// Field name.
        field: &'static str,
        /// Rejected value.
        value: String,
        /// Required rule.
        reason: &'static str,
    },

    /// A word appears more than once for the same action.
    #[error("duplicate {action} override for normalized word {word:?}")]
    DuplicateOverride {
        /// Duplicated action.
        action: CurationAction,
        /// Duplicated normalized key.
        word: String,
    },

    /// A word is listed as both an addition and a removal.
    #[error("conflicting add and remove overrides for normalized word {word:?}")]
    ConflictingOverride {
        /// Conflicting normalized key.
        word: String,
    },

    /// A versioned high-impact change has no matching independent approval.
    #[error("{kind} version {version} requires a two-person approval record")]
    MissingHighImpactApproval {
        /// Protected change kind.
        kind: HighImpactKind,
        /// Unapproved version.
        version: u32,
    },

    /// One base-key line is malformed, duplicated, or out of order.
    #[error("invalid base key at {path}:{line}: {value:?}; {reason}")]
    InvalidBaseKey {
        /// Generated keys path.
        path: PathBuf,
        /// One-based line number.
        line: u64,
        /// Rejected line.
        value: String,
        /// Required rule.
        reason: &'static str,
    },

    /// An addition already exists and therefore changes nothing.
    #[error("addition for {word:?} is a no-op because the base key already exists")]
    NoopAddition {
        /// Ineffective key.
        word: String,
    },

    /// A removal is absent and therefore changes nothing.
    #[error("removal for {word:?} is a no-op because the base key does not exist")]
    NoopRemoval {
        /// Ineffective key.
        word: String,
    },

    /// The curation output destination already exists.
    #[error("curation output already exists at {path}; choose a new destination")]
    OutputExists {
        /// Existing path.
        path: PathBuf,
    },

    /// A contextual filesystem operation failed.
    #[error("failed to access {path}: {source}")]
    Io {
        /// Path being accessed.
        path: PathBuf,
        /// Underlying failure.
        #[source]
        source: std::io::Error,
    },

    /// The deterministic curation report cannot be serialized.
    #[error("failed to serialize curation report TOML: {0}")]
    Toml(#[from] toml::ser::Error),

    /// A row count exceeds the portable report range.
    #[error("curation count exceeds u64: {0}")]
    CountOverflow(&'static str),
}

/// Loads and validates all three required curation documents in a directory.
///
/// # Errors
///
/// Returns [`CurationError`] for missing/malformed files, invalid normalized
/// keys, closed or proprietary evidence, duplicate/conflicting changes, or
/// missing independent high-impact approvals.
pub fn load_curation(directory: &Path) -> Result<CurationBundle, CurationError> {
    let additions_path = directory.join(ADDITIONS_FILE);
    let removals_path = directory.join(REMOVALS_FILE);
    let governance_path = directory.join(GOVERNANCE_FILE);
    let additions: CurationDocument = read_toml(&additions_path)?;
    let removals: CurationDocument = read_toml(&removals_path)?;
    let governance: CurationGovernance = read_toml(&governance_path)?;

    validate_document(&additions, &additions_path, CurationAction::Add)?;
    validate_document(&removals, &removals_path, CurationAction::Remove)?;
    validate_governance(&governance, &governance_path)?;
    validate_bundle_identity(
        &additions,
        &removals,
        &governance,
        &removals_path,
        &governance_path,
    )?;

    let added = additions
        .overrides
        .iter()
        .map(|change| change.normalized_word.as_str())
        .collect::<BTreeSet<_>>();
    for removal in &removals.overrides {
        if added.contains(removal.normalized_word.as_str()) {
            return Err(CurationError::ConflictingOverride {
                word: removal.normalized_word.clone(),
            });
        }
    }
    Ok(CurationBundle {
        additions,
        removals,
        governance,
    })
}

/// Applies a validated curation directory to a generated sorted key file.
///
/// The destination is atomically published with curated `keys.txt`, a
/// deterministic Markdown changelog, and a checksummed TOML report.
///
/// # Errors
///
/// Returns [`CurationError`] when inputs are invalid, an override is a no-op,
/// output serialization fails, or the destination already exists.
pub fn apply_curation(
    base_keys_path: &Path,
    output_directory: &Path,
    curation_directory: &Path,
) -> Result<CurationSummary, CurationError> {
    let bundle = load_curation(curation_directory)?;
    let mut keys = read_base_keys(base_keys_path, &bundle.governance.normalization_profile)?;
    let base_word_count = to_u64(keys.len(), "base word count")?;
    let additions = sorted_overrides(&bundle.additions.overrides);
    let removals = sorted_overrides(&bundle.removals.overrides);
    for addition in &additions {
        if !keys.insert(addition.normalized_word.clone()) {
            return Err(CurationError::NoopAddition {
                word: addition.normalized_word.clone(),
            });
        }
    }
    for removal in &removals {
        if !keys.remove(&removal.normalized_word) {
            return Err(CurationError::NoopRemoval {
                word: removal.normalized_word.clone(),
            });
        }
    }
    let applied = AppliedCuration {
        keys,
        additions,
        removals,
        base_word_count,
    };
    publish_curation(
        base_keys_path,
        output_directory,
        curation_directory,
        &bundle,
        applied,
    )
}

struct AppliedCuration {
    keys: BTreeSet<String>,
    additions: Vec<CurationOverride>,
    removals: Vec<CurationOverride>,
    base_word_count: u64,
}

fn publish_curation(
    base_keys_path: &Path,
    output_directory: &Path,
    curation_directory: &Path,
    bundle: &CurationBundle,
    applied: AppliedCuration,
) -> Result<CurationSummary, CurationError> {
    let AppliedCuration {
        keys,
        additions,
        removals,
        base_word_count,
    } = applied;
    if output_directory.exists() {
        return Err(CurationError::OutputExists {
            path: output_directory.to_path_buf(),
        });
    }
    let parent = output_directory
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| CurationError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let staging = tempfile::Builder::new()
        .prefix(".word-arena-curation-")
        .tempdir_in(parent)
        .map_err(|source| CurationError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    let keys_path = staging.path().join(KEYS_FILE);
    write_keys(&keys_path, &keys)?;
    let changelog_path = staging.path().join(CURATION_CHANGELOG_FILE);
    write_changelog(&changelog_path, &bundle.governance, &additions, &removals)?;
    let report = CurationReport {
        schema_version: CURATION_SCHEMA_VERSION,
        pack_id: bundle.governance.pack_id.clone(),
        normalization_profile: bundle.governance.normalization_profile.clone(),
        policy_version: bundle.governance.policy_version,
        normalization_version: bundle.governance.normalization_version,
        base_word_count,
        added_word_count: to_u64(additions.len(), "addition count")?,
        removed_word_count: to_u64(removals.len(), "removal count")?,
        final_word_count: to_u64(keys.len(), "final word count")?,
        base_keys_sha256: sha256_file(base_keys_path).map_err(builder_io)?,
        additions_sha256: sha256_file(&curation_directory.join(ADDITIONS_FILE))
            .map_err(builder_io)?,
        removals_sha256: sha256_file(&curation_directory.join(REMOVALS_FILE))
            .map_err(builder_io)?,
        governance_sha256: sha256_file(&curation_directory.join(GOVERNANCE_FILE))
            .map_err(builder_io)?,
        curated_keys_sha256: sha256_file(&keys_path).map_err(builder_io)?,
        changelog_sha256: sha256_file(&changelog_path).map_err(builder_io)?,
    };
    write_toml(&staging.path().join(CURATION_REPORT_FILE), &report)?;
    fs::rename(staging.path(), output_directory).map_err(|source| CurationError::Io {
        path: output_directory.to_path_buf(),
        source,
    })?;
    Ok(CurationSummary {
        output_directory: output_directory.to_path_buf(),
        additions,
        removals,
        report,
    })
}

fn validate_document(
    document: &CurationDocument,
    path: &Path,
    expected_action: CurationAction,
) -> Result<(), CurationError> {
    require_document_version(document.schema_version, path)?;
    require_document_text(path, "pack_id", &document.pack_id)?;
    require_document_text(
        path,
        "normalization_profile",
        &document.normalization_profile,
    )?;
    if document.document_action != expected_action {
        return Err(CurationError::InvalidDocument {
            path: path.to_path_buf(),
            field: "document_action",
            value: document.document_action.to_string(),
            reason: "the additions file must be add and the removals file must be remove",
        });
    }
    let mut words = BTreeSet::new();
    for (offset, change) in document.overrides.iter().enumerate() {
        let index = offset + 1;
        validate_override(change, path, index, document, expected_action)?;
        if !words.insert(change.normalized_word.as_str()) {
            return Err(CurationError::DuplicateOverride {
                action: expected_action,
                word: change.normalized_word.clone(),
            });
        }
    }
    Ok(())
}

fn validate_override(
    change: &CurationOverride,
    path: &Path,
    index: usize,
    document: &CurationDocument,
    expected_action: CurationAction,
) -> Result<(), CurationError> {
    if change.action != expected_action {
        return invalid_override(
            path,
            index,
            "action",
            change.action.to_string(),
            "override action must match its additions/removals document",
        );
    }
    validate_normalized_word(
        path,
        index,
        &change.normalized_word,
        &document.normalization_profile,
    )?;
    require_override_text(path, index, "reason", &change.reason)?;
    require_override_text(
        path,
        index,
        "supporting_source_title",
        &change.supporting_source_title,
    )?;
    validate_evidence_url(path, index, change)?;
    if !OPEN_EVIDENCE_LICENSES.contains(&change.supporting_source_license.as_str()) {
        return invalid_override(
            path,
            index,
            "supporting_source_license",
            change.supporting_source_license.clone(),
            "use an allowlisted open evidence license from lexicons/CURATION.md",
        );
    }
    require_override_text(path, index, "author", &change.author)?;
    require_override_text(path, index, "reviewer", &change.reviewer)?;
    if change.author.eq_ignore_ascii_case(&change.reviewer) {
        return invalid_override(
            path,
            index,
            "reviewer",
            change.reviewer.clone(),
            "author and reviewer must be distinct; this is mandatory for two-letter words",
        );
    }
    if !valid_date(&change.date) {
        return invalid_override(
            path,
            index,
            "date",
            change.date.clone(),
            "use a real Gregorian date in YYYY-MM-DD form",
        );
    }
    Ok(())
}

fn validate_normalized_word(
    path: &Path,
    index: usize,
    word: &str,
    profile: &str,
) -> Result<(), CurationError> {
    let normalized = normalize_key(profile, word).map_err(|_| CurationError::InvalidOverride {
        path: path.to_path_buf(),
        index,
        field: "normalized_word",
        value: word.to_owned(),
        reason: "word must be representable by the document normalization profile",
    })?;
    if normalized.as_ref() != word {
        return invalid_override(
            path,
            index,
            "normalized_word",
            word.to_owned(),
            "store the exact normalized uppercase board key, not a source spelling",
        );
    }
    let length = word.chars().count();
    if !(MIN_WORD_LENGTH..=MAX_WORD_LENGTH).contains(&length) {
        return invalid_override(
            path,
            index,
            "normalized_word",
            word.to_owned(),
            "normalized words must contain 2 through 15 board tokens",
        );
    }
    Ok(())
}

fn validate_evidence_url(
    path: &Path,
    index: usize,
    change: &CurationOverride,
) -> Result<(), CurationError> {
    let url = &change.supporting_source_url;
    if !valid_https_url(url) {
        return invalid_override(
            path,
            index,
            "supporting_source_url",
            url.clone(),
            "use a nonempty HTTPS URL without whitespace or Markdown delimiters",
        );
    }
    let evidence = format!(
        "{} {} {} {}",
        change.supporting_source_title,
        change.supporting_source_url,
        change.supporting_source_license,
        change.reason
    )
    .to_ascii_lowercase();
    let forbidden = evidence.contains("scrabblewordfinder")
        || evidence.contains("collins")
        || evidence
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|token| matches!(token, "nwl" | "ods" | "ospd"));
    if forbidden {
        return invalid_override(
            path,
            index,
            "supporting_source_url",
            url.clone(),
            "do not use proprietary word lists, scraped checkers, NWL, Collins, ODS, or OSPD",
        );
    }
    Ok(())
}

fn validate_governance(governance: &CurationGovernance, path: &Path) -> Result<(), CurationError> {
    require_document_version(governance.schema_version, path)?;
    require_document_text(path, "pack_id", &governance.pack_id)?;
    require_document_text(
        path,
        "normalization_profile",
        &governance.normalization_profile,
    )?;
    if governance.policy_version == 0 {
        return invalid_document(
            path,
            "policy_version",
            governance.policy_version.to_string(),
            "version must be greater than zero",
        );
    }
    if governance.normalization_version == 0 {
        return invalid_document(
            path,
            "normalization_version",
            governance.normalization_version.to_string(),
            "version must be greater than zero",
        );
    }
    let mut approval_versions = BTreeSet::new();
    for (offset, approval) in governance.approvals.iter().enumerate() {
        let index = offset + 1;
        validate_approval(approval, path, index)?;
        if !approval_versions.insert((approval.kind, approval.version)) {
            return invalid_approval(
                path,
                index,
                "version",
                approval.version.to_string(),
                "only one approval record is allowed per kind and version",
            );
        }
    }
    require_version_approval(
        HighImpactKind::BroadFilter,
        governance.policy_version,
        BASELINE_POLICY_VERSION,
        &approval_versions,
    )?;
    require_version_approval(
        HighImpactKind::Normalization,
        governance.normalization_version,
        BASELINE_NORMALIZATION_VERSION,
        &approval_versions,
    )
}

fn validate_approval(
    approval: &HighImpactApproval,
    path: &Path,
    index: usize,
) -> Result<(), CurationError> {
    if approval.version <= 1 {
        return invalid_approval(
            path,
            index,
            "version",
            approval.version.to_string(),
            "approval records describe changes after the V1 baseline",
        );
    }
    require_approval_text(path, index, "summary", &approval.summary)?;
    if !valid_https_url(&approval.tracking_url) {
        return invalid_approval(
            path,
            index,
            "tracking_url",
            approval.tracking_url.clone(),
            "use an HTTPS issue or pull-request URL",
        );
    }
    require_approval_text(path, index, "author", &approval.author)?;
    require_approval_text(path, index, "reviewer", &approval.reviewer)?;
    if approval.author.eq_ignore_ascii_case(&approval.reviewer) {
        return invalid_approval(
            path,
            index,
            "reviewer",
            approval.reviewer.clone(),
            "normalization and broad-filter changes require two distinct people",
        );
    }
    if !valid_date(&approval.date) {
        return invalid_approval(
            path,
            index,
            "date",
            approval.date.clone(),
            "use a real Gregorian date in YYYY-MM-DD form",
        );
    }
    Ok(())
}

fn validate_bundle_identity(
    additions: &CurationDocument,
    removals: &CurationDocument,
    governance: &CurationGovernance,
    removals_path: &Path,
    governance_path: &Path,
) -> Result<(), CurationError> {
    if removals.pack_id != additions.pack_id {
        return invalid_document(
            removals_path,
            "pack_id",
            removals.pack_id.clone(),
            "additions and removals must target the same pack",
        );
    }
    if governance.pack_id != additions.pack_id {
        return invalid_document(
            governance_path,
            "pack_id",
            governance.pack_id.clone(),
            "governance and override documents must target the same pack",
        );
    }
    if removals.normalization_profile != additions.normalization_profile {
        return invalid_document(
            removals_path,
            "normalization_profile",
            removals.normalization_profile.clone(),
            "additions and removals must use the same normalization profile",
        );
    }
    if governance.normalization_profile != additions.normalization_profile {
        return invalid_document(
            governance_path,
            "normalization_profile",
            governance.normalization_profile.clone(),
            "governance and override documents must use the same profile",
        );
    }
    Ok(())
}

fn require_version_approval(
    kind: HighImpactKind,
    version: u32,
    baseline: u32,
    approvals: &BTreeSet<(HighImpactKind, u32)>,
) -> Result<(), CurationError> {
    if version > baseline && !approvals.contains(&(kind, version)) {
        Err(CurationError::MissingHighImpactApproval { kind, version })
    } else {
        Ok(())
    }
}

fn read_base_keys(path: &Path, profile: &str) -> Result<BTreeSet<String>, CurationError> {
    let file = File::open(path).map_err(|source| CurationError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let reader = BufReader::new(file);
    let mut keys = BTreeSet::new();
    let mut previous: Option<String> = None;
    for (offset, line) in reader.lines().enumerate() {
        let line_number = u64::try_from(offset + 1)
            .map_err(|_| CurationError::CountOverflow("base key line number"))?;
        let value = line.map_err(|source| CurationError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let normalized =
            normalize_key(profile, &value).map_err(|_| CurationError::InvalidBaseKey {
                path: path.to_path_buf(),
                line: line_number,
                value: value.clone(),
                reason: "key is not representable by the curation normalization profile",
            })?;
        let length = value.chars().count();
        if normalized.as_ref() != value || !(MIN_WORD_LENGTH..=MAX_WORD_LENGTH).contains(&length) {
            return Err(CurationError::InvalidBaseKey {
                path: path.to_path_buf(),
                line: line_number,
                value,
                reason: "keys must be normalized and contain 2 through 15 board tokens",
            });
        }
        if previous.as_ref().is_some_and(|prior| prior >= &value) {
            return Err(CurationError::InvalidBaseKey {
                path: path.to_path_buf(),
                line: line_number,
                value,
                reason: "keys must be unique and strictly sorted by UTF-8 bytes",
            });
        }
        keys.insert(value.clone());
        previous = Some(value);
    }
    Ok(keys)
}

fn sorted_overrides(changes: &[CurationOverride]) -> Vec<CurationOverride> {
    let mut changes = changes.to_vec();
    changes.sort_unstable_by(|left, right| left.normalized_word.cmp(&right.normalized_word));
    changes
}

fn write_keys(path: &Path, keys: &BTreeSet<String>) -> Result<(), CurationError> {
    let file = File::create(path).map_err(|source| CurationError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut writer = BufWriter::new(file);
    for key in keys {
        writer
            .write_all(key.as_bytes())
            .and_then(|()| writer.write_all(b"\n"))
            .map_err(|source| CurationError::Io {
                path: path.to_path_buf(),
                source,
            })?;
    }
    writer.flush().map_err(|source| CurationError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn write_changelog(
    path: &Path,
    governance: &CurationGovernance,
    additions: &[CurationOverride],
    removals: &[CurationOverride],
) -> Result<(), CurationError> {
    let mut output = format!(
        "# Curation changelog\n\nPack: `{}`  \nNormalization: `{}` v{}  \nFilter policy: v{}\n\n",
        governance.pack_id,
        governance.normalization_profile,
        governance.normalization_version,
        governance.policy_version
    );
    append_changelog_section(&mut output, "Added playable keys", additions);
    append_changelog_section(&mut output, "Removed playable keys", removals);
    fs::write(path, output).map_err(|source| CurationError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn append_changelog_section(output: &mut String, heading: &str, changes: &[CurationOverride]) {
    output.push_str("## ");
    output.push_str(heading);
    output.push_str("\n\n");
    if changes.is_empty() {
        output.push_str("No changes.\n\n");
        return;
    }
    for change in changes {
        writeln!(
            output,
            "- `{}` — {} Source: {} ({}; <{}>). Author: {}; reviewer: {}; date: {}.",
            change.normalized_word,
            change.reason,
            change.supporting_source_title,
            change.supporting_source_license,
            change.supporting_source_url,
            change.author,
            change.reviewer,
            change.date
        )
        .expect("writing to a String cannot fail");
    }
    output.push('\n');
}

fn write_toml<T>(path: &Path, value: &T) -> Result<(), CurationError>
where
    T: Serialize,
{
    let mut encoded = toml::to_string_pretty(value)?;
    if !encoded.ends_with('\n') {
        encoded.push('\n');
    }
    fs::write(path, encoded).map_err(|source| CurationError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn read_toml<T>(path: &Path) -> Result<T, CurationError>
where
    T: for<'de> Deserialize<'de>,
{
    let value = fs::read_to_string(path).map_err(|source| CurationError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    toml::from_str(&value).map_err(|source| CurationError::Syntax {
        path: path.to_path_buf(),
        source,
    })
}

fn require_document_version(version: u32, path: &Path) -> Result<(), CurationError> {
    if version == CURATION_SCHEMA_VERSION {
        Ok(())
    } else {
        invalid_document(
            path,
            "schema_version",
            version.to_string(),
            "only curation schema version 1 is supported",
        )
    }
}

fn require_document_text(
    path: &Path,
    field: &'static str,
    value: &str,
) -> Result<(), CurationError> {
    if valid_plain_text(value) {
        Ok(())
    } else {
        invalid_document(
            path,
            field,
            value.to_owned(),
            "value must be nonempty, single-line text without control characters",
        )
    }
}

fn require_override_text(
    path: &Path,
    index: usize,
    field: &'static str,
    value: &str,
) -> Result<(), CurationError> {
    if valid_plain_text(value) {
        Ok(())
    } else {
        invalid_override(
            path,
            index,
            field,
            value.to_owned(),
            "value must be nonempty, single-line text without control characters",
        )
    }
}

fn require_approval_text(
    path: &Path,
    index: usize,
    field: &'static str,
    value: &str,
) -> Result<(), CurationError> {
    if valid_plain_text(value) {
        Ok(())
    } else {
        invalid_approval(
            path,
            index,
            field,
            value.to_owned(),
            "value must be nonempty, single-line text without control characters",
        )
    }
}

fn valid_plain_text(value: &str) -> bool {
    !value.is_empty() && value == value.trim() && !value.chars().any(char::is_control)
}

fn valid_https_url(value: &str) -> bool {
    value.starts_with("https://")
        && value.len() > "https://".len()
        && !value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        && !value.contains(['<', '>', '(', ')'])
}

fn valid_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let Ok(year) = value[0..4].parse::<u32>() else {
        return false;
    };
    let Ok(month) = value[5..7].parse::<u32>() else {
        return false;
    };
    let Ok(day) = value[8..10].parse::<u32>() else {
        return false;
    };
    if year == 0 || !(1..=12).contains(&month) {
        return false;
    }
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=days).contains(&day)
}

fn invalid_document<T>(
    path: &Path,
    field: &'static str,
    value: String,
    reason: &'static str,
) -> Result<T, CurationError> {
    Err(CurationError::InvalidDocument {
        path: path.to_path_buf(),
        field,
        value,
        reason,
    })
}

fn invalid_override<T>(
    path: &Path,
    index: usize,
    field: &'static str,
    value: String,
    reason: &'static str,
) -> Result<T, CurationError> {
    Err(CurationError::InvalidOverride {
        path: path.to_path_buf(),
        index,
        field,
        value,
        reason,
    })
}

fn invalid_approval<T>(
    path: &Path,
    index: usize,
    field: &'static str,
    value: String,
    reason: &'static str,
) -> Result<T, CurationError> {
    Err(CurationError::InvalidApproval {
        path: path.to_path_buf(),
        index,
        field,
        value,
        reason,
    })
}

fn to_u64(value: usize, label: &'static str) -> Result<u64, CurationError> {
    u64::try_from(value).map_err(|_| CurationError::CountOverflow(label))
}

fn builder_io(error: crate::BuilderError) -> CurationError {
    match error {
        crate::BuilderError::Io { path, source } => CurationError::Io { path, source },
        other => CurationError::Io {
            path: PathBuf::from("checksum input"),
            source: std::io::Error::other(other),
        },
    }
}
