use std::{collections::BTreeSet, fmt::Write as _, fs, fs::File, path::Path};

use flate2::{Compression, write::GzEncoder};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use word_arena_lexicon_builder::{
    AUDIT_FILE, ApprovedNativeReview, BUILD_METADATA_FILE, BuilderError, FILTER_REPORT_FILE,
    HunspellAuditRecord, HunspellPolicy, HunspellRejectReason, HunspellSourceClass, KEYS_FILE,
    build_hunspell_from_archive, build_hunspell_from_strings,
};

const OUTPUT_FILES: [&str; 4] = [
    KEYS_FILE,
    AUDIT_FILE,
    FILTER_REPORT_FILE,
    BUILD_METADATA_FILE,
];

const AFFIX: &str = r"SET UTF-8
FLAG UTF-8
NOSUGGEST X
FORBIDDENWORD F
SFX S Y 1
SFX S 0 s .
";

const DICTIONARY: &str = "13
Fuß/S
Füße
Fuße
Häuser
A
abcdefghijklmnop
alpha-beta
wort2
O'Neil
punkt.
λambda
privat/X
verboten/F
";

#[test]
fn synthetic_hunspell_forms_expand_filter_and_audit_deterministically() {
    let root = TempDir::new().expect("temporary build root");
    let policy = german_policy();
    let first = root.path().join("first");
    let second = root.path().join("second");

    let first_summary = build_hunspell_from_strings(AFFIX, DICTIONARY, &first, &policy)
        .expect("first synthetic build");
    let second_summary = build_hunspell_from_strings(AFFIX, DICTIONARY, &second, &policy)
        .expect("second synthetic build");

    assert_eq!(first_summary.report, second_summary.report);
    assert_eq!(first_summary.metadata, second_summary.metadata);
    for relative in OUTPUT_FILES {
        assert_eq!(
            fs::read(first.join(relative)).expect("read first output"),
            fs::read(second.join(relative)).expect("read second output"),
            "generated {relative} differs"
        );
    }

    let keys = fs::read_to_string(first.join(KEYS_FILE)).expect("read keys");
    let keys = keys.lines().collect::<BTreeSet<_>>();
    for expected in ["FUSS", "FUSSS", "FUSSE", "HAUSER", "PRIVAT"] {
        assert!(keys.contains(expected), "missing normalized key {expected}");
    }
    for excluded in [
        "A",
        "ABCDEFGHIJKLMNOP",
        "ALPHABETA",
        "WORT2",
        "ONEIL",
        "PUNKT",
        "LAMBDA",
        "VERBOTEN",
    ] {
        assert!(!keys.contains(excluded), "unexpected key {excluded}");
    }

    let audit = read_audit(&first);
    assert_eq!(
        find_form(&audit, "privat").source_classes,
        [HunspellSourceClass::NoSuggest]
    );
    assert_eq!(
        find_form(&audit, "alpha-beta").reason,
        Some(HunspellRejectReason::Hyphen)
    );
    assert_eq!(
        find_form(&audit, "λambda").reason,
        Some(HunspellRejectReason::UnsupportedCharacter)
    );
    let collisions = audit
        .iter()
        .filter(|record| record.normalized_key.as_deref() == Some("FUSSE"))
        .collect::<Vec<_>>();
    assert_eq!(collisions.len(), 2);
    assert_eq!(
        collisions
            .iter()
            .filter(|record| record.duplicate_key)
            .count(),
        1
    );
    assert_eq!(first_summary.report.forbidden_forms, 1);
    assert_eq!(
        first_summary.report.generated_forms,
        first_summary.report.accepted_forms + first_summary.report.rejected_forms
    );
}

#[test]
fn committed_multilingual_policies_are_strict_and_language_bound() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lexicons/policies");
    let german = HunspellPolicy::load(&root.join("de-v1.toml")).expect("German policy");
    let spanish = HunspellPolicy::load(&root.join("es-v1.toml")).expect("Spanish policy");

    assert_eq!(german.locale, "de");
    assert_eq!(german.normalization_profile, "de-basic-latin-fold-v1");
    assert_eq!(spanish.locale, "es");
    assert_eq!(spanish.normalization_profile, "es-basic-latin-fold-v1");

    let mut drifted = spanish;
    drifted.locale = "de".to_owned();
    assert!(matches!(
        drifted.validate(),
        Err(BuilderError::InvalidPolicy {
            field: "pack_id",
            ..
        })
    ));
}

