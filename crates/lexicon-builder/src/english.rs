use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use word_arena_lexicon::normalize_key;

use crate::{
    AUDIT_FILE, BUILD_METADATA_FILE, BUILDER_NAME, BUILDER_VERSION, BuilderError, EnglishPolicy,
    FILTER_REPORT_FILE, KEYS_FILE, prepare_scowl_archive, util::sha256_file,
};

const REPORT_SCHEMA_VERSION: u32 = 1;
const METADATA_SCHEMA_VERSION: u32 = 1;

/// `SCOWLv1` source subcategory carried into audit output.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceClass {
    /// Normal words; the only potentially playable source class.
    Words,
    /// Abbreviations and acronyms.
    Abbreviations,
    /// Apostrophe-bearing contractions.
    Contractions,
    /// Proper-name source lists.
    ProperNames,
    /// Common uppercase words and names.
    Upper,
    /// Raw affix definitions, accepted only as a defensive audit class.
    Affixes,
    /// SCOWL special lists such as hacker terms and Roman numerals.
    Special,
}

/// Stable filter reason for a rejected source row.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    /// File level exceeds the configured size boundary.
    AboveScowlLevel,
    /// Normal words belong to an unselected regional/variant profile.
    UnselectedSpellingCategory,
    /// Source row belongs to an abbreviation list.
    AbbreviationClass,
    /// Source row belongs to a contraction list.
    ContractionClass,
    /// Source row belongs to a proper-name list.
    ProperNameClass,
    /// Source row belongs to an uppercase/name list.
    UpperClass,
    /// Source row belongs to a raw affix list.
    AffixClass,
    /// Source row belongs to a special list.
    SpecialClass,
    /// Source row is empty.
    Empty,
    /// A normal-word source row contains uppercase characters.
    UppercaseSource,
    /// A normal-word source row contains an apostrophe.
    Apostrophe,
    /// A normal-word source row contains a hyphen.
    Hyphen,
    /// A normal-word source row contains whitespace.
    Whitespace,
    /// A normal-word source row contains a digit.
    Digit,
    /// A normal-word source row contains punctuation.
    Punctuation,
    /// A source character cannot be represented by the board profile.
    UnsupportedCharacter,
    /// Normalized key is shorter than the configured minimum.
    TooShort,
    /// Normalized key is longer than the configured maximum.
    TooLong,
}

impl RejectReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::AboveScowlLevel => "above_scowl_level",
            Self::UnselectedSpellingCategory => "unselected_spelling_category",
            Self::AbbreviationClass => "abbreviation_class",
            Self::ContractionClass => "contraction_class",
            Self::ProperNameClass => "proper_name_class",
            Self::UpperClass => "upper_class",
            Self::AffixClass => "affix_class",
            Self::SpecialClass => "special_class",
            Self::Empty => "empty",
            Self::UppercaseSource => "uppercase_source",
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

/// Final decision written for every SCOWL source row.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditDecision {
    /// Row contributes a valid key occurrence.
    Accepted,
    /// Row is excluded with one stable reason.
    Rejected,
}

/// One deterministic source-row audit record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuditRecord {
    /// Relative SCOWL final filename.
    pub source_file: String,
    /// One-based line number within that file.
    pub source_line: u64,
    /// SCOWL spelling category.
    pub spelling_category: String,
    /// SCOWL subcategory.
    pub source_class: SourceClass,
    /// SCOWL size level.
    pub scowl_level: u8,
    /// Original ISO-8859-1 source form decoded to Unicode.
    pub original_form: String,
    /// Normalized board key when normalization completed.
    pub normalized_key: Option<String>,
    /// Accepted/rejected decision.
    pub decision: AuditDecision,
    /// Stable rejection reason, absent for accepted rows.
    pub reason: Option<RejectReason>,
    /// Whether another accepted row already produced this key.
    pub duplicate_key: bool,
}

/// Counts for one generated SCOWL classification file.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceFileReport {
    /// Source rows observed.
    pub source_rows: u64,
    /// Valid key occurrences.
    pub accepted_rows: u64,
    /// Rejected rows.
    pub rejected_rows: u64,
}

