use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{BufWriter, Read, Write},
    path::{Path, PathBuf},
};

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use tar::Archive;
use word_arena_lexicon::normalize_key;

use crate::{
    AUDIT_FILE, ApprovedNativeReview, BUILD_METADATA_FILE, BUILDER_NAME, BUILDER_VERSION,
    BuildMetadata, BuilderError, FILTER_REPORT_FILE, HunspellPolicy, KEYS_FILE, util::sha256_file,
};

const REPORT_SCHEMA_VERSION: u32 = 1;
const METADATA_SCHEMA_VERSION: u32 = 1;

/// Hunspell acceptance class retained in deterministic source-form audits.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HunspellSourceClass {
    /// Normal accepted Hunspell word or generated inflection.
    Accepted,
    /// Accepted word intentionally omitted from spelling suggestions.
    NoSuggest,
}

/// Stable reason a generated Hunspell form cannot become a board key.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HunspellRejectReason {
    /// Source form is empty.
    Empty,
    /// Source form contains an apostrophe.
    Apostrophe,
    /// Source form contains a hyphen or Unicode dash.
    Hyphen,
    /// Source form contains whitespace.
    Whitespace,
    /// Source form contains a numeric character.
    Digit,
    /// Source form contains ASCII punctuation.
    Punctuation,
    /// Source form cannot be represented by the selected board profile.
    UnsupportedCharacter,
    /// Normalized key is shorter than policy permits.
    TooShort,
    /// Normalized key is longer than policy permits.
    TooLong,
}

impl HunspellRejectReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Apostrophe => "apostrophe",
            Self::Hyphen => "hyphen",
            Self::Whitespace => "whitespace",
            Self::Digit => "digit",
            Self::Punctuation => "punctuation",
            Self::UnsupportedCharacter => "unsupported_character",
            Self::TooShort => "too_short",
            Self::TooLong => "too_long",
        }
    }
}

/// Decision for one unique generated Hunspell form.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HunspellAuditRecord {
    /// Original UTF-8 source or affix-generated form.
    pub source_form: String,
    /// Every Hunspell acceptance class attached to the form.
    pub source_classes: Vec<HunspellSourceClass>,
    /// Normalized board key when normalization completed.
    pub normalized_key: Option<String>,
    /// Whether the form was accepted by the Word Arena board policy.
    pub accepted: bool,
    /// Stable rejection reason, absent for accepted forms.
    pub reason: Option<HunspellRejectReason>,
    /// Whether an earlier accepted source form produced the same board key.
    pub duplicate_key: bool,
}

/// Complete deterministic accounting for one Hunspell build.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HunspellBuildReport {
    /// Report schema version.
    pub schema_version: u32,
    /// Output pack ID.
    pub pack_id: String,
    /// Language code.
    pub locale: String,
    /// Filter policy identity.
    pub policy_id: String,
    /// Filter policy version.
    pub policy_version: u32,
    /// Pinned source identity.
    pub source_id: String,
    /// Full source revision.
    pub source_revision: String,
    /// Unique normal and no-suggest forms expanded by Hunspell rules.
    pub generated_forms: u64,
    /// Forms classified as forbidden by Hunspell and never considered.
    pub forbidden_forms: u64,
    /// Forms admitted by the board policy, including normalized collisions.
    pub accepted_forms: u64,
    /// Forms rejected by the board policy.
    pub rejected_forms: u64,
    /// Unique sorted exact-membership keys.
    pub unique_keys: u64,
    /// Accepted forms colliding with an earlier normalized key.
    pub duplicate_accepted_forms: u64,
    /// Rejection totals keyed by stable reason.
    pub rejection_reasons: BTreeMap<String, u64>,
}

/// Result of one atomically published synthetic or approved Hunspell build.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HunspellBuildSummary {
    /// Final output directory.
    pub output_directory: PathBuf,
    /// Complete row accounting.
    pub report: HunspellBuildReport,
    /// Reproducibility metadata and output checksums.
    pub metadata: BuildMetadata,
}

enum FormOutcome {
    Accepted(String),
    Rejected {
        reason: HunspellRejectReason,
        normalized_key: Option<String>,
    },
}

