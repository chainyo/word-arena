use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use tempfile::TempDir;
use word_arena_lexicon_builder::{
    AUDIT_FILE, AuditDecision, AuditRecord, BUILD_METADATA_FILE, BuildMetadata, BuildReport,
    BuilderError, EnglishPolicy, FILTER_REPORT_FILE, KEYS_FILE, RejectReason,
    build_english_from_final,
};

const OUTPUT_FILES: [&str; 4] = [
    KEYS_FILE,
    AUDIT_FILE,
    FILTER_REPORT_FILE,
    BUILD_METADATA_FILE,
];

#[test]
fn hand_authored_rows_cover_policy_and_audit_boundaries() {
    let fixture = ScowlFixture::new();
    let policy = load_policy();
    let output = fixture.root.path().join("output");
    let summary = build_english_from_final(&fixture.final_directory, &output, &policy)
        .expect("hand-authored English build must succeed");

    assert_eq!(summary.report.source_rows, 38);
    assert_eq!(summary.report.accepted_rows, 20);
    assert_eq!(summary.report.rejected_rows, 18);
    assert_eq!(summary.report.unique_keys, 18);
    assert_eq!(summary.report.duplicate_accepted_rows, 2);
    assert_eq!(
        summary.report.source_rows,
        summary.report.accepted_rows + summary.report.rejected_rows
    );
    assert_eq!(summary.report.rejection_reasons.len(), 18);
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
        "AT",
        "PLAY",
        "PLAYS",
        "PLAYED",
        "PLAYING",
        "COLOR",
        "COLOUR",
        "CENTER",
        "CENTRE",
        "ORGANIZE",
        "ORGANISE",
        "GREY",
        "CATALOGUE",
        "THEATRE",
        "TYRE",
        "ABCDEFGHIJKLMNO",
    ] {
        assert!(keys.contains(expected), "missing accepted key {expected}");
    }
    for excluded in ["A", "ALICE", "NASA", "CAFE", "CHEQUE", "ABCDEFGHIJKLMNOP"] {
        assert!(
            !keys.contains(excluded),
            "unexpected excluded key {excluded}"
        );
    }
    assert!(
        keys.iter().all(|key| {
            (2..=15).contains(&key.len()) && key.bytes().all(|byte| byte.is_ascii_uppercase())
        }),
        "runtime key output must contain only configured board forms"
    );

    let audit = read_audit(&output);
    assert_eq!(audit.len(), 38);
    let cafe = audit
        .iter()
        .find(|record| record.original_form == "café")
        .expect("Latin-1 source form must survive audit decoding");
    assert_eq!(cafe.decision, AuditDecision::Rejected);
    assert_eq!(cafe.reason, Some(RejectReason::UnsupportedCharacter));
    assert_eq!(cafe.normalized_key, None);

    let proper_name = audit
        .iter()
        .find(|record| record.source_file == "english-proper-names.35")
        .expect("proper-name row must be audited");
    assert_eq!(proper_name.original_form, "Alice");
    assert_eq!(proper_name.reason, Some(RejectReason::ProperNameClass));

    let organize_rows = audit
        .iter()
        .filter(|record| record.normalized_key.as_deref() == Some("ORGANIZE"))
        .collect::<Vec<_>>();
    assert_eq!(organize_rows.len(), 2);
    assert_eq!(
        organize_rows
            .iter()
            .filter(|record| record.duplicate_key)
            .count(),
        1
    );
}

#[test]
fn two_clean_builds_are_byte_identical() {
    let fixture = ScowlFixture::new();
    let policy = load_policy();
    let first = fixture.root.path().join("first");
    let second = fixture.root.path().join("second");

    let first_summary = build_english_from_final(&fixture.final_directory, &first, &policy)
        .expect("first clean build");
    let second_summary = build_english_from_final(&fixture.final_directory, &second, &policy)
        .expect("second clean build");

    assert_eq!(first_summary.report, second_summary.report);
    assert_eq!(first_summary.metadata, second_summary.metadata);
    for relative in OUTPUT_FILES {
        assert_eq!(
            fs::read(first.join(relative)).expect("read first build output"),
            fs::read(second.join(relative)).expect("read second build output"),
            "generated {relative} differs"
        );
    }

    let report: BuildReport = toml::from_str(
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
fn unknown_final_files_fail_instead_of_escaping_accounting() {
    let root = TempDir::new().expect("temporary fixture");
    let final_directory = root.path().join("final");
    fs::create_dir(&final_directory).expect("create final directory");
    fs::write(final_directory.join("README"), b"unclassified\n").expect("write unknown input");

    assert!(matches!(
        build_english_from_final(&final_directory, &root.path().join("output"), &load_policy()),
        Err(BuilderError::UnexpectedInputFile { path }) if path.ends_with("README")
    ));
}

fn load_policy() -> EnglishPolicy {
    EnglishPolicy::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lexicons/policies/en-world-v1.toml"),
    )
    .expect("committed English policy must validate")
}

fn read_audit(output: &Path) -> Vec<AuditRecord> {
    fs::read_to_string(output.join(AUDIT_FILE))
        .expect("read generated audit")
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse audit JSON line"))
        .collect()
}

struct ScowlFixture {
    root: TempDir,
    final_directory: PathBuf,
}

impl ScowlFixture {
    fn new() -> Self {
        let root = TempDir::new().expect("temporary fixture");
        let final_directory = root.path().join("final");
        fs::create_dir(&final_directory).expect("create final directory");

        write(
            &final_directory,
            "english-words.10",
            b"a\nat\nplay\nplays\nplayed\nplaying\ncant\nduplicate\nabcdefghijklmno\n",
        );
        let mut american = b"color\ncenter\norganize\nduplicate\nNASA\ncan't\nmother-in-law\nword2\ntwo words\nword!\nabcdefghijklmnop\n".to_vec();
        american.extend_from_slice(b"caf\xe9\n");
        write(&final_directory, "american-words.20", &american);
        write(
            &final_directory,
            "british-words.20",
            b"colour\ncentre\norganise\n",
        );
        write(&final_directory, "british_z-words.20", b"organize\n");
        write(&final_directory, "variant_1-words.35", b"grey\n");
        write(&final_directory, "variant_2-words.35", b"catalogue\n");
        write(&final_directory, "british_variant_1-words.35", b"theatre\n");
        write(&final_directory, "british_variant_2-words.35", b"tyre\n");
        write(&final_directory, "canadian-words.20", b"cheque\n");
        write(&final_directory, "english-proper-names.35", b"Alice\n");
        write(&final_directory, "english-abbreviations.35", b"etc\n");
        write(&final_directory, "english-contractions.35", b"can't\n");
        write(&final_directory, "english-upper.35", b"NASA\n");
        write(&final_directory, "english-affixes.35", b"-ing\n");
        write(&final_directory, "special-hacker.50", b"foobar\n");
        write(&final_directory, "english-words.95", b"beyond\n");
        write(&final_directory, "american-words.40", b"\n");

        Self {
            root,
            final_directory,
        }
    }
}

fn write(directory: &Path, filename: &str, bytes: &[u8]) {
    fs::write(directory.join(filename), bytes).expect("write hand-authored source fixture");
}
