use std::{
    fs,
    path::{Path, PathBuf},
};

use tempfile::TempDir;
use word_arena_lexicon::{
    CacheDecision, CompatibilityContext, CompatibilityError, ENGLISH_NORMALIZATION_PROFILE,
    FRENCH_NORMALIZATION_PROFILE, GERMAN_NORMALIZATION_PROFILE, NormalizedKey, NormalizedKeyError,
    PackError, PackManifest, SPANISH_NORMALIZATION_PROFILE, ensure_exact_pack, normalize_key,
    plan_cache_install, validate_pack,
};

const GOLDEN_FILES: [&str; 7] = [
    "manifest.toml",
    "lexicon.fst",
    "curation/additions.toml",
    "curation/removals.toml",
    "LICENSE",
    "SOURCE.md",
    "THIRD_PARTY_NOTICES",
];

#[test]
fn validates_english_and_french_golden_packs() {
    let english = validate_pack(&fixture("en-v1")).expect("English golden pack must validate");
    let french = validate_pack(&fixture("fr-v1")).expect("French golden pack must validate");

    assert_eq!(english.identity().locale, "en");
    assert_eq!(
        english.identity().content_sha256,
        "451921928f1b7796ee682b6129adf51816d6db6668b351e706b3196fc167bb27"
    );
    assert_eq!(french.identity().locale, "fr");
    assert_eq!(french.manifest().source.id, "morphalou-3.1-lmf");
    assert_eq!(
        french.identity().content_sha256,
        "c8c3ff09c8822dcb3ad646e1280f9046149c5edaeef9ff22a397965023b6e645"
    );
    assert_ne!(english.identity(), french.identity());
}

#[test]
fn content_identity_ignores_manifest_and_directory_enumeration_order() {
    let source = fixture("en-v1");
    let destination = TempDir::new().expect("temporary pack root");
    copy_fixture_in_reverse(&source, destination.path());
    let mut reordered_manifest = read_manifest(destination.path());
    reordered_manifest.files.reverse();
    write_manifest(destination.path(), &reordered_manifest);

    let source_pack = validate_pack(&source).expect("source fixture must validate");
    let copied_pack = validate_pack(destination.path()).expect("reordered fixture must validate");

    assert_eq!(source_pack.identity(), copied_pack.identity());
}

#[test]
fn rejects_unknown_manifest_fields() {
    let error =
        validate_pack(&fixture("malformed-unknown-field")).expect_err("unknown fields must fail");
    let PackError::ManifestSyntax { source, .. } = error else {
        panic!("expected a manifest syntax error");
    };
    assert!(source.to_string().contains("unknown field"));
    assert!(source.to_string().contains("future_required_field"));
}

#[test]
fn rejects_unsupported_format_and_normalization_versions() {
    assert!(matches!(
        validate_pack(&fixture("incompatible-format")),
        Err(PackError::UnsupportedFormatVersion {
            found: 99,
            supported: 1
        })
    ));

    let mut manifest = read_manifest(&fixture("en-v1"));
    manifest.normalization.version = 99;
    assert!(matches!(
        manifest.validate_schema(),
        Err(PackError::UnsupportedNormalizationVersion {
            found: 99,
            supported: 1
        })
    ));
}

#[test]
fn rejects_missing_required_records_and_payloads() {
    let mut manifest = read_manifest(&fixture("en-v1"));
    manifest.files.retain(|file| file.path != "LICENSE");
    assert!(matches!(
        manifest.validate_schema(),
        Err(PackError::MissingRequiredFileRecord { path: "LICENSE" })
    ));

    let temporary = copied_fixture("en-v1");
    fs::remove_file(temporary.path().join("LICENSE")).expect("remove fixture payload");
    assert!(matches!(
        validate_pack(temporary.path()),
        Err(PackError::MissingPayloadFile { relative_path, .. }) if relative_path == "LICENSE"
    ));
}

#[test]
fn rejects_unlisted_files() {
    let temporary = copied_fixture("en-v1");
    fs::write(temporary.path().join("stray.txt"), b"not in manifest\n")
        .expect("write unlisted fixture file");

    assert!(matches!(
        validate_pack(temporary.path()),
        Err(PackError::UnexpectedPayloadFile { path }) if path == "stray.txt"
    ));
}

#[test]
fn rejects_per_file_checksum_mismatches() {
    let temporary = copied_fixture("en-v1");
    let path = temporary.path().join("lexicon.fst");
    let mut bytes = fs::read(&path).expect("read fixture FST");
    bytes[0] ^= 1;
    fs::write(&path, bytes).expect("mutate fixture without changing its length");

    assert!(matches!(
        validate_pack(temporary.path()),
        Err(PackError::FileChecksumMismatch { path, .. }) if path == "lexicon.fst"
    ));
}

#[test]
fn rejects_pack_content_checksum_mismatches() {
    let temporary = copied_fixture("fr-v1");
    let mut manifest = read_manifest(temporary.path());
    manifest.content_sha256 = "0".repeat(64);
    write_manifest(temporary.path(), &manifest);

    assert!(matches!(
        validate_pack(temporary.path()),
        Err(PackError::ContentChecksumMismatch { expected, .. }) if expected == "0".repeat(64)
    ));
}

#[test]
fn rejects_missing_manifest_with_an_actionable_path() {
    let temporary = TempDir::new().expect("temporary pack root");
    let expected = temporary.path().join("manifest.toml");
    assert!(matches!(
        validate_pack(temporary.path()),
        Err(PackError::MissingManifest { path, .. }) if path == expected
    ));
}