/// Verifies and imports one pinned `tar.gz` Hunspell source archive.
///
/// The opaque approval value can only be created from a complete native-review
/// record matching the exact policy. Approval is checked before archive bytes
/// are inspected. Only the two exact configured members are read; nothing is
/// extracted to disk.
///
/// # Errors
///
/// Returns [`BuilderError`] for missing review approval, source pin drift,
/// malformed/duplicate members, invalid UTF-8, or deterministic build failure.
pub fn build_hunspell_from_archive(
    archive_path: &Path,
    output_directory: &Path,
    policy: &HunspellPolicy,
    approval: &ApprovedNativeReview,
) -> Result<HunspellBuildSummary, BuilderError> {
    policy.validate()?;
    if !approval.matches(policy) {
        return Err(BuilderError::NativeReviewRequired {
            locale: policy.locale.clone(),
            path: PathBuf::from(&policy.review_file),
            reason: "approval does not match the exact policy and source".to_owned(),
        });
    }
    let actual_size = fs::metadata(archive_path)
        .map_err(|source| BuilderError::Io {
            path: archive_path.to_path_buf(),
            source,
        })?
        .len();
    if actual_size != policy.source_archive_size_bytes {
        return Err(BuilderError::HunspellArchiveSizeMismatch {
            path: archive_path.to_path_buf(),
            expected: policy.source_archive_size_bytes,
            actual: actual_size,
        });
    }
    let actual_sha256 = sha256_file(archive_path)?;
    if actual_sha256 != policy.source_archive_sha256 {
        return Err(BuilderError::HunspellArchiveChecksumMismatch {
            path: archive_path.to_path_buf(),
            expected: policy.source_archive_sha256.clone(),
            actual: actual_sha256,
        });
    }

    let dictionary_member = format!(
        "{}/{}",
        policy.source_archive_root, policy.source_dictionary_path
    );
    let affix_member = format!(
        "{}/{}",
        policy.source_archive_root, policy.source_affix_path
    );
    let archive_file = File::open(archive_path).map_err(|source| BuilderError::Io {
        path: archive_path.to_path_buf(),
        source,
    })?;
    let mut archive = Archive::new(GzDecoder::new(archive_file));
    let entries = archive.entries().map_err(|source| BuilderError::Io {
        path: archive_path.to_path_buf(),
        source,
    })?;
    let mut dictionary = None;
    let mut affix = None;
    for entry in entries {
        let mut entry = entry.map_err(|source| BuilderError::Io {
            path: archive_path.to_path_buf(),
            source,
        })?;
        let member = entry.path().map_err(|source| BuilderError::Io {
            path: archive_path.to_path_buf(),
            source,
        })?;
        let Some(member) = member.to_str().map(str::to_owned) else {
            continue;
        };
        let destination = if member == dictionary_member {
            &mut dictionary
        } else if member == affix_member {
            &mut affix
        } else {
            continue;
        };
        if destination.is_some() || !entry.header().entry_type().is_file() {
            return Err(BuilderError::HunspellArchiveMember {
                archive: archive_path.to_path_buf(),
                member: member.clone(),
                reason: "member must occur exactly once as a regular file".to_owned(),
            });
        }
        let mut encoded = String::new();
        entry.read_to_string(&mut encoded).map_err(|source| {
            BuilderError::HunspellArchiveMember {
                archive: archive_path.to_path_buf(),
                member: member.clone(),
                reason: format!("member is not valid readable UTF-8: {source}"),
            }
        })?;
        *destination = Some(encoded);
    }
    let dictionary = require_member(archive_path, &dictionary_member, dictionary)?;
    let affix = require_member(archive_path, &affix_member, affix)?;
    build_hunspell_from_strings(&affix, &dictionary, output_directory, policy)
}

fn require_member(
    archive_path: &Path,
    member: &str,
    value: Option<String>,
) -> Result<String, BuilderError> {
    value.ok_or_else(|| BuilderError::HunspellArchiveMember {
        archive: archive_path.to_path_buf(),
        member: member.to_owned(),
        reason: "required member is missing".to_owned(),
    })
}

/// Builds deterministic outputs from UTF-8 Hunspell files.
///
/// This lower-level entry point is intended for hand-authored fixtures and for
/// archive tooling after it has independently verified source pins and review
/// approval. It does not download or inspect any upstream data.
///
/// # Errors
///
/// Returns [`BuilderError`] for policy, input, parser, serialization,
/// accounting, or atomic-publication failures.
pub fn build_hunspell_from_files(
    affix_path: &Path,
    dictionary_path: &Path,
    output_directory: &Path,
    policy: &HunspellPolicy,
) -> Result<HunspellBuildSummary, BuilderError> {
    let affix = fs::read_to_string(affix_path).map_err(|source| BuilderError::Io {
        path: affix_path.to_path_buf(),
        source,
    })?;
    let dictionary = fs::read_to_string(dictionary_path).map_err(|source| BuilderError::Io {
        path: dictionary_path.to_path_buf(),
        source,
    })?;
    build_hunspell_from_strings(&affix, &dictionary, output_directory, policy)
}

