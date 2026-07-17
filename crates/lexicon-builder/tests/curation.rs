use std::{fs, path::Path};

use tempfile::TempDir;
use word_arena_lexicon_builder::{
    CURATION_CHANGELOG_FILE, CURATION_REPORT_FILE, CurationAction, CurationDocument, CurationError,
    CurationGovernance, CurationOverride, HighImpactApproval, HighImpactKind, KEYS_FILE,
    apply_curation, load_curation,
};

const PACK_ID: &str = "word-arena-en-world-v1";
const PROFILE: &str = "en-basic-latin-v1";
const OUTPUT_FILES: [&str; 3] = [KEYS_FILE, CURATION_CHANGELOG_FILE, CURATION_REPORT_FILE];

#[test]
fn applies_attributable_changes_deterministically() {
    let fixture = Fixture::new("CAT\nDOG\nGO\n");
    fixture.write_bundle(
        vec![
            change("OWL", CurationAction::Add),
            change("AX", CurationAction::Add),
        ],
        vec![change("DOG", CurationAction::Remove)],
        governance(1, 1, vec![]),
    );

    let first_output = fixture.root.path().join("first-output");
    let second_output = fixture.root.path().join("second-output");
    let first = apply_curation(&fixture.base_keys, &first_output, &fixture.curation)
        .expect("valid curation must apply");
    let second = apply_curation(&fixture.base_keys, &second_output, &fixture.curation)
        .expect("repeated valid curation must apply");

    assert_eq!(
        fs::read_to_string(first_output.join(KEYS_FILE)).unwrap(),
        "AX\nCAT\nGO\nOWL\n"
    );
    assert_eq!(first.report.base_word_count, 3);
    assert_eq!(first.report.added_word_count, 2);
    assert_eq!(first.report.removed_word_count, 1);
    assert_eq!(first.report.final_word_count, 4);
    assert_eq!(first.report, second.report);
    assert_eq!(
        first
            .additions
            .iter()
            .map(|change| change.normalized_word.as_str())
            .collect::<Vec<_>>(),
        ["AX", "OWL"]
    );

    for filename in OUTPUT_FILES {
        assert_eq!(
            fs::read(first_output.join(filename)).unwrap(),
            fs::read(second_output.join(filename)).unwrap(),
            "{filename} must be byte-identical"
        );
    }
    let changelog = fs::read_to_string(first_output.join(CURATION_CHANGELOG_FILE)).unwrap();
    for expected in [
        "Added playable keys",
        "Removed playable keys",
        "`AX`",
        "`OWL`",
        "`DOG`",
        "Open fixture dictionary",
        "CC-BY-4.0",
        "fixture-author",
        "fixture-reviewer",
        "2026-07-17",
    ] {
        assert!(
            changelog.contains(expected),
            "missing changelog attribution: {expected}"
        );
    }
}

#[test]
fn committed_pack_baselines_are_valid_and_empty() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (directory, pack_id, profile) in [
        ("en-world-v1", "word-arena-en-world-v1", "en-basic-latin-v1"),
        ("fr-v1", "word-arena-fr-v1", "fr-basic-latin-fold-v1"),
    ] {
        let bundle = load_curation(&repository.join("lexicons/curation").join(directory))
            .expect("committed curation baseline must validate");
        assert_eq!(bundle.governance.pack_id, pack_id);
        assert_eq!(bundle.governance.normalization_profile, profile);
        assert!(bundle.additions.overrides.is_empty());
        assert!(bundle.removals.overrides.is_empty());
        assert!(bundle.governance.approvals.is_empty());
    }
}

#[test]
fn rejects_invalid_duplicate_and_conflicting_overrides() {
    for case in invalid_override_cases() {
        let fixture = Fixture::new("CAT\nDOG\n");
        fixture.write_bundle(case.additions, case.removals, governance(1, 1, vec![]));
        let error = load_curation(&fixture.curation).expect_err(case.name);
        assert_expected_error(case.name, case.expected, &error);
    }
}

#[test]
fn rejects_undocumented_override() {
    let fixture = Fixture::new("CAT\nDOG\n");
    fixture.write_bundle(
        vec![change("AX", CurationAction::Add)],
        vec![],
        governance(1, 1, vec![]),
    );
    let additions_path = fixture.curation.join("additions.toml");
    let undocumented = fs::read_to_string(&additions_path)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("reviewer ="))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&additions_path, format!("{undocumented}\n")).unwrap();
    assert!(matches!(
        load_curation(&fixture.curation),
        Err(CurationError::Syntax { .. })
    ));
}