/// Complete row-accounting report for one English build.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildReport {
    /// Report schema version.
    pub schema_version: u32,
    /// Output pack ID.
    pub pack_id: String,
    /// Filter policy identity.
    pub policy_id: String,
    /// Filter policy version.
    pub policy_version: u32,
    /// Pinned source identity.
    pub source_id: String,
    /// Full source revision.
    pub source_revision: String,
    /// Inclusive SCOWL boundary.
    pub max_scowl_level: u8,
    /// Number of classified source files processed.
    pub source_files: u64,
    /// Every row read from those source files.
    pub source_rows: u64,
    /// Rows producing valid key occurrences, including duplicates.
    pub accepted_rows: u64,
    /// Rows excluded by policy.
    pub rejected_rows: u64,
    /// Unique sorted runtime keys.
    pub unique_keys: u64,
    /// Accepted occurrences whose key already existed.
    pub duplicate_accepted_rows: u64,
    /// Rejection totals keyed by stable reason.
    pub rejection_reasons: BTreeMap<String, u64>,
    /// Row totals keyed by source filename.
    pub source_file_counts: BTreeMap<String, SourceFileReport>,
}

/// Reproducibility metadata and checksums for generated build outputs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildMetadata {
    /// Metadata schema version.
    pub schema_version: u32,
    /// Builder package name.
    pub builder_name: String,
    /// Builder semantic version.
    pub builder_version: String,
    /// Pack ID.
    pub pack_id: String,
    /// Filter policy identity.
    pub policy_id: String,
    /// Filter policy version.
    pub policy_version: u32,
    /// Pinned source registry ID.
    pub source_id: String,
    /// Full pinned revision.
    pub source_revision: String,
    /// Pinned source archive SHA-256.
    pub source_archive_sha256: String,
    /// Normalization profile identity.
    pub normalization_profile: String,
    /// Unique runtime key count.
    pub word_count: u64,
    /// `keys.txt` SHA-256.
    pub keys_sha256: String,
    /// `audit.jsonl` SHA-256.
    pub audit_sha256: String,
    /// `filter-report.toml` SHA-256.
    pub filter_report_sha256: String,
}

/// Result of a successfully published deterministic build directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildSummary {
    /// Final output directory.
    pub output_directory: PathBuf,
    /// Row-accounting report.
    pub report: BuildReport,
    /// Reproducibility metadata.
    pub metadata: BuildMetadata,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ScowlInput {
    path: PathBuf,
    source_file: String,
    spelling_category: String,
    source_class: SourceClass,
    level: u8,
}

enum RowOutcome {
    Accepted(String),
    Rejected {
        reason: RejectReason,
        normalized_key: Option<String>,
    },
}

/// Builds deterministic English outputs from a pinned `SCOWLv1` source archive.
///
/// # Errors
///
/// Returns [`BuilderError`] when source preparation, policy enforcement,
/// accounting, serialization, or atomic publication fails.
pub fn build_english_from_archive(
    archive_path: &Path,
    output_directory: &Path,
    policy: &EnglishPolicy,
) -> Result<BuildSummary, BuilderError> {
    let workspace = TempDir::new().map_err(|source| BuilderError::Io {
        path: std::env::temp_dir(),
        source,
    })?;
    let prepared = prepare_scowl_archive(archive_path, workspace.path(), policy)?;
    build_english_from_final(prepared.final_directory(), output_directory, policy)
}