/// Builds deterministic outputs from hand-authored Hunspell source strings.
///
/// # Errors
///
/// Returns [`BuilderError`] for policy, parser, filtering, serialization,
/// accounting, or atomic-publication failures.
pub fn build_hunspell_from_strings(
    affix: &str,
    dictionary: &str,
    output_directory: &Path,
    policy: &HunspellPolicy,
) -> Result<HunspellBuildSummary, BuilderError> {
    policy.validate()?;
    if output_directory.exists() {
        return Err(BuilderError::OutputExists {
            path: output_directory.to_path_buf(),
        });
    }
    validate_source_envelope(affix, dictionary)?;
    let (forms, forbidden_forms) = expand_forms(affix, dictionary)?;
    publish_forms(forms, forbidden_forms, output_directory, policy)
}

fn expand_forms(
    affix: &str,
    dictionary: &str,
) -> Result<(BTreeMap<String, BTreeSet<HunspellSourceClass>>, usize), BuilderError> {
    let parsed = zspell::builder()
        .config_str(affix)
        .dict_str(dictionary)
        .build()
        .map_err(|source| BuilderError::HunspellParse {
            message: source.to_string(),
        })?;

    let mut forms = BTreeMap::<String, BTreeSet<HunspellSourceClass>>::new();
    for form in parsed.wordlist().inner().keys() {
        forms
            .entry(form.to_string())
            .or_default()
            .insert(HunspellSourceClass::Accepted);
    }
    for form in parsed.wordlist_nosuggest().inner().keys() {
        forms
            .entry(form.to_string())
            .or_default()
            .insert(HunspellSourceClass::NoSuggest);
    }
    Ok((forms, parsed.wordlist_forbidden().inner().len()))
}

fn publish_forms(
    forms: BTreeMap<String, BTreeSet<HunspellSourceClass>>,
    forbidden_forms: usize,
    output_directory: &Path,
    policy: &HunspellPolicy,
) -> Result<HunspellBuildSummary, BuilderError> {
    let parent = output_directory
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| BuilderError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let staging = tempfile::Builder::new()
        .prefix(".word-arena-hunspell-build-")
        .tempdir_in(parent)
        .map_err(|source| BuilderError::Io {
            path: parent.to_path_buf(),
            source,
        })?;

    let (mut report, keys) = filter_forms(forms, forbidden_forms, staging.path(), policy)?;
    report.unique_keys = to_u64(keys.len(), "unique key count")?;
    validate_accounting(&report)?;
    let keys_path = staging.path().join(KEYS_FILE);
    write_keys(&keys_path, &keys)?;
    let report_path = staging.path().join(FILTER_REPORT_FILE);
    write_toml(&report_path, &report)?;
    let metadata = build_metadata(staging.path(), &report, policy)?;
    fs::rename(staging.path(), output_directory).map_err(|source| BuilderError::Io {
        path: output_directory.to_path_buf(),
        source,
    })?;

    Ok(HunspellBuildSummary {
        output_directory: output_directory.to_path_buf(),
        report,
        metadata,
    })
}

