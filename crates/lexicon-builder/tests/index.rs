use std::{fs, path::Path};

use fst::Set;
use tempfile::TempDir;
use word_arena_lexicon::{ENGLISH_NORMALIZATION_PROFILE, FRENCH_NORMALIZATION_PROFILE};
use word_arena_lexicon_builder::{BuilderError, compile_index};

#[test]
fn two_compilations_are_byte_identical_and_queryable() {
    let fixture = TempDir::new().unwrap();
    let keys = fixture.path().join("keys.txt");
    fs::write(&keys, "AGENT\nCAT\nDOG\nETE\n").unwrap();
    let first_path = fixture.path().join("first.fst");
    let second_path = fixture.path().join("second.fst");

    let first = compile_index(&keys, &first_path, ENGLISH_NORMALIZATION_PROFILE).unwrap();
    let second = compile_index(&keys, &second_path, ENGLISH_NORMALIZATION_PROFILE).unwrap();

    assert_eq!(first.word_count, 4);
    assert_eq!(first.word_count, second.word_count);
    assert_eq!(first.size_bytes, second.size_bytes);
    assert_eq!(first.sha256, second.sha256);
    assert_eq!(
        fs::read(&first_path).unwrap(),
        fs::read(&second_path).unwrap()
    );

    let index = Set::new(fs::read(first_path).unwrap()).expect("compiled FST must open");
    assert_eq!(index.len(), 4);
    assert!(index.contains("AGENT"));
    assert!(index.contains("ETE"));
    assert!(!index.contains("OWL"));
}

#[test]
fn accepts_exact_french_folded_keys() {
    let fixture = TempDir::new().unwrap();
    let keys = fixture.path().join("keys.txt");
    fs::write(&keys, "COEUR\nECOLE\nETE\n").unwrap();
    let output = fixture.path().join("lexicon.fst");

    let summary = compile_index(&keys, &output, FRENCH_NORMALIZATION_PROFILE).unwrap();
    assert_eq!(summary.word_count, 3);
    let index = Set::new(fs::read(output).unwrap()).unwrap();
    assert!(index.contains("COEUR"));
    assert!(index.contains("ECOLE"));
}

#[test]
fn rejects_malformed_unordered_and_unsupported_inputs_atomically() {
    for (name, input) in [
        ("duplicate", "CAT\nCAT\n"),
        ("unordered", "DOG\nCAT\n"),
        ("source spelling", "CAT\ncafe\n"),
        ("blank", "CAT\n\n"),
        ("crlf", "CAT\r\nDOG\r\n"),
    ] {
        assert_invalid_input(name, input);
    }

    let fixture = TempDir::new().unwrap();
    let keys = fixture.path().join("keys.txt");
    let output = fixture.path().join("lexicon.fst");
    fs::write(&keys, "CAT\n").unwrap();
    assert!(matches!(
        compile_index(&keys, &output, "unknown-profile"),
        Err(BuilderError::UnsupportedIndexProfile { profile }) if profile == "unknown-profile"
    ));
    assert!(!output.exists());
}

#[test]
fn never_overwrites_an_existing_index() {
    let fixture = TempDir::new().unwrap();
    let keys = fixture.path().join("keys.txt");
    let output = fixture.path().join("lexicon.fst");
    fs::write(&keys, "CAT\n").unwrap();
    fs::write(&output, b"existing bytes").unwrap();

    assert!(matches!(
        compile_index(&keys, &output, ENGLISH_NORMALIZATION_PROFILE),
        Err(BuilderError::OutputExists { path }) if path == output
    ));
    assert_eq!(fs::read(output).unwrap(), b"existing bytes");
}

fn assert_invalid_input(name: &str, input: &str) {
    let fixture = TempDir::new().unwrap();
    let keys = fixture.path().join("keys.txt");
    let output = fixture.path().join("output/lexicon.fst");
    fs::write(&keys, input).unwrap();
    assert!(
        matches!(
            compile_index(&keys, &output, ENGLISH_NORMALIZATION_PROFILE),
            Err(BuilderError::InvalidIndexKey { .. })
        ),
        "{name} must fail"
    );
    assert!(!output.exists(), "{name} published a partial index");
    assert_no_staging_files(output.parent().unwrap());
}

fn assert_no_staging_files(directory: &Path) {
    let entries = fs::read_dir(directory)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(
        entries.is_empty(),
        "failed builds must clean their staging files"
    );
}