#[test]
fn normalization_v1_produces_exact_utf8_board_keys() {
    assert_eq!(
        normalize_key(ENGLISH_NORMALIZATION_PROFILE, "Agent")
            .expect("English key")
            .as_ref(),
        "AGENT"
    );
    assert!(matches!(
        normalize_key(ENGLISH_NORMALIZATION_PROFILE, "café"),
        Err(NormalizedKeyError::UnsupportedCharacter { .. })
    ));
    assert_eq!(
        normalize_key(FRENCH_NORMALIZATION_PROFILE, "cœur")
            .expect("French ligature key")
            .as_ref(),
        "COEUR"
    );
    assert_eq!(
        normalize_key(FRENCH_NORMALIZATION_PROFILE, "Été")
            .expect("French accent-folded key")
            .as_ref(),
        "ETE"
    );
    assert_eq!(
        normalize_key(GERMAN_NORMALIZATION_PROFILE, "Füße")
            .expect("German folded key")
            .as_ref(),
        "FUSSE"
    );
    assert_eq!(
        normalize_key(SPANISH_NORMALIZATION_PROFILE, "niñez")
            .expect("Spanish folded key")
            .as_ref(),
        "NINEZ"
    );
    assert_eq!(
        normalize_key(SPANISH_NORMALIZATION_PROFILE, "vergüenza")
            .expect("Spanish diaeresis-folded key")
            .as_ref(),
        "VERGUENZA"
    );
    assert_eq!(
        NormalizedKey::from_utf8(b"BONJOUR".to_vec())
            .expect("valid UTF-8 key")
            .as_bytes(),
        b"BONJOUR"
    );
    assert!(matches!(
        NormalizedKey::from_utf8(vec![0xff]),
        Err(NormalizedKeyError::InvalidUtf8(_))
    ));
}

#[test]
fn manifests_bind_german_and_spanish_to_their_exact_profiles() {
    let mut german = read_manifest(&fixture("en-v1"));
    german.locale = "de".to_owned();
    german.normalization.profile = GERMAN_NORMALIZATION_PROFILE.to_owned();
    german
        .validate_schema()
        .expect("German profile must be understood by format V1");

    let mut spanish = german.clone();
    spanish.locale = "es".to_owned();
    spanish.normalization.profile = SPANISH_NORMALIZATION_PROFILE.to_owned();
    spanish
        .validate_schema()
        .expect("Spanish profile must be understood by format V1");

    spanish.normalization.profile = GERMAN_NORMALIZATION_PROFILE.to_owned();
    assert!(matches!(
        spanish.validate_schema(),
        Err(PackError::UnsupportedNormalizationProfile { locale, .. }) if locale == "es"
    ));
}

#[test]
fn rulesets_replays_and_active_games_require_exact_identity() {
    let pack = validate_pack(&fixture("en-v1")).expect("English golden pack");
    let expected = pack.identity();
    for context in [
        CompatibilityContext::Ruleset,
        CompatibilityContext::Replay,
        CompatibilityContext::ActiveGame,
    ] {
        ensure_exact_pack(context, expected, expected).expect("exact identity must be accepted");

        let mut changed = expected.clone();
        changed.content_sha256 = "f".repeat(64);
        assert!(matches!(
            ensure_exact_pack(context, expected, &changed),
            Err(CompatibilityError::ExactPackRequired { context: error_context, .. })
                if error_context == context
        ));
    }
}

#[test]
fn cache_is_idempotent_side_by_side_and_conflict_safe() {
    let installed = validate_pack(&fixture("en-v1"))
        .expect("English golden pack")
        .identity()
        .clone();
    assert_eq!(
        plan_cache_install(std::slice::from_ref(&installed), &installed)
            .expect("exact identity is idempotent"),
        CacheDecision::AlreadyInstalled
    );

    let mut next_version = installed.clone();
    next_version.pack_version = "1.1.0".to_owned();
    assert_eq!(
        plan_cache_install(std::slice::from_ref(&installed), &next_version)
            .expect("new version installs alongside"),
        CacheDecision::InstallAlongside
    );

    let mut conflict = installed.clone();
    conflict.content_sha256 = "f".repeat(64);
    assert!(matches!(
        plan_cache_install(&[installed], &conflict),
        Err(CompatibilityError::ConflictingPackVersion { .. })
    ));
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn copied_fixture(name: &str) -> TempDir {
    let temporary = TempDir::new().expect("temporary pack root");
    copy_fixture_in_reverse(&fixture(name), temporary.path());
    temporary
}

fn copy_fixture_in_reverse(source: &Path, destination: &Path) {
    for relative in GOLDEN_FILES.iter().rev() {
        let target = destination.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).expect("create fixture parent");
        }
        fs::copy(source.join(relative), target).expect("copy golden fixture");
    }
}

fn read_manifest(root: &Path) -> PackManifest {
    let value = fs::read_to_string(root.join("manifest.toml")).expect("read manifest fixture");
    toml::from_str(&value).expect("parse manifest fixture")
}

fn write_manifest(root: &Path, manifest: &PackManifest) {
    let value = toml::to_string_pretty(manifest).expect("serialize manifest fixture");
    fs::write(root.join("manifest.toml"), value).expect("write manifest fixture");
}
