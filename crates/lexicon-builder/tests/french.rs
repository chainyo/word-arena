use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use tempfile::TempDir;
use word_arena_lexicon_builder::{
    AUDIT_FILE, AuditDecision, BUILD_METADATA_FILE, BuildMetadata, BuilderError,
    FILTER_REPORT_FILE, FrenchAuditRecord, FrenchBuildReport, FrenchPolicy, FrenchRejectReason,
    KEYS_FILE, build_french_from_archive, build_french_from_xml,
};

const OUTPUT_FILES: [&str; 4] = [
    KEYS_FILE,
    AUDIT_FILE,
    FILTER_REPORT_FILE,
    BUILD_METADATA_FILE,
];

#[test]
fn hand_authored_rows_cover_french_policy_and_normalization_boundaries() {
    let fixture = MorphalouFixture::new();
    let policy = load_policy();
    let output = fixture.root.path().join("output");
    let summary = build_french_from_xml(&fixture.xml_path, &output, &policy)
        .expect("hand-authored French build must succeed");

    assert_eq!(summary.report.source_entries, 26);
    assert_eq!(summary.report.inactive_commented_entries, 1);
    assert_eq!(summary.report.source_rows, 35);
    assert_eq!(summary.report.inactive_commented_form_rows, 2);
    assert_eq!(summary.report.lemma_rows, 26);
    assert_eq!(summary.report.inflected_rows, 9);
    assert_eq!(summary.report.accepted_rows, 20);
    assert_eq!(summary.report.rejected_rows, 15);
    assert_eq!(summary.report.unique_keys, 17);
    assert_eq!(summary.report.duplicate_accepted_rows, 3);
    assert_eq!(summary.report.rejection_reasons.len(), 15);
    assert!(
        summary
            .report
            .rejection_reasons
            .values()
            .all(|count| *count == 1)
    );

    let keys = fs::read_to_string(output.join(KEYS_FILE)).expect("read generated keys");
    let keys = keys.lines().collect::<BTreeSet<_>>();
    for expected in [
        "CHAT",
        "CHATS",
        "MANGER",
        "MANGE",
        "MANGEONS",
        "MANGEES",
        "ETE",
        "ETES",
        "ECOLE",
        "ECOLES",
        "COEUR",
        "OEUF",
        "OEUFS",
        "CA",
        "ABIMER",
        "ABIME",
        "ABCDEFGHIJKLMNO",
    ] {
        assert!(keys.contains(expected), "missing accepted key {expected}");
    }
    for excluded in [
        "ETC",
        "PASKE",
        "PARIS",
        "MYSTERE",
        "SMR",
        "A",
        "ABCDEFGHIJKLMNOP",
    ] {
        assert!(
            !keys.contains(excluded),
            "unexpected excluded key {excluded}"
        );
    }

    let audit = read_audit(&output);
    assert_eq!(audit.len(), 35);
    let uppercase_accent = find_original(&audit, "École");
    assert_eq!(uppercase_accent.normalized_key.as_deref(), Some("ECOLE"));
    assert_eq!(uppercase_accent.decision, AuditDecision::Accepted);
    let cedilla = find_original(&audit, "Ça");
    assert_eq!(cedilla.normalized_key.as_deref(), Some("CA"));
    let ligature = find_original(&audit, "cœur");
    assert_eq!(ligature.normalized_key.as_deref(), Some("COEUR"));
    let collision_rows = audit
        .iter()
        .filter(|record| record.normalized_key.as_deref() == Some("COEUR"))
        .collect::<Vec<_>>();
    assert_eq!(collision_rows.len(), 2);
    assert_eq!(
        collision_rows
            .iter()
            .filter(|record| record.duplicate_key)
            .count(),
        1
    );

    let nonstandard = find_original(&audit, "paske");
    assert_eq!(
        nonstandard.reason,
        Some(FrenchRejectReason::NonstandardVariant)
    );
    assert_eq!(
        nonstandard.variant_origins.as_deref(),
        Some(["lefff".to_owned()].as_slice())
    );
    let standard_variant = find_original(&audit, "abimer");
    assert_eq!(standard_variant.decision, AuditDecision::Accepted);
    assert!(!standard_variant.nonstandard_variant);
    let expanded_too_long = find_original(&audit, "abcdefghijklmnœ");
    assert_eq!(
        expanded_too_long.normalized_key.as_deref(),
        Some("ABCDEFGHIJKLMNOE")
    );
    assert_eq!(expanded_too_long.reason, Some(FrenchRejectReason::TooLong));
}

