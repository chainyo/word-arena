use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

use quick_xml::{
    Reader, XmlVersion,
    encoding::Decoder,
    escape::resolve_predefined_entity,
    events::{BytesCData, BytesRef, BytesStart, BytesText, Event},
};
use serde::{Deserialize, Serialize};
use word_arena_lexicon::normalize_key;
use zip::{ZipArchive, result::ZipError};

use crate::{
    AUDIT_FILE, AuditDecision, BUILD_METADATA_FILE, BUILDER_NAME, BUILDER_VERSION, BuildMetadata,
    BuilderError, FILTER_REPORT_FILE, FrenchPolicy, KEYS_FILE, util::sha256_file,
};

const REPORT_SCHEMA_VERSION: u32 = 1;
const XML_BUFFER_SIZE: usize = 64 * 1024;

/// Kind of Morphalou source form represented by one audit row.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrenchFormKind {
    /// Canonical uninflected form for a lexical entry.
    Lemma,
    /// Declined or conjugated form associated with the lemma.
    Inflected,
}

/// Stable filter reason for a rejected Morphalou source form.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrenchRejectReason {
    /// The lexical entry has no grammatical category.
    MissingCategory,
    /// The lexical entry uses a category outside the standard V1 profile.
    UnsupportedCategory,
    /// The source explicitly classifies the entry as a proper name.
    ProperName,
    /// The source explicitly classifies the entry as an abbreviation.
    Abbreviation,
    /// The source explicitly marks the entry as a locution.
    Locution,
    /// A spelling variant has no standard evidence outside the excluded origin.
    NonstandardVariant,
    /// The source form is empty.
    Empty,
    /// The source form contains an apostrophe.
    Apostrophe,
    /// The source form contains a hyphen or dash.
    Hyphen,
    /// The source form contains whitespace.
    Whitespace,
    /// The source form contains a digit.
    Digit,
    /// The source form contains punctuation.
    Punctuation,
    /// The source cannot be represented by the French board-key profile.
    UnsupportedCharacter,
    /// The normalized key is shorter than the configured minimum.
    TooShort,
    /// The normalized key is longer than the configured maximum.
    TooLong,
}

impl FrenchRejectReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::MissingCategory => "missing_category",
            Self::UnsupportedCategory => "unsupported_category",
            Self::ProperName => "proper_name",
            Self::Abbreviation => "abbreviation",
            Self::Locution => "locution",
            Self::NonstandardVariant => "nonstandard_variant",
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

/// One deterministic source-form audit record from Morphalou LMF data.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FrenchAuditRecord {
    /// One-based lexical-entry position in the source XML.
    pub lexical_entry_index: u64,
    /// Source lexical-entry ID.
    pub lexical_entry_id: String,
    /// Whether the row is a lemma or inflected form.
    pub form_kind: FrenchFormKind,
    /// One-based position among forms of the same kind in this entry.
    pub form_index: u64,
    /// Original accented UTF-8 source form.
    pub original_form: String,
    /// Morphalou grammatical category, absent when the source field is empty.
    pub grammatical_category: Option<String>,
    /// Morphalou grammatical subcategory when present.
    pub grammatical_subcategory: Option<String>,
    /// Whether the source entry carries the configured locution marker.
    pub locution: bool,
    /// Referenced base lemma for a spelling variant.
    pub spelling_variant_of: Option<String>,
    /// Origins supporting a spelling variant, omitted for non-variants.
    pub variant_origins: Option<Vec<String>>,
    /// Whether the executable evidence rule classified this variant as nonstandard.
    pub nonstandard_variant: bool,
    /// Normalized board key when normalization completed.
    pub normalized_key: Option<String>,
    /// Accepted/rejected decision.
    pub decision: AuditDecision,
    /// Stable rejection reason, absent for accepted rows.
    pub reason: Option<FrenchRejectReason>,
    /// Whether another accepted row already produced this board key.
    pub duplicate_key: bool,
}

