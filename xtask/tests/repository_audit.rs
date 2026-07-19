use std::{fs, path::Path};

use tempfile::TempDir;
use word_arena_lexicon_builder::BuilderError;
use xtask::{PackRegistry, XtaskError, audit_repository};

#[test]
fn committed_repository_contract_is_self_consistent() {
    let workspace = workspace_root();
    let registry = PackRegistry::load(&workspace.join("lexicons/registry.toml")).unwrap();
    let summary = audit_repository(&workspace, &registry).unwrap();

    assert_eq!(summary.source_count, 4);
    assert_eq!(summary.pack_count, 2);
    assert_eq!(summary.release_tag, "lexicons-v1.0.0");
}

#[test]
fn changed_license_bytes_are_rejected() {
    let fixture = copied_lexicons();
    let license = fixture.path().join("lexicons/licenses/LGPLLR.txt");
    let mut bytes = fs::read(&license).unwrap();
    bytes.extend_from_slice(b"tampered\n");
    fs::write(&license, bytes).unwrap();
    let registry = PackRegistry::load(&fixture.path().join("lexicons/registry.toml")).unwrap();

    assert!(matches!(
        audit_repository(fixture.path(), &registry),
        Err(XtaskError::BuildContract { message }) if message.contains("committed license")
    ));
}

#[test]
fn changed_multilingual_notice_bytes_are_rejected() {
    let fixture = copied_lexicons();
    let notice = fixture
        .path()
        .join("lexicons/licenses/IGERMAN98-NOTICE.txt");
    let mut bytes = fs::read(&notice).unwrap();
    bytes.extend_from_slice(b"tampered\n");
    fs::write(&notice, bytes).unwrap();
    let registry = PackRegistry::load(&fixture.path().join("lexicons/registry.toml")).unwrap();

    assert!(matches!(
        audit_repository(fixture.path(), &registry),
        Err(XtaskError::BuildContract { message }) if message.contains("committed license")
    ));
}

#[test]
fn policy_and_source_pin_drift_is_rejected() {
    let fixture = copied_lexicons();
    let policy = fixture.path().join("lexicons/policies/en-world-v1.toml");
    let encoded = fs::read_to_string(&policy).unwrap().replace(
        "65e4891913a252659efd9a464b923124940082b5bd4da4878d1e7fbf1b80bc50",
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    );
    fs::write(policy, encoded).unwrap();
    let registry = PackRegistry::load(&fixture.path().join("lexicons/registry.toml")).unwrap();

    assert!(matches!(
        audit_repository(fixture.path(), &registry),
        Err(XtaskError::BuildContract { message }) if message.contains("contract drift")
    ));
}

#[test]
fn multilingual_policy_drift_is_rejected() {
    let fixture = copied_lexicons();
    let policy = fixture.path().join("lexicons/policies/es-v1.toml");
    let encoded = fs::read_to_string(&policy)
        .unwrap()
        .replace("max_word_length = 15", "max_word_length = 16");
    fs::write(policy, encoded).unwrap();
    let registry = PackRegistry::load(&fixture.path().join("lexicons/registry.toml")).unwrap();

    assert!(matches!(
        audit_repository(fixture.path(), &registry),
        Err(XtaskError::Builder(BuilderError::InvalidPolicy {
            field: "max_word_length",
            ..
        }))
    ));
}

#[test]
fn missing_reviewer_record_is_rejected() {
    let fixture = copied_lexicons();
    fs::remove_file(fixture.path().join("lexicons/reviews/de-v1.toml")).unwrap();
    let registry = PackRegistry::load(&fixture.path().join("lexicons/registry.toml")).unwrap();

    assert!(matches!(
        audit_repository(fixture.path(), &registry),
        Err(XtaskError::RegistryRead { .. })
    ));
}

#[test]
fn approval_without_reviewer_evidence_is_rejected() {
    let fixture = copied_lexicons();
    let review = fixture.path().join("lexicons/reviews/es-v1.toml");
    let encoded = fs::read_to_string(&review)
        .unwrap()
        .replace("status = \"pending\"", "status = \"approved\"");
    fs::write(review, encoded).unwrap();
    let registry = PackRegistry::load(&fixture.path().join("lexicons/registry.toml")).unwrap();

    assert!(matches!(
        audit_repository(fixture.path(), &registry),
        Err(XtaskError::BuildContract { message }) if message.contains("lacks reviewer evidence")
    ));
}

fn copied_lexicons() -> TempDir {
    let fixture = TempDir::new().unwrap();
    copy_tree(
        &workspace_root().join("lexicons"),
        &fixture.path().join("lexicons"),
    );
    fixture
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn workspace_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}