#[test]
fn two_clean_french_builds_are_byte_identical() {
    let fixture = MorphalouFixture::new();
    let policy = load_policy();
    let first = fixture.root.path().join("first");
    let second = fixture.root.path().join("second");

    let first_summary = build_french_from_xml(&fixture.xml_path, &first, &policy)
        .expect("first clean French build");
    let second_summary = build_french_from_xml(&fixture.xml_path, &second, &policy)
        .expect("second clean French build");

    assert_eq!(first_summary.report, second_summary.report);
    assert_eq!(first_summary.metadata, second_summary.metadata);
    for relative in OUTPUT_FILES {
        assert_eq!(
            fs::read(first.join(relative)).expect("read first build output"),
            fs::read(second.join(relative)).expect("read second build output"),
            "generated {relative} differs"
        );
    }
    let report: FrenchBuildReport = toml::from_str(
        &fs::read_to_string(first.join(FILTER_REPORT_FILE)).expect("read report TOML"),
    )
    .expect("parse report TOML");
    let metadata: BuildMetadata = toml::from_str(
        &fs::read_to_string(first.join(BUILD_METADATA_FILE)).expect("read metadata TOML"),
    )
    .expect("parse metadata TOML");
    assert_eq!(report, first_summary.report);
    assert_eq!(metadata, first_summary.metadata);
}

#[test]
fn malformed_lmf_and_unpinned_archives_fail_safely() {
    let root = TempDir::new().expect("temporary fixture");
    let malformed = root.path().join("malformed.xml");
    fs::write(
        &malformed,
        b"<?xml version=\"1.0\"?><lexicon><lexicalEntry></lexicalEntry></lexicon>",
    )
    .expect("write malformed fixture");
    assert!(matches!(
        build_french_from_xml(&malformed, &root.path().join("output"), &load_policy()),
        Err(BuilderError::MorphalouXml { .. })
    ));

    let fake_archive = root.path().join("fake.zip");
    fs::write(&fake_archive, b"not the pinned archive").expect("write fake archive");
    assert!(matches!(
        build_french_from_archive(
            &fake_archive,
            &root.path().join("archive-output"),
            &load_policy()
        ),
        Err(BuilderError::MorphalouArchiveSizeMismatch { .. })
    ));
}

fn load_policy() -> FrenchPolicy {
    FrenchPolicy::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lexicons/policies/fr-v1.toml"),
    )
    .expect("committed French policy must validate")
}

fn read_audit(output: &Path) -> Vec<FrenchAuditRecord> {
    fs::read_to_string(output.join(AUDIT_FILE))
        .expect("read generated audit")
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse audit JSON line"))
        .collect()
}

fn find_original<'a>(audit: &'a [FrenchAuditRecord], original: &str) -> &'a FrenchAuditRecord {
    audit
        .iter()
        .find(|record| record.original_form == original)
        .unwrap_or_else(|| panic!("missing audit row for {original:?}"))
}

struct MorphalouFixture {
    root: TempDir,
    xml_path: PathBuf,
}

impl MorphalouFixture {
    fn new() -> Self {
        let root = TempDir::new().expect("temporary fixture");
        let xml_path = root.path().join("morphalou.xml");
        let xml = FIXTURE_XML.replace("DECOMPOSED_ETE", "e\u{301}te");
        fs::write(&xml_path, xml).expect("write hand-authored Morphalou fixture");
        Self { root, xml_path }
    }
}