fn filter_forms(
    forms: BTreeMap<String, BTreeSet<HunspellSourceClass>>,
    forbidden_forms: usize,
    staging: &Path,
    policy: &HunspellPolicy,
) -> Result<(HunspellBuildReport, BTreeSet<String>), BuilderError> {
    let mut report = HunspellBuildReport {
        schema_version: REPORT_SCHEMA_VERSION,
        pack_id: policy.pack_id.clone(),
        locale: policy.locale.clone(),
        policy_id: policy.id.clone(),
        policy_version: policy.version,
        source_id: policy.source_id.clone(),
        source_revision: policy.source_revision.clone(),
        generated_forms: to_u64(forms.len(), "generated form count")?,
        forbidden_forms: to_u64(forbidden_forms, "forbidden form count")?,
        accepted_forms: 0,
        rejected_forms: 0,
        unique_keys: 0,
        duplicate_accepted_forms: 0,
        rejection_reasons: BTreeMap::new(),
    };
    let mut keys = BTreeSet::new();
    let audit_path = staging.join(AUDIT_FILE);
    let audit_file = File::create(&audit_path).map_err(|source| BuilderError::Io {
        path: audit_path.clone(),
        source,
    })?;
    let mut audit_writer = BufWriter::new(audit_file);

    for (source_form, classes) in forms {
        let outcome = classify_form(&source_form, policy);
        let (accepted, normalized_key, reason, duplicate_key) = match outcome {
            FormOutcome::Accepted(key) => {
                let duplicate = !keys.insert(key.clone());
                increment(&mut report.accepted_forms, "accepted form count")?;
                if duplicate {
                    increment(
                        &mut report.duplicate_accepted_forms,
                        "duplicate accepted form count",
                    )?;
                }
                (true, Some(key), None, duplicate)
            }
            FormOutcome::Rejected {
                reason,
                normalized_key,
            } => {
                increment(&mut report.rejected_forms, "rejected form count")?;
                increment(
                    report
                        .rejection_reasons
                        .entry(reason.as_str().to_owned())
                        .or_default(),
                    "rejection reason count",
                )?;
                (false, normalized_key, Some(reason), false)
            }
        };
        let record = HunspellAuditRecord {
            source_form,
            source_classes: classes.into_iter().collect(),
            normalized_key,
            accepted,
            reason,
            duplicate_key,
        };
        serde_json::to_writer(&mut audit_writer, &record)?;
        audit_writer
            .write_all(b"\n")
            .map_err(|source| BuilderError::Io {
                path: audit_path.clone(),
                source,
            })?;
    }
    audit_writer.flush().map_err(|source| BuilderError::Io {
        path: audit_path.clone(),
        source,
    })?;
    drop(audit_writer);

    Ok((report, keys))
}

fn build_metadata(
    staging: &Path,
    report: &HunspellBuildReport,
    policy: &HunspellPolicy,
) -> Result<BuildMetadata, BuilderError> {
    let keys_path = staging.join(KEYS_FILE);
    let audit_path = staging.join(AUDIT_FILE);
    let report_path = staging.join(FILTER_REPORT_FILE);
    let metadata = BuildMetadata {
        schema_version: METADATA_SCHEMA_VERSION,
        builder_name: BUILDER_NAME.to_owned(),
        builder_version: BUILDER_VERSION.to_owned(),
        pack_id: policy.pack_id.clone(),
        policy_id: policy.id.clone(),
        policy_version: policy.version,
        source_id: policy.source_id.clone(),
        source_revision: policy.source_revision.clone(),
        source_archive_sha256: policy.source_archive_sha256.clone(),
        normalization_profile: policy.normalization_profile.clone(),
        word_count: report.unique_keys,
        keys_sha256: sha256_file(&keys_path)?,
        audit_sha256: sha256_file(&audit_path)?,
        filter_report_sha256: sha256_file(&report_path)?,
    };
    write_toml(&staging.join(BUILD_METADATA_FILE), &metadata)?;
    Ok(metadata)
}

fn validate_source_envelope(affix: &str, dictionary: &str) -> Result<(), BuilderError> {
    if affix.contains('\0') || !affix.lines().map(str::trim).any(|line| line == "SET UTF-8") {
        return Err(BuilderError::HunspellParse {
            message: "affix data must be NUL-free and declare exactly SET UTF-8".to_owned(),
        });
    }
    if dictionary.contains('\0') {
        return Err(BuilderError::HunspellParse {
            message: "dictionary data must not contain NUL bytes".to_owned(),
        });
    }
    let mut lines = dictionary.lines();
    let declared = lines
        .next()
        .map(str::trim)
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or_else(|| BuilderError::HunspellParse {
            message: "dictionary first line must be an exact decimal entry count".to_owned(),
        })?;
    let actual = lines.filter(|line| !line.trim().is_empty()).count();
    if declared != actual {
        return Err(BuilderError::HunspellParse {
            message: format!(
                "dictionary declares {declared} entries but contains {actual} non-empty rows"
            ),
        });
    }
    Ok(())
}

