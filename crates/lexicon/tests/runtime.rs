use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use fst::SetBuilder;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use word_arena_lexicon::{
    INDEX_FILE, PackError, PackManifest, calculate_content_sha256, load_lexicon, normalize_key,
};

const PACK_FILES: [&str; 7] = [
    "manifest.toml",
    INDEX_FILE,
    "curation/additions.toml",
    "curation/removals.toml",
    "LICENSE",
    "SOURCE.md",
    "THIRD_PARTY_NOTICES",
];

#[test]
fn verified_reader_exposes_manifest_and_exact_membership() {
    let english = runtime_pack("en-v1", &[b"AGENT", b"CAT", b"DOG"], 3, None);
    let lexicon = load_lexicon(english.path()).expect("valid runtime pack must load");
    assert_eq!(lexicon.manifest().pack_id, "word-arena-en-world-v1");
    assert_eq!(lexicon.identity(), &lexicon.manifest().identity());
    assert_eq!(lexicon.word_count(), 3);

    let cat = normalize_key(&lexicon.manifest().normalization.profile, "cat").unwrap();
    let owl = normalize_key(&lexicon.manifest().normalization.profile, "owl").unwrap();
    for _ in 0..10_000 {
        assert!(lexicon.contains(&cat));
        assert!(!lexicon.contains(&owl));
    }

    let french = runtime_pack("fr-v1", &[b"COEUR", b"ETE"], 2, None);
    let lexicon = load_lexicon(french.path()).expect("valid French runtime pack must load");
    let coeur = normalize_key(&lexicon.manifest().normalization.profile, "cœur").unwrap();
    let ete = normalize_key(&lexicon.manifest().normalization.profile, "Été").unwrap();
    assert!(lexicon.contains(&coeur));
    assert!(lexicon.contains(&ete));
}

#[test]
fn loaded_instances_keep_owned_immutable_bytes() {
    let old_pack = runtime_pack("en-v1", &[b"CAT"], 1, None);
    let old = load_lexicon(old_pack.path()).unwrap();
    let old_identity = old.identity().clone();
    fs::remove_dir_all(old_pack.path()).unwrap();

    let new_pack = runtime_pack("en-v1", &[b"OWL"], 1, Some("1.1.0"));
    let new = load_lexicon(new_pack.path()).unwrap();
    let cat = normalize_key("en-basic-latin-v1", "CAT").unwrap();
    let owl = normalize_key("en-basic-latin-v1", "OWL").unwrap();

    assert!(old.contains(&cat));
    assert!(!old.contains(&owl));
    assert!(!new.contains(&cat));
    assert!(new.contains(&owl));
    assert_eq!(old.identity(), &old_identity);
    assert_eq!(new.identity().pack_version, "1.1.0");
    assert_ne!(old.identity(), new.identity());
}

#[test]
fn rejects_outer_corruption_before_exposing_queries() {
    let pack = runtime_pack("en-v1", &[b"CAT", b"DOG"], 2, None);
    let path = pack.path().join(INDEX_FILE);
    let mut bytes = fs::read(&path).unwrap();
    let midpoint = bytes.len() / 2;
    bytes[midpoint] ^= 1;
    fs::write(path, bytes).unwrap();

    assert!(matches!(
        load_lexicon(pack.path()),
        Err(PackError::FileChecksumMismatch { path, .. }) if path == INDEX_FILE
    ));
}

#[test]
fn rejects_self_consistent_truncated_and_invalid_indexes() {
    let truncated = runtime_pack("en-v1", &[b"CAT", b"DOG"], 2, None);
    let mut bytes = fs::read(truncated.path().join(INDEX_FILE)).unwrap();
    bytes.truncate(12);
    replace_index(truncated.path(), &bytes, 2, None);
    assert!(matches!(
        load_lexicon(truncated.path()),
        Err(PackError::InvalidIndex { .. })
    ));

    for invalid_key in [&b"cat"[..], &[0xff][..]] {
        let invalid = runtime_pack("en-v1", &[invalid_key], 1, None);
        assert!(matches!(
            load_lexicon(invalid.path()),
            Err(PackError::InvalidIndexKey { position: 1, .. })
        ));
    }
}

#[test]
fn rejects_word_count_and_unsupported_contract_mismatches() {
    let mismatched = runtime_pack("en-v1", &[b"CAT", b"DOG"], 3, None);
    assert!(matches!(
        load_lexicon(mismatched.path()),
        Err(PackError::IndexWordCountMismatch {
            expected: 3,
            actual: 2
        })
    ));

    assert!(matches!(
        load_lexicon(&fixture("incompatible-format")),
        Err(PackError::UnsupportedFormatVersion {
            found: 99,
            supported: 1
        })
    ));
}

fn runtime_pack(
    source_fixture: &str,
    keys: &[&[u8]],
    declared_count: u64,
    pack_version: Option<&str>,
) -> TempDir {
    let pack = copied_fixture(source_fixture);
    let bytes = fst_bytes(keys);
    replace_index(pack.path(), &bytes, declared_count, pack_version);
    pack
}

fn fst_bytes(keys: &[&[u8]]) -> Vec<u8> {
    let mut builder = SetBuilder::memory();
    for key in keys {
        builder.insert(key).unwrap();
    }
    builder.into_inner().unwrap()
}

fn replace_index(root: &Path, bytes: &[u8], word_count: u64, pack_version: Option<&str>) {
    fs::write(root.join(INDEX_FILE), bytes).unwrap();
    let mut manifest = read_manifest(root);
    manifest.word_count = word_count;
    if let Some(version) = pack_version {
        version.clone_into(&mut manifest.pack_version);
    }
    let descriptor = manifest
        .files
        .iter_mut()
        .find(|descriptor| descriptor.path == INDEX_FILE)
        .unwrap();
    descriptor.size_bytes = u64::try_from(bytes.len()).unwrap();
    descriptor.sha256 = sha256_hex(bytes);
    manifest.content_sha256 = calculate_content_sha256(root, &manifest.files).unwrap();
    write_manifest(root, &manifest);
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn copied_fixture(name: &str) -> TempDir {
    let temporary = TempDir::new().unwrap();
    for relative in PACK_FILES {
        let destination = temporary.path().join(relative);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(fixture(name).join(relative), destination).unwrap();
    }
    temporary
}

fn read_manifest(root: &Path) -> PackManifest {
    toml::from_str(&fs::read_to_string(root.join("manifest.toml")).unwrap()).unwrap()
}

fn write_manifest(root: &Path, manifest: &PackManifest) {
    fs::write(
        root.join("manifest.toml"),
        toml::to_string_pretty(manifest).unwrap(),
    )
    .unwrap();
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}
