use std::{fs, path::Path};

use serde::Deserialize;

use crate::{BuilderError, HunspellPolicy};

/// Opaque evidence that one exact Hunspell policy passed native-language review.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovedNativeReview {
    locale: String,
    policy_id: String,
    policy_version: u32,
    source_id: String,
    reviewer: String,
    reviewed_on: String,
    evidence_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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

impl ApprovedNativeReview {
    /// Loads complete approval evidence for one exact policy.
    ///
    /// Pending records return [`BuilderError::NativeReviewRequired`]; this is
    /// the only constructor, so archive import cannot fabricate approval.
    ///
    /// # Errors
    ///
    /// Returns [`BuilderError`] for unsafe paths, I/O, TOML, pending status,
    /// missing evidence, or policy/source mismatch.
    pub fn load(lexicons_root: &Path, policy: &HunspellPolicy) -> Result<Self, BuilderError> {
        policy.validate()?;
        let path = lexicons_root.join(&policy.review_file);
        let encoded =
            fs::read_to_string(&path).map_err(|source| BuilderError::NativeReviewRead {
                path: path.clone(),
                source,
            })?;
        let record: ReviewRecord =
            toml::from_str(&encoded).map_err(|source| BuilderError::NativeReviewSyntax {
                path: path.clone(),
                source,
            })?;
        validate_record(&path, policy, record)
    }

    pub(crate) fn matches(&self, policy: &HunspellPolicy) -> bool {
        self.locale == policy.locale
            && self.policy_id == policy.id
            && self.policy_version == policy.version
            && self.source_id == policy.source_id
    }

    /// Reviewer identity retained for release provenance.
    #[must_use]
    pub fn reviewer(&self) -> &str {
        &self.reviewer
    }

    /// ISO calendar date of the approval.
    #[must_use]
    pub fn reviewed_on(&self) -> &str {
        &self.reviewed_on
    }

    /// Stable HTTPS review evidence.
    #[must_use]
    pub fn evidence_url(&self) -> &str {
        &self.evidence_url
    }
}

fn validate_record(
    path: &Path,
    policy: &HunspellPolicy,
    record: ReviewRecord,
) -> Result<ApprovedNativeReview, BuilderError> {
    let mismatch = record.schema_version != 1
        || record.language != policy.locale
        || record.policy_id != policy.id
        || record.policy_version != policy.version
        || record.source_id != policy.source_id
        || record.required_qualification != policy.review_requirement.qualification;
    if mismatch {
        return required(
            path,
            policy,
            "review identity does not match the exact policy and source",
        );
    }
    if record.status != "approved" || record.decision.as_deref() != Some("approved") {
        let pending_detail = record
            .gate
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("review status and decision must both be approved");
        return required(path, policy, pending_detail);
    }
    let reviewer = required_value(path, policy, "reviewer", record.reviewer)?;
    let reviewed_on = required_value(path, policy, "reviewed_on", record.reviewed_on)?;
    if !valid_date(&reviewed_on) {
        return required(path, policy, "reviewed_on must be an ISO YYYY-MM-DD date");
    }
    let _rationale = required_value(path, policy, "rationale", record.rationale)?;
    let evidence_url = required_value(path, policy, "evidence_url", record.evidence_url)?;
    if !evidence_url.starts_with("https://") {
        return required(path, policy, "evidence_url must use HTTPS");
    }
    Ok(ApprovedNativeReview {
        locale: record.language,
        policy_id: record.policy_id,
        policy_version: record.policy_version,
        source_id: record.source_id,
        reviewer,
        reviewed_on,
        evidence_url,
    })
}

fn required_value(
    path: &Path,
    policy: &HunspellPolicy,
    field: &str,
    value: Option<String>,
) -> Result<String, BuilderError> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| BuilderError::NativeReviewRequired {
            locale: policy.locale.clone(),
            path: path.to_path_buf(),
            reason: format!("approved review is missing {field}"),
        })
}

fn required<T>(
    path: &Path,
    policy: &HunspellPolicy,
    reason: impl Into<String>,
) -> Result<T, BuilderError> {
    Err(BuilderError::NativeReviewRequired {
        locale: policy.locale.clone(),
        path: path.to_path_buf(),
        reason: reason.into(),
    })
}

fn valid_date(value: &str) -> bool {
    value.len() == 10
        && value.as_bytes()[4] == b'-'
        && value.as_bytes()[7] == b'-'
        && value
            .bytes()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
}