struct InvalidCase {
    name: &'static str,
    additions: Vec<CurationOverride>,
    removals: Vec<CurationOverride>,
    expected: ExpectedError,
}

#[derive(Clone, Copy)]
enum ExpectedError {
    Duplicate,
    Conflicting,
    InvalidField(&'static str),
}

fn invalid_override_cases() -> Vec<InvalidCase> {
    vec![
        InvalidCase {
            name: "duplicate",
            additions: vec![
                change("AX", CurationAction::Add),
                change("AX", CurationAction::Add),
            ],
            removals: vec![],
            expected: ExpectedError::Duplicate,
        },
        InvalidCase {
            name: "conflicting",
            additions: vec![change("AX", CurationAction::Add)],
            removals: vec![change("AX", CurationAction::Remove)],
            expected: ExpectedError::Conflicting,
        },
        InvalidCase {
            name: "wrong action",
            additions: vec![change("AX", CurationAction::Remove)],
            removals: vec![],
            expected: ExpectedError::InvalidField("action"),
        },
        InvalidCase {
            name: "not normalized",
            additions: vec![change("cafe", CurationAction::Add)],
            removals: vec![],
            expected: ExpectedError::InvalidField("normalized_word"),
        },
        InvalidCase {
            name: "closed evidence",
            additions: vec![change_with("AX", |change| {
                change.supporting_source_license = "proprietary".into();
            })],
            removals: vec![],
            expected: ExpectedError::InvalidField("supporting_source_license"),
        },
        InvalidCase {
            name: "proprietary list evidence",
            additions: vec![change_with("AX", |change| {
                change.supporting_source_title = "Collins list".into();
            })],
            removals: vec![],
            expected: ExpectedError::InvalidField("supporting_source_url"),
        },
        InvalidCase {
            name: "self review",
            additions: vec![change_with("AX", |change| {
                change.reviewer = change.author.clone();
            })],
            removals: vec![],
            expected: ExpectedError::InvalidField("reviewer"),
        },
        InvalidCase {
            name: "case-insensitive self review",
            additions: vec![change_with("AX", |change| {
                change.author = "Fixture-Author".into();
                change.reviewer = "fixture-author".into();
            })],
            removals: vec![],
            expected: ExpectedError::InvalidField("reviewer"),
        },
        InvalidCase {
            name: "blank reason",
            additions: vec![change_with("AX", |change| {
                change.reason = "   ".into();
            })],
            removals: vec![],
            expected: ExpectedError::InvalidField("reason"),
        },
        InvalidCase {
            name: "invalid date",
            additions: vec![change_with("AX", |change| {
                change.date = "2025-02-29".into();
            })],
            removals: vec![],
            expected: ExpectedError::InvalidField("date"),
        },
    ]
}

fn assert_expected_error(name: &str, expected: ExpectedError, error: &CurationError) {
    let matches = match expected {
        ExpectedError::Duplicate => {
            matches!(error, CurationError::DuplicateOverride { word, .. } if word == "AX")
        }
        ExpectedError::Conflicting => {
            matches!(error, CurationError::ConflictingOverride { word } if word == "AX")
        }
        ExpectedError::InvalidField(expected) => {
            matches!(error, CurationError::InvalidOverride { field, .. } if *field == expected)
        }
    };
    assert!(matches, "{name} produced unexpected error: {error}");
}

#[test]
fn rejects_noop_changes_and_invalid_base_sets() {
    let addition_fixture = Fixture::new("CAT\nDOG\n");
    addition_fixture.write_bundle(
        vec![change("CAT", CurationAction::Add)],
        vec![],
        governance(1, 1, vec![]),
    );
    assert!(matches!(
        apply_curation(
            &addition_fixture.base_keys,
            &addition_fixture.root.path().join("output"),
            &addition_fixture.curation,
        ),
        Err(CurationError::NoopAddition { word }) if word == "CAT"
    ));

    let removal_fixture = Fixture::new("CAT\nDOG\n");
    removal_fixture.write_bundle(
        vec![],
        vec![change("OWL", CurationAction::Remove)],
        governance(1, 1, vec![]),
    );
    assert!(matches!(
        apply_curation(
            &removal_fixture.base_keys,
            &removal_fixture.root.path().join("output"),
            &removal_fixture.curation,
        ),
        Err(CurationError::NoopRemoval { word }) if word == "OWL"
    ));

    for invalid_keys in ["DOG\nCAT\n", "CAT\nCAT\n", "Cat\nDOG\n"] {
        let fixture = Fixture::new(invalid_keys);
        fixture.write_bundle(vec![], vec![], governance(1, 1, vec![]));
        assert!(matches!(
            apply_curation(
                &fixture.base_keys,
                &fixture.root.path().join("output"),
                &fixture.curation,
            ),
            Err(CurationError::InvalidBaseKey { .. })
        ));
    }
}

#[test]
fn high_impact_versions_require_independent_matching_approval() {
    for (policy_version, normalization_version, expected_kind) in [
        (2, 1, HighImpactKind::BroadFilter),
        (1, 2, HighImpactKind::Normalization),
    ] {
        let fixture = Fixture::new("CAT\nDOG\n");
        fixture.write_bundle(
            vec![],
            vec![],
            governance(policy_version, normalization_version, vec![]),
        );
        assert!(matches!(
            load_curation(&fixture.curation),
            Err(CurationError::MissingHighImpactApproval { kind, version: 2 }) if kind == expected_kind
        ));
    }

    let fixture = Fixture::new("CAT\nDOG\n");
    fixture.write_bundle(
        vec![],
        vec![],
        governance(
            2,
            2,
            vec![
                approval(HighImpactKind::BroadFilter, 2),
                approval(HighImpactKind::Normalization, 2),
            ],
        ),
    );
    load_curation(&fixture.curation).expect("matching two-person approvals must validate");

    let fixture = Fixture::new("CAT\nDOG\n");
    let mut self_approved = approval(HighImpactKind::BroadFilter, 2);
    self_approved.reviewer = self_approved.author.clone();
    fixture.write_bundle(vec![], vec![], governance(2, 1, vec![self_approved]));
    assert!(matches!(
        load_curation(&fixture.curation),
        Err(CurationError::InvalidApproval {
            field: "reviewer",
            ..
        })
    ));
}

struct Fixture {
    root: TempDir,
    base_keys: std::path::PathBuf,
    curation: std::path::PathBuf,
}

impl Fixture {
    fn new(base_keys: &str) -> Self {
        let root = TempDir::new().unwrap();
        let base_keys_path = root.path().join("base-keys.txt");
        let curation = root.path().join("curation");
        fs::write(&base_keys_path, base_keys).unwrap();
        fs::create_dir(&curation).unwrap();
        Self {
            root,
            base_keys: base_keys_path,
            curation,
        }
    }