/// Builds deterministic English outputs from SCOWL's generated `final/` files.
///
/// The output directory is atomically renamed from a temporary sibling after
/// every row is audited and all accounting invariants pass.
///
/// # Errors
///
/// Returns [`BuilderError`] for invalid policy/input, unknown files, I/O,
/// serialization, accounting failure, or an existing destination.
pub fn build_english_from_final(
    final_directory: &Path,
    output_directory: &Path,
    policy: &EnglishPolicy,
) -> Result<BuildSummary, BuilderError> {
    policy.validate()?;
    if !final_directory.is_dir() {
        return Err(BuilderError::MissingFinalDirectory {
            path: final_directory.to_path_buf(),
        });
    }
    if output_directory.exists() {
        return Err(BuilderError::OutputExists {
            path: output_directory.to_path_buf(),
        });
    }
    let parent = output_directory
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| BuilderError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let staging = tempfile::Builder::new()
        .prefix(".word-arena-en-build-")
        .tempdir_in(parent)
        .map_err(|source| BuilderError::Io {
            path: parent.to_path_buf(),
            source,
        })?;

    let inputs = discover_inputs(final_directory)?;
    let mut report = BuildReport {
        schema_version: REPORT_SCHEMA_VERSION,
        pack_id: policy.pack_id.clone(),
        policy_id: policy.id.clone(),
        policy_version: policy.version,
        source_id: policy.source_id.clone(),
        source_revision: policy.source_revision.clone(),
        max_scowl_level: policy.max_scowl_level,
        source_files: u64::try_from(inputs.len()).map_err(|_| {
            BuilderError::AccountingInvariant {
                message: "input file count exceeds u64".to_owned(),
            }
        })?,
        source_rows: 0,
        accepted_rows: 0,
        rejected_rows: 0,
        unique_keys: 0,
        duplicate_accepted_rows: 0,
        rejection_reasons: BTreeMap::new(),
        source_file_counts: BTreeMap::new(),
    };
    let mut keys = BTreeSet::new();
    let audit_path = staging.path().join(AUDIT_FILE);
    let audit_file = File::create(&audit_path).map_err(|source| BuilderError::Io {
        path: audit_path.clone(),
        source,
    })?;
    let mut audit_writer = BufWriter::new(audit_file);

    for input in &inputs {
        process_input(input, policy, &mut keys, &mut report, &mut audit_writer)?;
    }
    audit_writer.flush().map_err(|source| BuilderError::Io {
        path: audit_path.clone(),
        source,
    })?;
    drop(audit_writer);

    report.unique_keys =
        u64::try_from(keys.len()).map_err(|_| BuilderError::AccountingInvariant {
            message: "unique key count exceeds u64".to_owned(),
        })?;
    validate_accounting(&report)?;

    let keys_path = staging.path().join(KEYS_FILE);
    write_keys(&keys_path, &keys)?;
    let report_path = staging.path().join(FILTER_REPORT_FILE);
    write_toml(&report_path, &report)?;

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
    write_toml(&staging.path().join(BUILD_METADATA_FILE), &metadata)?;

    fs::rename(staging.path(), output_directory).map_err(|source| BuilderError::Io {
        path: output_directory.to_path_buf(),
        source,
    })?;

    Ok(BuildSummary {
        output_directory: output_directory.to_path_buf(),
        report,
        metadata,
    })
}

fn discover_inputs(final_directory: &Path) -> Result<Vec<ScowlInput>, BuilderError> {
    let entries = fs::read_dir(final_directory).map_err(|source| BuilderError::Io {
        path: final_directory.to_path_buf(),
        source,
    })?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| BuilderError::Io {
            path: final_directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| BuilderError::Io {
            path: path.clone(),
            source,
        })?;
        if !file_type.is_file() {
            return Err(BuilderError::UnexpectedInputFile { path });
        }
        paths.push(path);
    }
    paths.sort_unstable_by(|left, right| left.as_os_str().cmp(right.as_os_str()));
    paths.into_iter().map(parse_input).collect()
}