fn classify_form(source: &str, policy: &HunspellPolicy) -> FormOutcome {
    if source.is_empty() {
        return rejected(HunspellRejectReason::Empty);
    }
    if source.contains(['\'', '\u{2019}', '\u{92}']) {
        return rejected(HunspellRejectReason::Apostrophe);
    }
    if source.contains('-') || source.chars().any(is_unicode_dash) {
        return rejected(HunspellRejectReason::Hyphen);
    }
    if source.chars().any(char::is_whitespace) {
        return rejected(HunspellRejectReason::Whitespace);
    }
    if source.chars().any(char::is_numeric) {
        return rejected(HunspellRejectReason::Digit);
    }
    if source
        .chars()
        .any(|character| character.is_ascii_punctuation())
    {
        return rejected(HunspellRejectReason::Punctuation);
    }
    let Ok(key) = normalize_key(&policy.normalization_profile, source) else {
        return rejected(HunspellRejectReason::UnsupportedCharacter);
    };
    let key = key.into_string();
    if !key
        .chars()
        .all(|character| policy.alphabet.contains(character))
    {
        return rejected(HunspellRejectReason::UnsupportedCharacter);
    }
    let key_length = key.chars().count();
    if key_length < policy.min_word_length {
        return FormOutcome::Rejected {
            reason: HunspellRejectReason::TooShort,
            normalized_key: Some(key),
        };
    }
    if key_length > policy.max_word_length {
        return FormOutcome::Rejected {
            reason: HunspellRejectReason::TooLong,
            normalized_key: Some(key),
        };
    }
    FormOutcome::Accepted(key)
}

fn is_unicode_dash(character: char) -> bool {
    matches!(
        character,
        '\u{058a}'
            | '\u{05be}'
            | '\u{1400}'
            | '\u{1806}'
            | '\u{2010}'..='\u{2015}'
            | '\u{2e17}'
            | '\u{2e1a}'
            | '\u{2e3a}'..='\u{2e3b}'
            | '\u{2e40}'
            | '\u{301c}'
            | '\u{3030}'
            | '\u{30a0}'
            | '\u{fe31}'..='\u{fe32}'
            | '\u{fe58}'
            | '\u{fe63}'
            | '\u{ff0d}'
    )
}

fn rejected(reason: HunspellRejectReason) -> FormOutcome {
    FormOutcome::Rejected {
        reason,
        normalized_key: None,
    }
}

fn validate_accounting(report: &HunspellBuildReport) -> Result<(), BuilderError> {
    let decided = report
        .accepted_forms
        .checked_add(report.rejected_forms)
        .ok_or_else(|| BuilderError::HunspellAccountingInvariant {
            message: "accepted + rejected form count overflow".to_owned(),
        })?;
    if decided != report.generated_forms {
        return Err(BuilderError::HunspellAccountingInvariant {
            message: format!(
                "{} generated forms != {} accepted + {} rejected",
                report.generated_forms, report.accepted_forms, report.rejected_forms
            ),
        });
    }
    if report.unique_keys > report.accepted_forms
        || report.duplicate_accepted_forms != report.accepted_forms - report.unique_keys
    {
        return Err(BuilderError::HunspellAccountingInvariant {
            message: "unique and duplicate accepted-form counts do not balance".to_owned(),
        });
    }
    let rejected_by_reason = report
        .rejection_reasons
        .values()
        .try_fold(0_u64, |total, count| total.checked_add(*count));
    if rejected_by_reason != Some(report.rejected_forms) {
        return Err(BuilderError::HunspellAccountingInvariant {
            message: "rejection reason counts do not equal rejected forms".to_owned(),
        });
    }
    Ok(())
}

fn write_keys(path: &Path, keys: &BTreeSet<String>) -> Result<(), BuilderError> {
    let file = File::create(path).map_err(|source| BuilderError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut writer = BufWriter::new(file);
    for key in keys {
        writer
            .write_all(key.as_bytes())
            .and_then(|()| writer.write_all(b"\n"))
            .map_err(|source| BuilderError::Io {
                path: path.to_path_buf(),
                source,
            })?;
    }
    writer.flush().map_err(|source| BuilderError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn write_toml<T: Serialize>(path: &Path, value: &T) -> Result<(), BuilderError> {
    let mut encoded = toml::to_string_pretty(value)?;
    if !encoded.ends_with('\n') {
        encoded.push('\n');
    }
    fs::write(path, encoded).map_err(|source| BuilderError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn increment(value: &mut u64, label: &str) -> Result<(), BuilderError> {
    *value = value
        .checked_add(1)
        .ok_or_else(|| BuilderError::HunspellAccountingInvariant {
            message: format!("{label} exceeds u64"),
        })?;
    Ok(())
}

fn to_u64(value: usize, label: &str) -> Result<u64, BuilderError> {
    u64::try_from(value).map_err(|_| BuilderError::HunspellAccountingInvariant {
        message: format!("{label} exceeds u64"),
    })
}