const FIXTURE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<lexicon>
  <!-- <lexicalEntry id="inactive_1"><formSet><lemmatizedForm><orthography>inactive</orthography></lemmatizedForm><inflectedForm><orthography>inactives</orthography></inflectedForm></formSet></lexicalEntry> -->
  <lexicalEntry id="chat_1"><formSet><lemmatizedForm><orthography>chat</orthography><grammaticalCategory>commonNoun</grammaticalCategory><originatingEntry target="morphalou2">chat</originatingEntry></lemmatizedForm><inflectedForm><orthography>chat</orthography></inflectedForm><inflectedForm><orthography>chats</orthography></inflectedForm></formSet></lexicalEntry>
  <lexicalEntry id="manger_1"><formSet><lemmatizedForm><orthography>manger</orthography><grammaticalCategory>verb</grammaticalCategory></lemmatizedForm><inflectedForm><orthography>mange</orthography></inflectedForm><inflectedForm><orthography>mangeons</orthography></inflectedForm><inflectedForm><orthography>mangées</orthography></inflectedForm></formSet></lexicalEntry>
  <lexicalEntry id="été_1"><formSet><lemmatizedForm><orthography>été</orthography><grammaticalCategory>commonNoun</grammaticalCategory></lemmatizedForm><inflectedForm><orthography>étés</orthography></inflectedForm></formSet></lexicalEntry>
  <lexicalEntry id="école_1"><formSet><lemmatizedForm><orthography>École</orthography><grammaticalCategory>commonNoun</grammaticalCategory></lemmatizedForm><inflectedForm><orthography>écoles</orthography></inflectedForm></formSet></lexicalEntry>
  <lexicalEntry id="cœur_1"><formSet><lemmatizedForm><orthography>cœur</orthography><grammaticalCategory>commonNoun</grammaticalCategory></lemmatizedForm></formSet></lexicalEntry>
  <lexicalEntry id="coeur_1"><formSet><lemmatizedForm><orthography>coeur</orthography><grammaticalCategory>commonNoun</grammaticalCategory></lemmatizedForm></formSet></lexicalEntry>
  <lexicalEntry id="œuf_1"><formSet><lemmatizedForm><orthography>Œuf</orthography><grammaticalCategory>commonNoun</grammaticalCategory></lemmatizedForm><inflectedForm><orthography>œufs</orthography></inflectedForm></formSet></lexicalEntry>
  <lexicalEntry id="ça_1"><formSet><lemmatizedForm><orthography>Ça</orthography><grammaticalCategory>pronoun</grammaticalCategory></lemmatizedForm></formSet></lexicalEntry>
  <lexicalEntry id="abimer_1"><spellingVariantOf target="abîmer_1">abîmer</spellingVariantOf><formSet><lemmatizedForm><orthography>abimer</orthography><grammaticalCategory>verb</grammaticalCategory><originatingEntry target="dicollecte">abimer</originatingEntry></lemmatizedForm><inflectedForm><orthography>abimé</orthography></inflectedForm></formSet></lexicalEntry>
  <lexicalEntry id="decomposed_1"><formSet><lemmatizedForm><orthography>DECOMPOSED_ETE</orthography><grammaticalCategory>commonNoun</grammaticalCategory></lemmatizedForm></formSet></lexicalEntry>
  <lexicalEntry id="max_1"><formSet><lemmatizedForm><orthography>abcdefghijklmno</orthography><grammaticalCategory>commonNoun</grammaticalCategory></lemmatizedForm></formSet></lexicalEntry>
  <lexicalEntry id="etc_1"><formSet><lemmatizedForm><orthography>etc.</orthography><grammaticalCategory>commonNoun</grammaticalCategory><grammaticalSubCategory>abbreviation</grammaticalSubCategory></lemmatizedForm></formSet></lexicalEntry>
  <lexicalEntry id="locution_1"><formSet><lemmatizedForm><orthography>pomme de terre</orthography><grammaticalCategory>commonNoun</grammaticalCategory><locution>oui</locution></lemmatizedForm></formSet></lexicalEntry>
  <lexicalEntry id="paske_1"><spellingVariantOf target="parce_que_1">parce que</spellingVariantOf><formSet><lemmatizedForm><orthography>paske</orthography><grammaticalCategory>conjunction</grammaticalCategory><grammaticalSubCategory>subordination</grammaticalSubCategory><originatingEntry target="lefff">pask'</originatingEntry></lemmatizedForm></formSet></lexicalEntry>
  <lexicalEntry id="Paris_1"><formSet><lemmatizedForm><orthography>Paris</orthography><grammaticalCategory>properNoun</grammaticalCategory></lemmatizedForm></formSet></lexicalEntry>
  <lexicalEntry id="missing_1"><formSet><lemmatizedForm><orthography>mystere</orthography><grammaticalCategory></grammaticalCategory></lemmatizedForm></formSet></lexicalEntry>
  <lexicalEntry id="unsupported_category_1"><formSet><lemmatizedForm><orthography>article</orthography><grammaticalCategory>article</grammaticalCategory></lemmatizedForm></formSet></lexicalEntry>
  <lexicalEntry id="empty_1"><formSet><lemmatizedForm><orthography></orthography><grammaticalCategory>commonNoun</grammaticalCategory></lemmatizedForm></formSet></lexicalEntry>
  <lexicalEntry id="apostrophe_1"><formSet><lemmatizedForm><orthography>l'été</orthography><grammaticalCategory>commonNoun</grammaticalCategory></lemmatizedForm></formSet></lexicalEntry>
  <lexicalEntry id="hyphen_1"><formSet><lemmatizedForm><orthography>arc-en-ciel</orthography><grammaticalCategory>commonNoun</grammaticalCategory></lemmatizedForm></formSet></lexicalEntry>
  <lexicalEntry id="space_1"><formSet><lemmatizedForm><orthography>deux mots</orthography><grammaticalCategory>commonNoun</grammaticalCategory></lemmatizedForm></formSet></lexicalEntry>
  <lexicalEntry id="digit_1"><formSet><lemmatizedForm><orthography>mot2</orthography><grammaticalCategory>commonNoun</grammaticalCategory></lemmatizedForm></formSet></lexicalEntry>
  <lexicalEntry id="punctuation_1"><formSet><lemmatizedForm><orthography>mot!</orthography><grammaticalCategory>commonNoun</grammaticalCategory></lemmatizedForm></formSet></lexicalEntry>
  <lexicalEntry id="unsupported_character_1"><formSet><lemmatizedForm><orthography>smør</orthography><grammaticalCategory>commonNoun</grammaticalCategory></lemmatizedForm></formSet></lexicalEntry>
  <lexicalEntry id="short_1"><formSet><lemmatizedForm><orthography>à</orthography><grammaticalCategory>preposition</grammaticalCategory></lemmatizedForm></formSet></lexicalEntry>
  <lexicalEntry id="long_ligature_1"><formSet><lemmatizedForm><orthography>abcdefghijklmnœ</orthography><grammaticalCategory>commonNoun</grammaticalCategory></lemmatizedForm></formSet></lexicalEntry>
</lexicon>
"#;