fn parse_input(path: PathBuf) -> Result<ScowlInput, BuilderError> {
    let Some(source_file) = path
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
    else {
        return Err(BuilderError::UnexpectedInputFile { path });
    };
    let Some((stem, level)) = source_file.rsplit_once('.') else {
        return Err(BuilderError::UnexpectedInputFile { path });
    };
    if level.len() != 2 || !level.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(BuilderError::UnexpectedInputFile { path });
    }
    let level = level
        .parse::<u8>()
        .map_err(|_| BuilderError::UnexpectedInputFile { path: path.clone() })?;
    let (spelling_category, source_class) = if stem.starts_with("special-") {
        ("special", SourceClass::Special)
    } else {
        [
            ("-proper-names", SourceClass::ProperNames),
            ("-abbreviations", SourceClass::Abbreviations),
            ("-contractions", SourceClass::Contractions),
            ("-affixes", SourceClass::Affixes),
            ("-upper", SourceClass::Upper),
            ("-words", SourceClass::Words),
        ]
        .into_iter()
        .find_map(|(suffix, source_class)| {
            stem.strip_suffix(suffix)
                .map(|category| (category, source_class))
        })
        .filter(|(category, _)| !category.is_empty())
        .ok_or_else(|| BuilderError::UnexpectedInputFile { path: path.clone() })?
    };
    let spelling_category = spelling_category.to_owned();

    Ok(ScowlInput {
        path,
        source_file,
        spelling_category,
        source_class,
        level,
    })
}

fn process_input(
    input: &ScowlInput,
    policy: &EnglishPolicy,
    keys: &mut BTreeSet<String>,
    report: &mut BuildReport,
    audit_writer: &mut BufWriter<File>,
) -> Result<(), BuilderError> {
    let file = File::open(&input.path).map_err(|source| BuilderError::Io {
        path: input.path.clone(),
        source,
    })?;
    let mut reader = BufReader::new(file);
    report
        .source_file_counts
        .entry(input.source_file.clone())
        .or_default();
    let mut source_line = 0_u64;
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        let read = reader
            .read_until(b'\n', &mut bytes)
            .map_err(|source| BuilderError::Io {
                path: input.path.clone(),
                source,
            })?;
        if read == 0 {
            break;
        }
        source_line =
            source_line
                .checked_add(1)
                .ok_or_else(|| BuilderError::AccountingInvariant {
                    message: format!("line count overflow in {}", input.source_file),
                })?;
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
        }
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        let original_form = decode_iso_8859_1(&bytes);
        let outcome = classify_row(input, &original_form, policy);
        let (decision, reason, normalized_key, duplicate_key) = match outcome {
            RowOutcome::Accepted(key) => {
                let duplicate = !keys.insert(key.clone());
                increment(&mut report.accepted_rows, "accepted row count")?;
                if duplicate {
                    increment(
                        &mut report.duplicate_accepted_rows,
                        "duplicate accepted row count",
                    )?;
                }
                (AuditDecision::Accepted, None, Some(key), duplicate)
            }
            RowOutcome::Rejected {
                reason,
                normalized_key,
            } => {
                increment(&mut report.rejected_rows, "rejected row count")?;
                increment(
                    report
                        .rejection_reasons
                        .entry(reason.as_str().to_owned())
                        .or_default(),
                    "rejection reason count",
                )?;
                (AuditDecision::Rejected, Some(reason), normalized_key, false)
            }
        };
        increment(&mut report.source_rows, "source row count")?;
        let file_report = report
            .source_file_counts
            .get_mut(&input.source_file)
            .ok_or_else(|| BuilderError::AccountingInvariant {
                message: format!("missing report entry for {}", input.source_file),
            })?;
        increment(&mut file_report.source_rows, "source file row count")?;
        match decision {
            AuditDecision::Accepted => {
                increment(&mut file_report.accepted_rows, "source file accepted count")?;
            }
            AuditDecision::Rejected => {
                increment(&mut file_report.rejected_rows, "source file rejected count")?;
            }
        }

        let record = AuditRecord {
            source_file: input.source_file.clone(),
            source_line,
            spelling_category: input.spelling_category.clone(),
            source_class: input.source_class,
            scowl_level: input.level,
            original_form,
            normalized_key,
            decision,
            reason,
            duplicate_key,
        };
        serde_json::to_writer(&mut *audit_writer, &record)?;
        audit_writer
            .write_all(b"\n")
            .map_err(|source| BuilderError::Io {
                path: PathBuf::from(AUDIT_FILE),
                source,
            })?;
    }
    Ok(())
}

