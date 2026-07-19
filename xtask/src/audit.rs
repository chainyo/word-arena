use std::{collections::BTreeSet, fs, path::Path};

use serde::Deserialize;
use word_arena_lexicon::NORMALIZATION_VERSION;
use word_arena_lexicon_builder::{EnglishPolicy, FrenchPolicy, HunspellPolicy, load_curation};

use crate::{
    PackRecord, PackRegistry, XtaskError, artifact::sha256_file, release::audit_release_config,
};

const SOURCE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
struct SourceRegistry {
    schema_version: u32,
    sources: Vec<SourceRecord>,
}

#[derive(Debug, Deserialize)]
struct SourceRecord {
    id: String,
    language: String,
    version: String,
    revision: Option<String>,
    archive_url: String,
    archive_size_bytes: u64,
    archive_sha256: String,
    license_id: String,
    license_file: String,
    license_sha256: String,
    revision_url: Option<String>,
    canonical_source_url: Option<String>,
    license_source_url: Option<String>,
    attribution: Option<String>,
    compatibility: Option<String>,
    redistribution_obligations: Option<Vec<String>>,
    archive_root: Option<String>,
    archive_dictionary_path: Option<String>,
    archive_affix_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReviewRecord {
    schema_version: u32,
    language: String,
    policy_id: String,
    policy_version: u32,
    source_id: String,
    status: String,
    required_qualification: String,
    gate: Option<String>,
    reviewer: Option<String>,
    reviewed_on: Option<String>,
    decision: Option<String>,
    rationale: Option<String>,
    evidence_url: Option<String>,
}

/// Summary of the committed offline lexicon supply-chain audit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryAuditSummary {
    /// Validated source pins.
    pub source_count: usize,
    /// Validated pack registry records.
    pub pack_count: usize,
    /// Independently versioned release tag.
    pub release_tag: String,
}

/// Cross-checks every committed source, license, policy, curation, registry,
/// and release metadata input without downloading data.
///
/// # Errors
///
/// Returns [`XtaskError`] when a pin is malformed, a committed license hash
/// differs, or any version/identity contract drifts across files.
pub fn audit_repository(
    workspace_root: &Path,
    registry: &PackRegistry,
) -> Result<RepositoryAuditSummary, XtaskError> {
    let sources = load_sources(&workspace_root.join("lexicons/sources.toml"))?;
    validate_sources(workspace_root, &sources)?;

    let english_policy =
        EnglishPolicy::load(&workspace_root.join("lexicons/policies/en-world-v1.toml"))?;
    let english_source = require_source(&sources, &english_policy.source_id)?;
    validate_contract(
        registry.require(&english_policy.pack_id)?,
        english_source,
        &english_policy.source_revision,
        &english_policy.source_archive_sha256,
        english_policy.source_archive_size_bytes,
        &english_policy.normalization_profile,
        english_policy.version,
        &workspace_root.join("lexicons/curation/en-world-v1"),
    )?;

    let french_policy = FrenchPolicy::load(&workspace_root.join("lexicons/policies/fr-v1.toml"))?;
    let french_source = require_source(&sources, &french_policy.source_id)?;
    validate_contract(
        registry.require(&french_policy.pack_id)?,
        french_source,
        &french_policy.source_revision,
        &french_policy.source_archive_sha256,
        french_policy.source_archive_size_bytes,
        &french_policy.normalization_profile,
        french_policy.version,
        &workspace_root.join("lexicons/curation/fr-v1"),
    )?;

    validate_gated_selection(workspace_root, registry, &sources, "de", "de-v1.toml")?;
    validate_gated_selection(workspace_root, registry, &sources, "es", "es-v1.toml")?;

    let release_tag = audit_release_config(workspace_root, registry)?;
    Ok(RepositoryAuditSummary {
        source_count: sources.sources.len(),
        pack_count: registry.packs.len(),
        release_tag,
    })
}

fn load_sources(path: &Path) -> Result<SourceRegistry, XtaskError> {
    let encoded = fs::read_to_string(path).map_err(|source| XtaskError::RegistryRead {
        path: path.to_path_buf(),
        source,
    })?;
    toml::from_str(&encoded).map_err(|source| XtaskError::RegistrySyntax {
        path: path.to_path_buf(),
        source,
    })
}

fn validate_sources(workspace_root: &Path, registry: &SourceRegistry) -> Result<(), XtaskError> {
    if registry.schema_version != SOURCE_SCHEMA_VERSION || registry.sources.len() != 4 {
        return audit_error("source registry must use schema 1 and contain exactly four sources");
    }
    let mut ids = BTreeSet::new();
    let mut languages = BTreeSet::new();
    for source in &registry.sources {
        if !ids.insert(source.id.as_str())
            || !languages.insert(source.language.as_str())
            || !matches!(source.language.as_str(), "en" | "fr" | "de" | "es")
            || !source.archive_url.starts_with("https://")
            || source.archive_size_bytes == 0
            || !is_sha256(&source.archive_sha256)
            || !is_sha256(&source.license_sha256)
            || source.license_id.is_empty()
            || !portable_license_path(&source.license_file)
            || !source
                .revision_url
                .as_deref()
                .is_some_and(|value| value.starts_with("https://"))
            || !source
                .canonical_source_url
                .as_deref()
                .is_some_and(|value| value.starts_with("https://"))
            || !source
                .license_source_url
                .as_deref()
                .is_some_and(|value| value.starts_with("https://"))
            || source
                .attribution
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            || !source
                .redistribution_obligations
                .as_ref()
                .is_some_and(|values| {
                    !values.is_empty() && values.iter().all(|value| !value.trim().is_empty())
                })
        {
            return audit_error(format!("invalid or duplicate source pin {}", source.id));
        }
        let license = workspace_root.join("lexicons").join(&source.license_file);
        if !license.is_file() || sha256_file(&license)? != source.license_sha256 {
            return audit_error(format!(
                "committed license {} does not match source pin {}",
                license.display(),
                source.id
            ));
        }
        if matches!(source.language.as_str(), "de" | "es")
            && source
                .compatibility
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
        {
            return audit_error(format!(
                "source {} is missing its license compatibility analysis",
                source.id
            ));
        }
    }
    if languages != BTreeSet::from(["de", "en", "es", "fr"]) {
        return audit_error("source registry must contain one pin for each initial language");
    }
    Ok(())
}