#[test]
fn malformed_sources_and_existing_destinations_fail_closed() {
    let root = TempDir::new().expect("temporary build root");
    let output = root.path().join("output");
    assert!(matches!(
        build_hunspell_from_strings(
            "not an affix",
            "not a dictionary",
            &output,
            &german_policy()
        ),
        Err(BuilderError::HunspellParse { .. })
    ));

    fs::create_dir(&output).expect("reserve output");
    assert!(matches!(
        build_hunspell_from_strings(AFFIX, DICTIONARY, &output, &german_policy()),
        Err(BuilderError::OutputExists { .. })
    ));
}

#[test]
fn real_archive_entry_point_requires_exact_review_and_source_pins() {
    let root = TempDir::new().expect("temporary archive root");
    let mut policy = german_policy();
    assert!(matches!(
        ApprovedNativeReview::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lexicons"),
            &policy
        ),
        Err(BuilderError::NativeReviewRequired { .. })
    ));

    let archive = root.path().join("source.tar.gz");
    write_archive(&archive, &policy, true);
    let bytes = fs::read(&archive).expect("read synthetic archive");
    policy.source_archive_size_bytes = u64::try_from(bytes.len()).expect("portable archive size");
    policy.source_archive_sha256 = digest(&bytes);

    let reviews = root.path().join("reviews");
    fs::create_dir(&reviews).expect("create review directory");
    fs::write(
        reviews.join("de-v1.toml"),
        format!(
            r#"schema_version = 1
language = "de"
policy_id = "{}"
policy_version = {}
source_id = "{}"
status = "approved"
required_qualification = "{}"
reviewer = "fixture-native-reviewer"
reviewed_on = "2026-07-20"
decision = "approved"
rationale = "Synthetic fixture review exercises the guarded archive boundary."
evidence_url = "https://example.invalid/reviews/de-fixture"
"#,
            policy.id, policy.version, policy.source_id, policy.review_requirement.qualification
        ),
    )
    .expect("write approval fixture");
    let approval = ApprovedNativeReview::load(root.path(), &policy).expect("approved fixture");
    assert_eq!(approval.reviewer(), "fixture-native-reviewer");
    assert_eq!(approval.reviewed_on(), "2026-07-20");

    let output = root.path().join("archive-output");
    build_hunspell_from_archive(&archive, &output, &policy, &approval)
        .expect("pinned approved synthetic archive");
    assert!(output.join(KEYS_FILE).is_file());

    let mut wrong_pin = policy.clone();
    wrong_pin.source_archive_sha256 = "f".repeat(64);
    assert!(matches!(
        build_hunspell_from_archive(
            &archive,
            &root.path().join("wrong-pin"),
            &wrong_pin,
            &approval
        ),
        Err(BuilderError::HunspellArchiveChecksumMismatch { .. })
    ));
}

fn german_policy() -> HunspellPolicy {
    HunspellPolicy::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lexicons/policies/de-v1.toml"),
    )
    .expect("committed German policy")
}

fn read_audit(output: &Path) -> Vec<HunspellAuditRecord> {
    fs::read_to_string(output.join(AUDIT_FILE))
        .expect("read audit")
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse audit row"))
        .collect()
}

fn find_form<'a>(audit: &'a [HunspellAuditRecord], source: &str) -> &'a HunspellAuditRecord {
    audit
        .iter()
        .find(|record| record.source_form == source)
        .unwrap_or_else(|| panic!("missing audit form {source:?}"))
}

fn write_archive(path: &Path, policy: &HunspellPolicy, include_dictionary: bool) {
    let file = File::create(path).expect("create archive");
    let encoder = GzEncoder::new(file, Compression::default());
    let mut archive = tar::Builder::new(encoder);
    append_member(
        &mut archive,
        &format!(
            "{}/{}",
            policy.source_archive_root, policy.source_affix_path
        ),
        AFFIX.as_bytes(),
    );
    if include_dictionary {
        append_member(
            &mut archive,
            &format!(
                "{}/{}",
                policy.source_archive_root, policy.source_dictionary_path
            ),
            DICTIONARY.as_bytes(),
        );
    }
    let encoder = archive.into_inner().expect("finish tar");
    encoder.finish().expect("finish gzip");
}

fn append_member(archive: &mut tar::Builder<GzEncoder<File>>, path: &str, bytes: &[u8]) {
    let mut header = tar::Header::new_gnu();
    header.set_path(path).expect("set member path");
    header.set_size(u64::try_from(bytes.len()).expect("portable member size"));
    header.set_mode(0o644);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_cksum();
    archive
        .append(&header, bytes)
        .expect("append synthetic member");
}

fn digest(bytes: &[u8]) -> String {
    let hash = Sha256::digest(bytes);
    hash.iter()
        .fold(String::with_capacity(64), |mut encoded, byte| {
            write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
            encoded
        })
}