fn classify_row(input: &ScowlInput, source: &str, policy: &EnglishPolicy) -> RowOutcome {
    if input.level > policy.max_scowl_level {
        return rejected(RejectReason::AboveScowlLevel);
    }
    let class_reason = match input.source_class {
        SourceClass::Words => None,
        SourceClass::Abbreviations => Some(RejectReason::AbbreviationClass),
        SourceClass::Contractions => Some(RejectReason::ContractionClass),
        SourceClass::ProperNames => Some(RejectReason::ProperNameClass),
        SourceClass::Upper => Some(RejectReason::UpperClass),
        SourceClass::Affixes => Some(RejectReason::AffixClass),
        SourceClass::Special => Some(RejectReason::SpecialClass),
    };
    if let Some(reason) = class_reason {
        return rejected(reason);
    }
    if !policy.selects_spelling_category(&input.spelling_category) {
        return rejected(RejectReason::UnselectedSpellingCategory);
    }
    if source.is_empty() {
        return rejected(RejectReason::Empty);
    }
    if source.chars().any(char::is_uppercase) {
        return rejected(RejectReason::UppercaseSource);
    }
    if source.contains(['\'', '\u{2019}', '\u{92}']) {
        return rejected(RejectReason::Apostrophe);
    }
    if source.contains('-') {
        return rejected(RejectReason::Hyphen);
    }
    if source.chars().any(char::is_whitespace) {
        return rejected(RejectReason::Whitespace);
    }
    if source.chars().any(char::is_numeric) {
        return rejected(RejectReason::Digit);
    }
    if source
        .chars()
        .any(|character| character.is_ascii_punctuation())
    {
        return rejected(RejectReason::Punctuation);
    }

    let Ok(key) = normalize_key(&policy.normalization_profile, source) else {
        return rejected(RejectReason::UnsupportedCharacter);
    };
    let key = key.into_string();
    if !key
        .chars()
        .all(|character| policy.alphabet.contains(character))
    {
        return rejected(RejectReason::UnsupportedCharacter);
    }
    let key_length = key.chars().count();
    if key_length < policy.min_word_length {
        return RowOutcome::Rejected {
            reason: RejectReason::TooShort,
            normalized_key: Some(key),
        };
    }
    if key_length > policy.max_word_length {
        return RowOutcome::Rejected {
            reason: RejectReason::TooLong,
            normalized_key: Some(key),
        };
    }
    RowOutcome::Accepted(key)
}

fn rejected(reason: RejectReason) -> RowOutcome {
    RowOutcome::Rejected {
        reason,
        normalized_key: None,
    }
}

fn decode_iso_8859_1(bytes: &[u8]) -> String {
    bytes.iter().copied().map(char::from).collect()
}

fn validate_accounting(report: &BuildReport) -> Result<(), BuilderError> {
    let decided = report
        .accepted_rows
        .checked_add(report.rejected_rows)
        .ok_or_else(|| BuilderError::AccountingInvariant {
            message: "accepted + rejected row count overflow".to_owned(),
        })?;
    if decided != report.source_rows {
        return Err(BuilderError::AccountingInvariant {
            message: format!(
                "{} source rows != {} accepted + {} rejected",
                report.source_rows, report.accepted_rows, report.rejected_rows
            ),
        });
    }
    if report.unique_keys > report.accepted_rows {
        return Err(BuilderError::AccountingInvariant {
            message: "unique keys exceed accepted source rows".to_owned(),
        });
    }
    let expected_duplicates = report.accepted_rows - report.unique_keys;
    if report.duplicate_accepted_rows != expected_duplicates {
        return Err(BuilderError::AccountingInvariant {
            message: format!(
                "{} duplicate rows != accepted rows minus unique keys ({expected_duplicates})",
                report.duplicate_accepted_rows
            ),
        });
    }
    for (source_file, counts) in &report.source_file_counts {
        if counts.accepted_rows + counts.rejected_rows != counts.source_rows {
            return Err(BuilderError::AccountingInvariant {
                message: format!("per-file counts do not balance for {source_file}"),
            });
        }
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

fn write_toml<T>(path: &Path, value: &T) -> Result<(), BuilderError>
where
    T: Serialize,
{
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
        .ok_or_else(|| BuilderError::AccountingInvariant {
            message: format!("{label} exceeds u64"),
        })?;
    Ok(())
}