/// Complete row-accounting report for one French source build.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FrenchBuildReport {
    /// Report schema version.
    pub schema_version: u32,
    /// Output pack ID.
    pub pack_id: String,
    /// Filter-policy identity.
    pub policy_id: String,
    /// Filter-policy version.
    pub policy_version: u32,
    /// Pinned source identity.
    pub source_id: String,
    /// Full source revision/version.
    pub source_revision: String,
    /// Lexical entries observed.
    pub source_entries: u64,
    /// Lexical-entry fragments intentionally disabled inside XML comments.
    pub inactive_commented_entries: u64,
    /// Lemma and inflection rows observed.
    pub source_rows: u64,
    /// Orthography fragments inside intentionally inactive XML comments.
    pub inactive_commented_form_rows: u64,
    /// Lemma rows observed.
    pub lemma_rows: u64,
    /// Inflected-form rows observed.
    pub inflected_rows: u64,
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
}

/// Result of a successfully published deterministic French build directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrenchBuildSummary {
    /// Final output directory.
    pub output_directory: PathBuf,
    /// Row-accounting report.
    pub report: FrenchBuildReport,
    /// Reproducibility metadata.
    pub metadata: BuildMetadata,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct PendingEntry {
    id: String,
    grammatical_category: Option<String>,
    grammatical_subcategory: Option<String>,
    locution_value: Option<String>,
    spelling_variant_of: Option<String>,
    lemma_origins: BTreeSet<String>,
    forms: Vec<PendingForm>,
    lemma_forms: u64,
    inflected_forms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingForm {
    kind: FrenchFormKind,
    index: u64,
    original_form: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CaptureField {
    Orthography,
    GrammaticalCategory,
    GrammaticalSubcategory,
    Locution,
    SpellingVariantOf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Capture {
    field: CaptureField,
    value: String,
}

enum FormOutcome {
    Accepted(String),
    Rejected {
        reason: FrenchRejectReason,
        normalized_key: Option<String>,
    },
}

/// Builds deterministic French outputs directly from the pinned Morphalou ZIP.
///
/// # Errors
///
/// Returns [`BuilderError`] when archive integrity, XML parsing, policy,
/// accounting, serialization, or atomic publication fails.
pub fn build_french_from_archive(
    archive_path: &Path,
    output_directory: &Path,
    policy: &FrenchPolicy,
) -> Result<FrenchBuildSummary, BuilderError> {
    policy.validate()?;
    validate_archive(archive_path, policy)?;
    let archive_file = File::open(archive_path).map_err(|source| BuilderError::Io {
        path: archive_path.to_path_buf(),
        source,
    })?;
    let mut archive =
        ZipArchive::new(archive_file).map_err(|source| BuilderError::MorphalouZip {
            path: archive_path.to_path_buf(),
            source,
        })?;
    let xml = match archive.by_name(&policy.source_archive_data_path) {
        Ok(xml) => xml,
        Err(ZipError::FileNotFound) => {
            return Err(BuilderError::MissingMorphalouData {
                archive: archive_path.to_path_buf(),
                expected: policy.source_archive_data_path.clone(),
            });
        }
        Err(source) => {
            return Err(BuilderError::MorphalouZip {
                path: archive_path.to_path_buf(),
                source,
            });
        }
    };
    if xml.size() != policy.source_data_size_bytes {
        return Err(BuilderError::MorphalouDataSizeMismatch {
            member: policy.source_archive_data_path.clone(),
            expected: policy.source_data_size_bytes,
            actual: xml.size(),
        });
    }
    build_french_from_reader(
        BufReader::with_capacity(XML_BUFFER_SIZE, xml),
        output_directory,
        policy,
    )
}

/// Builds deterministic French outputs from an unpacked Morphalou LMF XML file.
///
/// This entry point exists for source fixtures and reproducibility diagnostics;
/// release builds must use [`build_french_from_archive`] to enforce the pin.
///
/// # Errors
///
/// Returns [`BuilderError`] for invalid policy/XML, I/O, accounting,
/// serialization, or an existing output destination.
pub fn build_french_from_xml(
    xml_path: &Path,
    output_directory: &Path,
    policy: &FrenchPolicy,
) -> Result<FrenchBuildSummary, BuilderError> {
    policy.validate()?;
    let xml = File::open(xml_path).map_err(|source| BuilderError::Io {
        path: xml_path.to_path_buf(),
        source,
    })?;
    build_french_from_reader(
        BufReader::with_capacity(XML_BUFFER_SIZE, xml),
        output_directory,
        policy,
    )
}

fn build_french_from_reader<R: BufRead>(
    source: R,
    output_directory: &Path,
    policy: &FrenchPolicy,
) -> Result<FrenchBuildSummary, BuilderError> {
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
        .prefix(".word-arena-fr-build-")
        .tempdir_in(parent)
        .map_err(|source| BuilderError::Io {
            path: parent.to_path_buf(),
            source,
        })?;

    let mut report = FrenchBuildReport {
        schema_version: REPORT_SCHEMA_VERSION,
        pack_id: policy.pack_id.clone(),
        policy_id: policy.id.clone(),
        policy_version: policy.version,
        source_id: policy.source_id.clone(),
        source_revision: policy.source_revision.clone(),
        source_entries: 0,
        inactive_commented_entries: 0,
        source_rows: 0,
        inactive_commented_form_rows: 0,
        lemma_rows: 0,
        inflected_rows: 0,
        accepted_rows: 0,
        rejected_rows: 0,
        unique_keys: 0,
        duplicate_accepted_rows: 0,
        rejection_reasons: BTreeMap::new(),
    };
    let mut keys = BTreeSet::new();
    let audit_path = staging.path().join(AUDIT_FILE);
    let audit_file = File::create(&audit_path).map_err(|source| BuilderError::Io {
        path: audit_path.clone(),
        source,
    })?;
    let mut audit_writer = BufWriter::new(audit_file);

    parse_morphalou(source, policy, &mut keys, &mut report, &mut audit_writer)?;
    audit_writer.flush().map_err(|source| BuilderError::Io {
        path: audit_path.clone(),
        source,
    })?;
    drop(audit_writer);

    report.unique_keys =
        u64::try_from(keys.len()).map_err(|_| BuilderError::FrenchAccountingInvariant {
            message: "unique key count exceeds u64".to_owned(),
        })?;
    validate_accounting(&report)?;

    let keys_path = staging.path().join(KEYS_FILE);
    write_keys(&keys_path, &keys)?;
    let report_path = staging.path().join(FILTER_REPORT_FILE);
    write_toml(&report_path, &report)?;
    let metadata = BuildMetadata {
        schema_version: 1,
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
    Ok(FrenchBuildSummary {
        output_directory: output_directory.to_path_buf(),
        report,
        metadata,
    })
}

fn parse_morphalou<R: BufRead>(
    source: R,
    policy: &FrenchPolicy,
    keys: &mut BTreeSet<String>,
    report: &mut FrenchBuildReport,
    audit_writer: &mut BufWriter<File>,
) -> Result<(), BuilderError> {
    let mut reader = Reader::from_reader(source);
    let mut buffer = Vec::new();
    let mut state = ParserState::default();

    loop {
        let event =
            reader
                .read_event_into(&mut buffer)
                .map_err(|source| BuilderError::MorphalouXml {
                    position: reader.error_position(),
                    message: source.to_string(),
                })?;
        let position = reader.buffer_position();
        match event {
            Event::Start(start) => state.handle_start(&start, reader.decoder(), position)?,
            Event::Empty(empty) => state.handle_empty(&empty, reader.decoder(), position)?,
            Event::Text(text) => append_text(&mut state.capture, &text, position)?,
            Event::CData(text) => append_cdata(&mut state.capture, &text, position)?,
            Event::GeneralRef(reference) => {
                append_reference(&mut state.capture, &reference, position)?;
            }
            Event::End(end) => state.handle_end(
                end.name().as_ref(),
                position,
                policy,
                keys,
                report,
                audit_writer,
            )?,
            Event::Comment(comment) => record_inactive_comment(&comment, position, report)?,
            Event::Eof => break,
            Event::Decl(_) | Event::PI(_) | Event::DocType(_) => {}
        }
        buffer.clear();
    }
    state.finish(reader.buffer_position())
}

#[derive(Default)]
struct ParserState {
    entry: Option<PendingEntry>,
    form_kind: Option<FrenchFormKind>,
    capture: Option<Capture>,
    saw_lexicon: bool,
}

impl ParserState {
    fn handle_start(
        &mut self,
        start: &BytesStart<'_>,
        decoder: Decoder,
        position: u64,
    ) -> Result<(), BuilderError> {
        match start.name().as_ref() {
            b"lexicon" => {
                if self.saw_lexicon {
                    return xml_error(position, "multiple lexicon roots");
                }
                self.saw_lexicon = true;
            }
            b"lexicalEntry" => {
                if self.entry.is_some() {
                    return xml_error(position, "nested lexicalEntry elements");
                }
                self.entry = Some(PendingEntry {
                    id: required_attribute(start, b"id", decoder, position)?,
                    ..PendingEntry::default()
                });
            }
            b"lemmatizedForm" => self.begin_source_form(FrenchFormKind::Lemma, position)?,
            b"inflectedForm" => self.begin_source_form(FrenchFormKind::Inflected, position)?,
            b"orthography" => {
                if self.form_kind.is_none() {
                    return xml_error(position, "orthography outside a recognized form");
                }
                begin_capture(&mut self.capture, CaptureField::Orthography, position)?;
            }
            b"grammaticalCategory" if self.in_lemma() => begin_capture(
                &mut self.capture,
                CaptureField::GrammaticalCategory,
                position,
            )?,
            b"grammaticalSubCategory" if self.in_lemma() => begin_capture(
                &mut self.capture,
                CaptureField::GrammaticalSubcategory,
                position,
            )?,
            b"locution" if self.in_lemma() => {
                begin_capture(&mut self.capture, CaptureField::Locution, position)?;
            }
            b"spellingVariantOf" => {
                require_entry(self.entry.as_ref(), position)?;
                begin_capture(&mut self.capture, CaptureField::SpellingVariantOf, position)?;
            }
            b"originatingEntry" if self.in_lemma() => {
                collect_origin(start, decoder, self.entry.as_mut(), position)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_empty(
        &mut self,
        empty: &BytesStart<'_>,
        decoder: Decoder,
        position: u64,
    ) -> Result<(), BuilderError> {
        if empty.name().as_ref() == b"originatingEntry" && self.in_lemma() {
            collect_origin(empty, decoder, self.entry.as_mut(), position)?;
        }
        Ok(())
    }

    fn handle_end(
        &mut self,
        name: &[u8],
        position: u64,
        policy: &FrenchPolicy,
        keys: &mut BTreeSet<String>,
        report: &mut FrenchBuildReport,
        audit_writer: &mut BufWriter<File>,
    ) -> Result<(), BuilderError> {
        match name {
            b"orthography" => self.complete_orthography(position)?,
            b"grammaticalCategory" if self.in_lemma() => {
                self.complete_metadata(CaptureField::GrammaticalCategory, position)?;
            }
            b"grammaticalSubCategory" if self.in_lemma() => {
                self.complete_metadata(CaptureField::GrammaticalSubcategory, position)?;
            }
            b"locution" if self.in_lemma() => {
                self.complete_metadata(CaptureField::Locution, position)?;
            }
            b"spellingVariantOf" => {
                self.complete_metadata(CaptureField::SpellingVariantOf, position)?;
            }
            b"lemmatizedForm" => {
                end_form(&mut self.form_kind, FrenchFormKind::Lemma, position)?;
            }
            b"inflectedForm" => {
                end_form(&mut self.form_kind, FrenchFormKind::Inflected, position)?;
            }
            b"lexicalEntry" => {
                if self.form_kind.is_some() || self.capture.is_some() {
                    return xml_error(position, "lexicalEntry ended with an open form field");
                }
                let completed = self
                    .entry
                    .take()
                    .ok_or_else(|| BuilderError::MorphalouXml {
                        position,
                        message: "lexicalEntry end without a start".to_owned(),
                    })?;
                process_entry(&completed, policy, keys, report, audit_writer)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn begin_source_form(
        &mut self,
        kind: FrenchFormKind,
        position: u64,
    ) -> Result<(), BuilderError> {
        require_entry(self.entry.as_ref(), position)?;
        begin_form(&mut self.form_kind, kind, position)
    }

    fn complete_orthography(&mut self, position: u64) -> Result<(), BuilderError> {
        let original_form = finish_capture(&mut self.capture, CaptureField::Orthography, position)?;
        let kind = self.form_kind.ok_or_else(|| BuilderError::MorphalouXml {
            position,
            message: "orthography ended outside a form".to_owned(),
        })?;
        push_form(self.entry.as_mut(), kind, original_form, position)
    }

    fn complete_metadata(
        &mut self,
        field: CaptureField,
        position: u64,
    ) -> Result<(), BuilderError> {
        let value = nonempty(finish_capture(&mut self.capture, field, position)?);
        let entry = require_entry_mut(self.entry.as_mut(), position)?;
        let (destination, name) = match field {
            CaptureField::GrammaticalCategory => {
                (&mut entry.grammatical_category, "grammaticalCategory")
            }
            CaptureField::GrammaticalSubcategory => {
                (&mut entry.grammatical_subcategory, "grammaticalSubCategory")
            }
            CaptureField::Locution => (&mut entry.locution_value, "locution"),
            CaptureField::SpellingVariantOf => {
                (&mut entry.spelling_variant_of, "spellingVariantOf")
            }
            CaptureField::Orthography => {
                return xml_error(position, "orthography is not entry metadata");
            }
        };
        assign_once(destination, value, name, position)
    }

    fn in_lemma(&self) -> bool {
        self.form_kind == Some(FrenchFormKind::Lemma)
    }

    fn finish(self, position: u64) -> Result<(), BuilderError> {
        if !self.saw_lexicon {
            return xml_error(position, "missing lexicon root");
        }
        if self.entry.is_some() || self.form_kind.is_some() || self.capture.is_some() {
            return xml_error(position, "unexpected EOF inside a lexical entry");
        }
        Ok(())
    }
}

fn process_entry(
    entry: &PendingEntry,
    policy: &FrenchPolicy,
    keys: &mut BTreeSet<String>,
    report: &mut FrenchBuildReport,
    audit_writer: &mut BufWriter<File>,
) -> Result<(), BuilderError> {
    if entry.lemma_forms != 1 {
        return Err(BuilderError::MorphalouXml {
            position: 0,
            message: format!(
                "lexical entry {:?} has {} lemma orthographies; expected exactly one",
                entry.id, entry.lemma_forms
            ),
        });
    }
    increment(&mut report.source_entries, "source entry count")?;
    let entry_index = report.source_entries;
    let locution =
        meaningful(entry.locution_value.as_ref()) == Some(policy.locution_value.as_str());
    let nonstandard_variant =
        policy.is_nonstandard_variant(entry.spelling_variant_of.is_some(), &entry.lemma_origins);
    let variant_origins = entry
        .spelling_variant_of
        .as_ref()
        .map(|_| entry.lemma_origins.iter().cloned().collect::<Vec<_>>());

    for form in &entry.forms {
        increment(&mut report.source_rows, "source row count")?;
        match form.kind {
            FrenchFormKind::Lemma => increment(&mut report.lemma_rows, "lemma row count")?,
            FrenchFormKind::Inflected => {
                increment(&mut report.inflected_rows, "inflected row count")?;
            }
        }
        let outcome = classify_form(entry, &form.original_form, policy, nonstandard_variant);
        let (decision, reason, normalized_key, duplicate_key) = match outcome {
            FormOutcome::Accepted(key) => {
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
            FormOutcome::Rejected {
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
        let record = FrenchAuditRecord {
            lexical_entry_index: entry_index,
            lexical_entry_id: entry.id.clone(),
            form_kind: form.kind,
            form_index: form.index,
            original_form: form.original_form.clone(),
            grammatical_category: meaningful_owned(entry.grammatical_category.as_ref()),
            grammatical_subcategory: meaningful_owned(entry.grammatical_subcategory.as_ref()),
            locution,
            spelling_variant_of: meaningful_owned(entry.spelling_variant_of.as_ref()),
            variant_origins: variant_origins.clone(),
            nonstandard_variant,
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

fn classify_form(
    entry: &PendingEntry,
    source: &str,
    policy: &FrenchPolicy,
    nonstandard_variant: bool,
) -> FormOutcome {
    let Some(category) = meaningful(entry.grammatical_category.as_ref()) else {
        return rejected(FrenchRejectReason::MissingCategory);
    };
    if policy.is_proper_name_category(category) {
        return rejected(FrenchRejectReason::ProperName);
    }
    if !policy.accepts_grammatical_category(category) {
        return rejected(FrenchRejectReason::UnsupportedCategory);
    }
    if meaningful(entry.grammatical_subcategory.as_ref())
        == Some(policy.abbreviation_subcategory.as_str())
    {
        return rejected(FrenchRejectReason::Abbreviation);
    }
    if meaningful(entry.locution_value.as_ref()) == Some(policy.locution_value.as_str()) {
        return rejected(FrenchRejectReason::Locution);
    }
    if nonstandard_variant {
        return rejected(FrenchRejectReason::NonstandardVariant);
    }
    if source.is_empty() {
        return rejected(FrenchRejectReason::Empty);
    }
    if source.contains(['\'', '\u{2019}', '\u{02bc}', '\u{92}']) {
        return rejected(FrenchRejectReason::Apostrophe);
    }
    if source.contains(['-', '‐', '‑', '‒', '–', '—', '―']) {
        return rejected(FrenchRejectReason::Hyphen);
    }
    if source.chars().any(char::is_whitespace) {
        return rejected(FrenchRejectReason::Whitespace);
    }
    if source.chars().any(char::is_numeric) {
        return rejected(FrenchRejectReason::Digit);
    }
    if source
        .chars()
        .any(|character| character.is_ascii_punctuation())
    {
        return rejected(FrenchRejectReason::Punctuation);
    }

    let Ok(key) = normalize_key(&policy.normalization_profile, source) else {
        return rejected(FrenchRejectReason::UnsupportedCharacter);
    };
    let key = key.into_string();
    if !key
        .chars()
        .all(|character| policy.alphabet.contains(character))
    {
        return rejected(FrenchRejectReason::UnsupportedCharacter);
    }
    let key_length = key.chars().count();
    if key_length < policy.min_word_length {
        return FormOutcome::Rejected {
            reason: FrenchRejectReason::TooShort,
            normalized_key: Some(key),
        };
    }
    if key_length > policy.max_word_length {
        return FormOutcome::Rejected {
            reason: FrenchRejectReason::TooLong,
            normalized_key: Some(key),
        };
    }
    FormOutcome::Accepted(key)
}

fn rejected(reason: FrenchRejectReason) -> FormOutcome {
    FormOutcome::Rejected {
        reason,
        normalized_key: None,
    }
}

fn validate_archive(archive_path: &Path, policy: &FrenchPolicy) -> Result<(), BuilderError> {
    let metadata = fs::metadata(archive_path).map_err(|source| BuilderError::Io {
        path: archive_path.to_path_buf(),
        source,
    })?;
    if metadata.len() != policy.source_archive_size_bytes {
        return Err(BuilderError::MorphalouArchiveSizeMismatch {
            path: archive_path.to_path_buf(),
            expected: policy.source_archive_size_bytes,
            actual: metadata.len(),
        });
    }
    let actual = sha256_file(archive_path)?;
    if actual != policy.source_archive_sha256 {
        return Err(BuilderError::MorphalouArchiveChecksumMismatch {
            path: archive_path.to_path_buf(),
            expected: policy.source_archive_sha256.clone(),
            actual,
        });
    }
    Ok(())
}

fn required_attribute(
    start: &BytesStart<'_>,
    name: &[u8],
    decoder: Decoder,
    position: u64,
) -> Result<String, BuilderError> {
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|source| BuilderError::MorphalouXml {
            position,
            message: source.to_string(),
        })?;
        if attribute.key.as_ref() == name {
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)
                .map_err(|source| BuilderError::MorphalouXml {
                    position,
                    message: source.to_string(),
                })?
                .into_owned();
            if value.is_empty() {
                return xml_error(position, "required XML attribute is empty");
            }
            return Ok(value);
        }
    }
    xml_error(
        position,
        format!(
            "missing required {:?} attribute",
            String::from_utf8_lossy(name)
        ),
    )
}

fn collect_origin(
    start: &BytesStart<'_>,
    decoder: Decoder,
    entry: Option<&mut PendingEntry>,
    position: u64,
) -> Result<(), BuilderError> {
    let target = required_attribute(start, b"target", decoder, position)?;
    require_entry_mut(entry, position)?
        .lemma_origins
        .insert(target);
    Ok(())
}

fn require_entry(entry: Option<&PendingEntry>, position: u64) -> Result<(), BuilderError> {
    if entry.is_some() {
        Ok(())
    } else {
        xml_error(position, "source form metadata outside lexicalEntry")
    }
}

fn require_entry_mut(
    entry: Option<&mut PendingEntry>,
    position: u64,
) -> Result<&mut PendingEntry, BuilderError> {
    entry.ok_or_else(|| BuilderError::MorphalouXml {
        position,
        message: "source form metadata outside lexicalEntry".to_owned(),
    })
}

fn begin_form(
    current: &mut Option<FrenchFormKind>,
    kind: FrenchFormKind,
    position: u64,
) -> Result<(), BuilderError> {
    if current.replace(kind).is_some() {
        xml_error(position, "nested lemma/inflected form elements")
    } else {
        Ok(())
    }
}

fn end_form(
    current: &mut Option<FrenchFormKind>,
    expected: FrenchFormKind,
    position: u64,
) -> Result<(), BuilderError> {
    if current.take() == Some(expected) {
        Ok(())
    } else {
        xml_error(position, "form end does not match its start")
    }
}

fn begin_capture(
    capture: &mut Option<Capture>,
    field: CaptureField,
    position: u64,
) -> Result<(), BuilderError> {
    if capture.is_some() {
        xml_error(position, "nested text-bearing Morphalou fields")
    } else {
        *capture = Some(Capture {
            field,
            value: String::new(),
        });
        Ok(())
    }
}

fn finish_capture(
    capture: &mut Option<Capture>,
    expected: CaptureField,
    position: u64,
) -> Result<String, BuilderError> {
    let completed = capture.take().ok_or_else(|| BuilderError::MorphalouXml {
        position,
        message: "text-bearing field ended without a start".to_owned(),
    })?;
    if completed.field == expected {
        Ok(completed.value)
    } else {
        xml_error(position, "text-bearing field end does not match its start")
    }
}

fn append_text(
    capture: &mut Option<Capture>,
    text: &BytesText<'_>,
    position: u64,
) -> Result<(), BuilderError> {
    let Some(capture) = capture else {
        return Ok(());
    };
    let value = text
        .xml10_content()
        .map_err(|source| BuilderError::MorphalouXml {
            position,
            message: source.to_string(),
        })?;
    capture.value.push_str(&value);
    Ok(())
}

fn append_cdata(
    capture: &mut Option<Capture>,
    text: &BytesCData<'_>,
    position: u64,
) -> Result<(), BuilderError> {
    let Some(capture) = capture else {
        return Ok(());
    };
    let value = text
        .xml10_content()
        .map_err(|source| BuilderError::MorphalouXml {
            position,
            message: source.to_string(),
        })?;
    capture.value.push_str(&value);
    Ok(())
}

fn append_reference(
    capture: &mut Option<Capture>,
    reference: &BytesRef<'_>,
    position: u64,
) -> Result<(), BuilderError> {
    let Some(capture) = capture else {
        return Ok(());
    };
    if let Some(character) =
        reference
            .resolve_char_ref()
            .map_err(|source| BuilderError::MorphalouXml {
                position,
                message: source.to_string(),
            })?
    {
        capture.value.push(character);
        return Ok(());
    }
    let name = reference
        .decode()
        .map_err(|source| BuilderError::MorphalouXml {
            position,
            message: source.to_string(),
        })?;
    let Some(value) = resolve_predefined_entity(&name) else {
        return xml_error(
            position,
            format!("unsupported XML entity reference &{name};"),
        );
    };
    capture.value.push_str(value);
    Ok(())
}

fn record_inactive_comment(
    comment: &BytesText<'_>,
    position: u64,
    report: &mut FrenchBuildReport,
) -> Result<(), BuilderError> {
    let value = comment
        .xml10_content()
        .map_err(|source| BuilderError::MorphalouXml {
            position,
            message: source.to_string(),
        })?;
    add_count(
        &mut report.inactive_commented_entries,
        value.matches("<lexicalEntry ").count(),
        "inactive commented entry count",
    )?;
    add_count(
        &mut report.inactive_commented_form_rows,
        value.matches("<orthography>").count(),
        "inactive commented form count",
    )
}

fn push_form(
    entry: Option<&mut PendingEntry>,
    kind: FrenchFormKind,
    original_form: String,
    position: u64,
) -> Result<(), BuilderError> {
    let entry = require_entry_mut(entry, position)?;
    let index = match kind {
        FrenchFormKind::Lemma => {
            entry.lemma_forms = entry.lemma_forms.checked_add(1).ok_or_else(|| {
                BuilderError::FrenchAccountingInvariant {
                    message: "lemma form index exceeds u64".to_owned(),
                }
            })?;
            entry.lemma_forms
        }
        FrenchFormKind::Inflected => {
            entry.inflected_forms = entry.inflected_forms.checked_add(1).ok_or_else(|| {
                BuilderError::FrenchAccountingInvariant {
                    message: "inflected form index exceeds u64".to_owned(),
                }
            })?;
            entry.inflected_forms
        }
    };
    entry.forms.push(PendingForm {
        kind,
        index,
        original_form,
    });
    Ok(())
}

fn assign_once(
    destination: &mut Option<String>,
    value: Option<String>,
    field: &str,
    position: u64,
) -> Result<(), BuilderError> {
    if destination.is_some() {
        xml_error(position, format!("duplicate {field} in one lexical entry"))
    } else {
        *destination = Some(value.unwrap_or_default());
        Ok(())
    }
}

fn nonempty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn meaningful(value: Option<&String>) -> Option<&str> {
    value.map(String::as_str).filter(|value| !value.is_empty())
}

fn meaningful_owned(value: Option<&String>) -> Option<String> {
    meaningful(value).map(str::to_owned)
}

fn validate_accounting(report: &FrenchBuildReport) -> Result<(), BuilderError> {
    let form_rows = report
        .lemma_rows
        .checked_add(report.inflected_rows)
        .ok_or_else(|| BuilderError::FrenchAccountingInvariant {
            message: "lemma + inflected row count overflow".to_owned(),
        })?;
    if form_rows != report.source_rows {
        return Err(BuilderError::FrenchAccountingInvariant {
            message: format!(
                "{} source rows != {} lemmas + {} inflections",
                report.source_rows, report.lemma_rows, report.inflected_rows
            ),
        });
    }
    let decided = report
        .accepted_rows
        .checked_add(report.rejected_rows)
        .ok_or_else(|| BuilderError::FrenchAccountingInvariant {
            message: "accepted + rejected row count overflow".to_owned(),
        })?;
    if decided != report.source_rows {
        return Err(BuilderError::FrenchAccountingInvariant {
            message: format!(
                "{} source rows != {} accepted + {} rejected",
                report.source_rows, report.accepted_rows, report.rejected_rows
            ),
        });
    }
    let rejection_total = report
        .rejection_reasons
        .values()
        .try_fold(0_u64, |total, count| {
            total
                .checked_add(*count)
                .ok_or_else(|| BuilderError::FrenchAccountingInvariant {
                    message: "rejection reason count overflow".to_owned(),
                })
        })?;
    if rejection_total != report.rejected_rows {
        return Err(BuilderError::FrenchAccountingInvariant {
            message: format!(
                "{rejection_total} rejection reasons != {} rejected rows",
                report.rejected_rows
            ),
        });
    }
    if report.unique_keys > report.accepted_rows {
        return Err(BuilderError::FrenchAccountingInvariant {
            message: "unique keys exceed accepted source rows".to_owned(),
        });
    }
    let expected_duplicates = report.accepted_rows - report.unique_keys;
    if report.duplicate_accepted_rows != expected_duplicates {
        return Err(BuilderError::FrenchAccountingInvariant {
            message: format!(
                "{} duplicate rows != accepted rows minus unique keys ({expected_duplicates})",
                report.duplicate_accepted_rows
            ),
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
        .ok_or_else(|| BuilderError::FrenchAccountingInvariant {
            message: format!("{label} exceeds u64"),
        })?;
    Ok(())
}

fn add_count(value: &mut u64, additional: usize, label: &str) -> Result<(), BuilderError> {
    let additional =
        u64::try_from(additional).map_err(|_| BuilderError::FrenchAccountingInvariant {
            message: format!("{label} exceeds u64"),
        })?;
    *value =
        value
            .checked_add(additional)
            .ok_or_else(|| BuilderError::FrenchAccountingInvariant {
                message: format!("{label} exceeds u64"),
            })?;
    Ok(())
}

fn xml_error<T>(position: u64, message: impl Into<String>) -> Result<T, BuilderError> {
    Err(BuilderError::MorphalouXml {
        position,
        message: message.into(),
    })
}