    fn write_bundle(
        &self,
        additions: Vec<CurationOverride>,
        removals: Vec<CurationOverride>,
        governance: CurationGovernance,
    ) {
        write_toml(
            &self.curation.join("additions.toml"),
            document(CurationAction::Add, additions),
        );
        write_toml(
            &self.curation.join("removals.toml"),
            document(CurationAction::Remove, removals),
        );
        write_toml(&self.curation.join("governance.toml"), governance);
    }
}

fn document(action: CurationAction, overrides: Vec<CurationOverride>) -> CurationDocument {
    CurationDocument {
        schema_version: 1,
        pack_id: PACK_ID.into(),
        normalization_profile: PROFILE.into(),
        document_action: action,
        overrides,
    }
}

fn governance(
    policy_version: u32,
    normalization_version: u32,
    approvals: Vec<HighImpactApproval>,
) -> CurationGovernance {
    CurationGovernance {
        schema_version: 1,
        pack_id: PACK_ID.into(),
        policy_version,
        normalization_version,
        normalization_profile: PROFILE.into(),
        approvals,
    }
}

fn change(word: &str, action: CurationAction) -> CurationOverride {
    CurationOverride {
        normalized_word: word.into(),
        action,
        reason: "The open source explicitly documents this lexical form.".into(),
        supporting_source_title: "Open fixture dictionary".into(),
        supporting_source_url: "https://example.org/open-dictionary/entry".into(),
        supporting_source_license: "CC-BY-4.0".into(),
        author: "fixture-author".into(),
        reviewer: "fixture-reviewer".into(),
        date: "2026-07-17".into(),
    }
}

fn change_with(word: &str, mutate: impl FnOnce(&mut CurationOverride)) -> CurationOverride {
    let mut change = change(word, CurationAction::Add);
    mutate(&mut change);
    change
}

fn approval(kind: HighImpactKind, version: u32) -> HighImpactApproval {
    HighImpactApproval {
        kind,
        version,
        summary: "Reviewed high-impact fixture change".into(),
        tracking_url: "https://github.com/chainyo/word-arena/pull/123".into(),
        author: "fixture-author".into(),
        reviewer: "fixture-reviewer".into(),
        date: "2026-07-17".into(),
    }
}

fn write_toml(path: &Path, value: impl serde::Serialize) {
    let encoded = toml::to_string_pretty(&value).unwrap();
    fs::write(path, encoded).unwrap();
}
