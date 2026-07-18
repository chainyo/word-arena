use std::{
    collections::BTreeSet,
    fmt::Write as _,
    fs::{self, File},
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

use flate2::read::GzDecoder;
use serde::Deserialize;
use word_arena_lexicon::PackManifest;

use crate::{
    PackRegistry, XtaskError,
    artifact::{copy_noclobber, create_deterministic_archive, sha256_file},
};

const RELEASE_SCHEMA_VERSION: u32 = 1;
const SOURCE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseConfig {
    schema_version: u32,
    release_version: String,
    tag: String,
    immutable_releases_required: bool,
    packs: Vec<ReleasePack>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleasePack {
    pack_id: String,
    pack_version: String,
    source_id: String,
    compiled_asset: String,
    source_asset: String,
    keys_asset: String,
    audit_asset: String,
}

#[derive(Debug, Deserialize)]
struct SourceRegistry {
    schema_version: u32,
    sources: Vec<SourceRecord>,
}

#[derive(Debug, Deserialize)]
struct SourceRecord {
    id: String,
    archive_size_bytes: u64,
    archive_sha256: String,
}

/// Verified immutable files ready to attach to a GitHub release draft.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleasePackageSummary {
    /// Independent lexicon release tag.
    pub tag: String,
    /// Complete release-asset directory.
    pub output_directory: PathBuf,
    /// Sorted attached filenames, including `SHA256SUMS`.
    pub assets: Vec<String>,
}

/// Verifies source-build outputs and atomically assembles release attachments.
///
/// # Errors
///
/// Returns [`XtaskError`] when configuration, compiled/source checksums,
/// manifests, legible keys, audits, materials, or output publication fails.
pub fn package_release(
    workspace_root: &Path,
    input_directory: &Path,
    output_directory: &Path,
) -> Result<ReleasePackageSummary, XtaskError> {
    if output_directory.exists() {
        return Err(XtaskError::ArtifactOutputExists {
            path: output_directory.to_path_buf(),
        });
    }
    let release = load_toml::<ReleaseConfig>(&workspace_root.join("lexicons/release.toml"))?;
    verify_release_config(&release)?;
    let registry = PackRegistry::load(&workspace_root.join("lexicons/registry.toml"))?;
    let sources = load_toml::<SourceRegistry>(&workspace_root.join("lexicons/sources.toml"))?;
    if sources.schema_version != SOURCE_SCHEMA_VERSION {
        return release_error("lexicons/sources.toml must use schema version 1");
    }
    let parent = output_directory
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| XtaskError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let staging = tempfile::Builder::new()
        .prefix(".lexicon-release-")
        .tempdir_in(parent)
        .map_err(|source| XtaskError::Io {
            path: parent.to_path_buf(),
            source,
        })?;

    for pack in &release.packs {
        verify_and_copy_pack(pack, &registry, &sources, input_directory, staging.path())?;
    }

    publish_standalone_documents(workspace_root, staging.path())?;
    publish_build_materials(workspace_root, &release, staging.path())?;
    let notes_name = format!(
        "word-arena-lexicons-{}-release-notes.md",
        release.release_version
    );
    copy_noclobber(
        &workspace_root.join("lexicons/RELEASE_NOTES.md"),
        &staging.path().join(notes_name),
    )?;
    let metadata_name = format!(
        "word-arena-lexicons-{}-release.toml",
        release.release_version
    );
    copy_noclobber(
        &workspace_root.join("lexicons/release.toml"),
        &staging.path().join(metadata_name),
    )?;
    write_checksums(staging.path())?;

    let mut assets = regular_filenames(staging.path())?;
    assets.sort_unstable();
    let staging_path = staging.keep();
    fs::rename(&staging_path, output_directory).map_err(|source| XtaskError::Io {
        path: output_directory.to_path_buf(),
        source,
    })?;
    Ok(ReleasePackageSummary {
        tag: release.tag,
        output_directory: output_directory.to_path_buf(),
        assets,
    })
}

fn verify_and_copy_pack(
    pack: &ReleasePack,
    registry: &PackRegistry,
    sources: &SourceRegistry,
    input_directory: &Path,
    output_directory: &Path,
) -> Result<(), XtaskError> {
    let record = registry.require(&pack.pack_id)?;
    if record.pack_version != pack.pack_version {
        return release_error(format!(
            "release pack {} version {} differs from registry version {}",
            pack.pack_id, pack.pack_version, record.pack_version
        ));
    }
    let source = sources
        .sources
        .iter()
        .find(|source| source.id == pack.source_id)
        .ok_or_else(|| XtaskError::UnknownSource {
            source_id: pack.source_id.clone(),
        })?;
    let compiled = input_directory.join(&pack.compiled_asset);
    verify_file(
        &compiled,
        record.artifact_size_bytes,
        &record.artifact_sha256,
        &pack.compiled_asset,
    )?;
    let manifest = read_archive_manifest(&compiled)?;
    if manifest.identity() != record.identity()
        || manifest.source.license_id != record.license_id
        || manifest.source.id != pack.source_id
    {
        return release_error(format!(
            "compiled asset {} manifest differs from registry/release source and license",
            pack.compiled_asset
        ));
    }
    verify_file(
        &input_directory.join(&pack.source_asset),
        source.archive_size_bytes,
        &source.archive_sha256,
        &pack.source_asset,
    )?;
    let key_count = count_gzip_lines(&input_directory.join(&pack.keys_asset))?;
    if key_count != manifest.word_count {
        return release_error(format!(
            "legible asset {} contains {key_count} keys but manifest declares {}",
            pack.keys_asset, manifest.word_count
        ));
    }
    if count_gzip_lines(&input_directory.join(&pack.audit_asset))? == 0 {
        return release_error(format!("audit asset {} is empty", pack.audit_asset));
    }
    for filename in [
        &pack.compiled_asset,
        &pack.source_asset,
        &pack.keys_asset,
        &pack.audit_asset,
    ] {
        copy_noclobber(
            &input_directory.join(filename),
            &output_directory.join(filename),
        )?;
    }
    Ok(())
}

fn verify_release_config(release: &ReleaseConfig) -> Result<(), XtaskError> {
    if release.schema_version != RELEASE_SCHEMA_VERSION
        || release.release_version.is_empty()
        || release.tag != format!("lexicons-v{}", release.release_version)
        || !release.immutable_releases_required
        || release.packs.len() != 2
    {
        return release_error(
            "release config must use schema 1, an independent lexicons-v<version> tag, immutable releases, and exactly two packs",
        );
    }
    let ids = release
        .packs
        .iter()
        .map(|pack| pack.pack_id.as_str())
        .collect::<BTreeSet<_>>();
    if ids != BTreeSet::from(["word-arena-en-world-v1", "word-arena-fr-v1"]) {
        return release_error("release config must contain the English and French V1 pack IDs");
    }
    let mut filenames = BTreeSet::new();
    for pack in &release.packs {
        for filename in [
            &pack.compiled_asset,
            &pack.source_asset,
            &pack.keys_asset,
            &pack.audit_asset,
        ] {
            if !portable_filename(filename) || !filenames.insert(filename.as_str()) {
                return release_error("release asset filenames must be unique portable basenames");
            }
        }
    }
    Ok(())
}

fn publish_standalone_documents(workspace_root: &Path, output: &Path) -> Result<(), XtaskError> {
    for (source, destination) in [
        ("lexicons/registry.toml", "registry.toml"),
        ("lexicons/sources.toml", "sources.toml"),
        (
            "lexicons/licenses/SCOWL-v1-Copyright.txt",
            "SCOWL-v1-Copyright.txt",
        ),
        ("lexicons/licenses/LGPLLR.txt", "LGPLLR.txt"),
        ("lexicons/THIRD_PARTY_NOTICES.md", "THIRD_PARTY_NOTICES.md"),
        ("lexicons/CURATION.md", "CURATION.md"),
        ("lexicons/ENGLISH_BUILD.md", "ENGLISH_BUILD.md"),
        ("lexicons/FRENCH_BUILD.md", "FRENCH_BUILD.md"),
        ("lexicons/PACK_FORMAT.md", "PACK_FORMAT.md"),
    ] {
        copy_noclobber(&workspace_root.join(source), &output.join(destination))?;
    }
    Ok(())
}

fn publish_build_materials(
    workspace_root: &Path,
    release: &ReleaseConfig,
    output: &Path,
) -> Result<(), XtaskError> {
    let materials = tempfile::TempDir::new().map_err(|source| XtaskError::Io {
        path: std::env::temp_dir(),
        source,
    })?;
    for relative in [
        "AGENTS.md",
        "Cargo.lock",
        "Cargo.toml",
        "LICENSE",
        "README.md",
        "rust-toolchain.toml",
        ".github/workflows/lexicon-release.yml",
        "crates/lexicon/Cargo.toml",
        "crates/lexicon/src",
        "crates/lexicon-builder/Cargo.toml",
        "crates/lexicon-builder/src",
        "xtask/Cargo.toml",
        "xtask/src",
        "lexicons",
        "docs/LOCAL_SETUP.md",
        "docs/LEXICON_GAMEPLAY.md",
    ] {
        copy_tree_entry(
            &workspace_root.join(relative),
            &materials.path().join(relative),
        )?;
    }
    create_deterministic_archive(
        materials.path(),
        &output.join(format!(
            "word-arena-lexicons-{}-build-materials.tar.gz",
            release.release_version
        )),
    )
}

fn copy_tree_entry(source: &Path, destination: &Path) -> Result<(), XtaskError> {
    let metadata = fs::symlink_metadata(source).map_err(|source_error| XtaskError::Io {
        path: source.to_path_buf(),
        source: source_error,
    })?;
    if metadata.file_type().is_file() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|source| XtaskError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::copy(source, destination)
            .map(|_| ())
            .map_err(|source_error| XtaskError::Io {
                path: source.to_path_buf(),
                source: source_error,
            })
    } else if metadata.file_type().is_dir() {
        fs::create_dir_all(destination).map_err(|source| XtaskError::Io {
            path: destination.to_path_buf(),
            source,
        })?;
        for entry in fs::read_dir(source).map_err(|source_error| XtaskError::Io {
            path: source.to_path_buf(),
            source: source_error,
        })? {
            let entry = entry.map_err(|source_error| XtaskError::Io {
                path: source.to_path_buf(),
                source: source_error,
            })?;
            copy_tree_entry(&entry.path(), &destination.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        release_error(format!(
            "build materials contain unsupported entry {}",
            source.display()
        ))
    }
}

fn read_archive_manifest(path: &Path) -> Result<PackManifest, XtaskError> {
    let file = File::open(path).map_err(|source| XtaskError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut archive = tar::Archive::new(GzDecoder::new(BufReader::new(file)));
    for entry in archive.entries().map_err(|source| XtaskError::Io {
        path: path.to_path_buf(),
        source,
    })? {
        let mut entry = entry.map_err(|source| XtaskError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if entry.path().map_err(|source| XtaskError::Io {
            path: path.to_path_buf(),
            source,
        })? == Path::new("manifest.toml")
        {
            let mut encoded = String::new();
            entry
                .read_to_string(&mut encoded)
                .map_err(|source| XtaskError::Io {
                    path: path.to_path_buf(),
                    source,
                })?;
            return toml::from_str(&encoded).map_err(|source| XtaskError::RegistrySyntax {
                path: path.to_path_buf(),
                source,
            });
        }
    }
    release_error(format!(
        "compiled asset {} has no manifest.toml",
        path.display()
    ))
}

fn count_gzip_lines(path: &Path) -> Result<u64, XtaskError> {
    let file = File::open(path).map_err(|source| XtaskError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut reader = BufReader::new(GzDecoder::new(file));
    let mut line = String::new();
    let mut count = 0_u64;
    loop {
        line.clear();
        let bytes = reader
            .read_line(&mut line)
            .map_err(|source| XtaskError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        if bytes == 0 {
            break;
        }
        count = count
            .checked_add(1)
            .ok_or_else(|| XtaskError::BuildContract {
                message: format!("line count overflow in {}", path.display()),
            })?;
    }
    Ok(count)
}

fn verify_file(
    path: &Path,
    expected_size: u64,
    expected_sha256: &str,
    label: &str,
) -> Result<(), XtaskError> {
    let actual_size = fs::metadata(path)
        .map_err(|source| XtaskError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    let actual_sha256 = sha256_file(path)?;
    if actual_size == expected_size && actual_sha256 == expected_sha256 {
        Ok(())
    } else {
        release_error(format!(
            "release asset {label} expected {expected_size} bytes and SHA-256 {expected_sha256}, found {actual_size} bytes and {actual_sha256}"
        ))
    }
}

fn write_checksums(root: &Path) -> Result<(), XtaskError> {
    let mut filenames = regular_filenames(root)?;
    filenames.sort_unstable();
    let mut output = String::new();
    for filename in filenames {
        let _ = writeln!(
            output,
            "{}  {filename}",
            sha256_file(&root.join(&filename))?
        );
    }
    fs::write(root.join("SHA256SUMS"), output).map_err(|source| XtaskError::Io {
        path: root.join("SHA256SUMS"),
        source,
    })
}

fn regular_filenames(root: &Path) -> Result<Vec<String>, XtaskError> {
    let mut filenames = Vec::new();
    for entry in fs::read_dir(root).map_err(|source| XtaskError::Io {
        path: root.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| XtaskError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if !entry
            .file_type()
            .map_err(|source| XtaskError::Io {
                path: path.clone(),
                source,
            })?
            .is_file()
        {
            return release_error(format!(
                "release output contains non-file {}",
                path.display()
            ));
        }
        filenames.push(
            entry
                .file_name()
                .into_string()
                .map_err(|_| XtaskError::BuildContract {
                    message: format!("release output filename is not UTF-8 at {}", path.display()),
                })?,
        );
    }
    Ok(filenames)
}

fn portable_filename(filename: &str) -> bool {
    !filename.is_empty()
        && filename != "."
        && filename != ".."
        && !filename.contains(['/', '\\'])
        && filename.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
}

fn load_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, XtaskError> {
    let encoded = fs::read_to_string(path).map_err(|source| XtaskError::RegistryRead {
        path: path.to_path_buf(),
        source,
    })?;
    toml::from_str(&encoded).map_err(|source| XtaskError::RegistrySyntax {
        path: path.to_path_buf(),
        source,
    })
}

fn release_error<T>(message: impl Into<String>) -> Result<T, XtaskError> {
    Err(XtaskError::BuildContract {
        message: message.into(),
    })
}