fn validate_gated_selection(
    workspace_root: &Path,
    pack_registry: &PackRegistry,
    sources: &SourceRegistry,
    language: &str,
    policy_name: &str,
) -> Result<(), XtaskError> {
    let policy_path = workspace_root.join("lexicons/policies").join(policy_name);
    let policy = HunspellPolicy::load(&policy_path)?;
    let source = require_source(sources, &policy.source_id)?;
    let expected_revision = source.revision.as_deref().unwrap_or(&source.version);
    let policy_valid = source.language == language
        && policy.source_revision == expected_revision
        && policy.source_archive_sha256 == source.archive_sha256
        && policy.source_archive_size_bytes == source.archive_size_bytes
        && source.archive_root.as_deref() == Some(policy.source_archive_root.as_str())
        && source.archive_dictionary_path.as_deref()
            == Some(policy.source_dictionary_path.as_str())
        && source.archive_affix_path.as_deref() == Some(policy.source_affix_path.as_str());
    if !policy_valid {
        return audit_error(format!("invalid or incomplete {language} selection policy"));
    }

    let review_path = workspace_root.join("lexicons").join(&policy.review_file);
    if !portable_review_path(&policy.review_file) {
        return audit_error(format!("invalid {language} reviewer evidence path"));
    }
    let review: ReviewRecord = load_toml(&review_path)?;
    if review.schema_version != 1
        || review.language != language
        || review.policy_id != policy.id
        || review.policy_version != policy.version
        || review.source_id != policy.source_id
        || review.required_qualification != policy.review_requirement.qualification
    {
        return audit_error(format!(
            "{language} reviewer evidence does not match its policy"
        ));
    }

    match review.status.as_str() {
        "pending" => {
            let has_gate = review
                .gate
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());
            let has_approval = review.reviewer.is_some()
                || review.reviewed_on.is_some()
                || review.decision.is_some()
                || review.rationale.is_some()
                || review.evidence_url.is_some();
            if !has_gate
                || has_approval
                || pack_registry
                    .packs
                    .iter()
                    .any(|pack| pack.locale == language)
            {
                return audit_error(format!(
                    "pending {language} review must gate every import and release"
                ));
            }
        }
        "approved" => {
            let complete = review
                .reviewer
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
                && review.reviewed_on.as_deref().is_some_and(valid_review_date)
                && review.decision.as_deref() == Some("approved")
                && review
                    .rationale
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
                && review
                    .evidence_url
                    .as_deref()
                    .is_some_and(|value| value.starts_with("https://"));
            if !complete {
                return audit_error(format!(
                    "approved {language} review lacks reviewer evidence"
                ));
            }
        }
        _ => return audit_error(format!("invalid {language} review status")),
    }
    Ok(())
}

fn load_toml<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, XtaskError> {
    let encoded = fs::read_to_string(path).map_err(|source| XtaskError::RegistryRead {
        path: path.to_path_buf(),
        source,
    })?;
    toml::from_str(&encoded).map_err(|source| XtaskError::RegistrySyntax {
        path: path.to_path_buf(),
        source,
    })
}

fn portable_review_path(path: &str) -> bool {
    path.starts_with("reviews/")
        && path
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn valid_review_date(value: &str) -> bool {
    value.len() == 10
        && value.as_bytes()[4] == b'-'
        && value.as_bytes()[7] == b'-'
        && value
            .bytes()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
}

#[allow(clippy::too_many_arguments)]
fn validate_contract(
    pack: &PackRecord,
    source: &SourceRecord,
    source_revision: &str,
    archive_sha256: &str,
    archive_size_bytes: u64,
    normalization_profile: &str,
    policy_version: u32,
    curation_directory: &Path,
) -> Result<(), XtaskError> {
    let expected_revision = source.revision.as_deref().unwrap_or(&source.version);
    let curation = load_curation(curation_directory)?;
    let governance = curation.governance;
    let matches = pack.locale == source.language
        && pack.license_id == source.license_id
        && source_revision == expected_revision
        && archive_sha256 == source.archive_sha256
        && archive_size_bytes == source.archive_size_bytes
        && pack.normalization_profile == normalization_profile
        && governance.pack_id == pack.pack_id
        && governance.policy_version == policy_version
        && governance.normalization_version == NORMALIZATION_VERSION
        && governance.normalization_profile == normalization_profile;
    if matches {
        Ok(())
    } else {
        audit_error(format!(
            "source, policy, curation, and pack registry contract drift for {}",
            pack.pack_id
        ))
    }
}

fn require_source<'a>(
    registry: &'a SourceRegistry,
    source_id: &str,
) -> Result<&'a SourceRecord, XtaskError> {
    registry
        .sources
        .iter()
        .find(|source| source.id == source_id)
        .ok_or_else(|| XtaskError::UnknownSource {
            source_id: source_id.to_owned(),
        })
}

fn portable_license_path(path: &str) -> bool {
    path.starts_with("licenses/")
        && path
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn audit_error<T>(message: impl Into<String>) -> Result<T, XtaskError> {
    Err(XtaskError::BuildContract {
        message: message.into(),
    })
}
