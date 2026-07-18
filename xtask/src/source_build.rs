use std::{fs, path::Path};

use serde::Deserialize;
use tempfile::TempDir;
use word_arena_lexicon_builder::{
    AUDIT_FILE, EnglishPolicy, FrenchPolicy, KEYS_FILE, apply_curation, build_english_from_archive,
    build_french_from_archive,
};

use crate::{
    ArtifactBuildSummary, PackRecord, PackRegistry, XtaskError,
    artifact::{AssemblySpec, assemble_artifact, copy_noclobber, create_deterministic_gzip},
    install::{download_with_curl, verify_tool},
};

const SOURCE_REGISTRY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
struct SourceRegistry {
    schema_version: u32,
    sources: Vec<SourceRecord>,
}

#[derive(Debug, Deserialize)]
struct SourceRecord {
    id: String,
    name: String,
    version: String,
    revision: Option<String>,
    canonical_source_url: String,
    archive_url: String,
    archive_size_bytes: u64,
    archive_sha256: String,
    license_id: String,
    license_file: String,
    archive_format: String,
}

/// Rebuilds one or all release packs from the pinned upstream archives.
///
/// Every source archive, policy, curation input, license, manifest, and output
/// archive is checked. By default, the built identities must exactly match the
/// committed install registry. `allow_registry_mismatch` exists only to
/// bootstrap an intentional new release and print the new immutable pins.
///
/// # Errors
///
/// Returns [`XtaskError`] for tool, source, policy, build, curation, pack,
/// artifact, or registry-contract failures.
pub fn build_from_source(
    workspace_root: &Path,
    registry: &PackRegistry,
    selected_pack: Option<&str>,
    output_directory: &Path,
    allow_registry_mismatch: bool,
    release_materials: bool,
) -> Result<Vec<ArtifactBuildSummary>, XtaskError> {
    verify_tool(
        "curl",
        "install curl; source release builds require the pinned upstream archives",
    )?;
    let sources = load_sources(&workspace_root.join("lexicons/sources.toml"))?;
    let records = selected_records(registry, selected_pack)?;
    fs::create_dir_all(output_directory).map_err(|source| XtaskError::Io {
        path: output_directory.to_path_buf(),
        source,
    })?;

    let mut summaries = Vec::with_capacity(records.len());
    for record in records {
        let summary = build_one(
            workspace_root,
            &sources,
            record,
            output_directory,
            release_materials,
        )?;
        if !allow_registry_mismatch {
            verify_release_identity(record, &summary)?;
        }
        summaries.push(summary);
    }
    Ok(summaries)
}

fn load_sources(path: &Path) -> Result<SourceRegistry, XtaskError> {
    let encoded = fs::read_to_string(path).map_err(|source| XtaskError::RegistryRead {
        path: path.to_path_buf(),
        source,
    })?;
    let registry: SourceRegistry =
        toml::from_str(&encoded).map_err(|source| XtaskError::RegistrySyntax {
            path: path.to_path_buf(),
            source,
        })?;
    if registry.schema_version != SOURCE_REGISTRY_SCHEMA_VERSION {
        return Err(XtaskError::BuildContract {
            message: format!(
                "lexicons/sources.toml schema_version must be {SOURCE_REGISTRY_SCHEMA_VERSION}"
            ),
        });
    }
    Ok(registry)
}

fn selected_records<'a>(
    registry: &'a PackRegistry,
    selected_pack: Option<&str>,
) -> Result<Vec<&'a PackRecord>, XtaskError> {
    match selected_pack {
        Some(pack_id) => Ok(vec![registry.require(pack_id)?]),
        None => Ok(registry.packs.iter().collect()),
    }
}

fn build_one(
    workspace_root: &Path,
    sources: &SourceRegistry,
    record: &PackRecord,
    output_directory: &Path,
    release_materials: bool,
) -> Result<ArtifactBuildSummary, XtaskError> {
    let workspace = TempDir::new().map_err(|source| XtaskError::Io {
        path: std::env::temp_dir(),
        source,
    })?;
    let source_build = workspace.path().join("source-build");
    let curated = workspace.path().join("curated");
    let source_archive = workspace.path().join("source.archive");

    let inputs = match record.locale.as_str() {
        "en" => {
            let policy =
                EnglishPolicy::load(&workspace_root.join("lexicons/policies/en-world-v1.toml"))?;
            let source = require_source(sources, &policy.source_id)?;
            verify_english_contract(record, source, &policy)?;
            download_with_curl(&source.archive_url, &source_archive)?;
            build_english_from_archive(&source_archive, &source_build, &policy)?;
            let curation_directory = workspace_root.join("lexicons/curation/en-world-v1");
            apply_curation(&source_build.join(KEYS_FILE), &curated, &curation_directory)?;
            BuildInputs {
                source,
                source_revision: policy.source_revision,
                policy_id: policy.id,
                policy_version: policy.version,
                curation_directory,
            }
        }
        "fr" => {
            let policy = FrenchPolicy::load(&workspace_root.join("lexicons/policies/fr-v1.toml"))?;
            let source = require_source(sources, &policy.source_id)?;
            verify_french_contract(record, source, &policy)?;
            download_with_curl(&source.archive_url, &source_archive)?;
            build_french_from_archive(&source_archive, &source_build, &policy)?;
            let curation_directory = workspace_root.join("lexicons/curation/fr-v1");
            apply_curation(&source_build.join(KEYS_FILE), &curated, &curation_directory)?;
            BuildInputs {
                source,
                source_revision: policy.source_revision,
                policy_id: policy.id,
                policy_version: policy.version,
                curation_directory,
            }
        }
        locale => {
            return Err(XtaskError::BuildContract {
                message: format!("unsupported source-build locale {locale:?}"),
            });
        }
    };

    let license_path = workspace_root
        .join("lexicons")
        .join(&inputs.source.license_file);
    let output_path =
        output_directory.join(format!("{}-{}.tar.gz", record.pack_id, record.pack_version));
    let mut summary = assemble_artifact(
        &AssemblySpec {
            record,
            source_id: &inputs.source.id,
            source_name: &inputs.source.name,
            source_revision: &inputs.source_revision,
            source_archive_sha256: &inputs.source.archive_sha256,
            source_url: &inputs.source.canonical_source_url,
            policy_id: &inputs.policy_id,
            policy_version: inputs.policy_version,
            source_build_directory: &source_build,
            curation_input_directory: &inputs.curation_directory,
            curation_output_directory: &curated,
            license_path: &license_path,
        },
        &output_path,
    )?;
    if release_materials {
        summary.release_materials = publish_release_materials(
            record,
            inputs.source,
            &source_archive,
            &source_build,
            &curated,
            output_directory,
        )?;
    }
    Ok(summary)
}

fn publish_release_materials(
    record: &PackRecord,
    source: &SourceRecord,
    source_archive: &Path,
    source_build: &Path,
    curated: &Path,
    output_directory: &Path,
) -> Result<Vec<std::path::PathBuf>, XtaskError> {
    let source_extension = match source.archive_format.as_str() {
        "tar.gz" => "tar.gz",
        "zip" => "zip",
        other => {
            return Err(XtaskError::BuildContract {
                message: format!("unsupported release source archive format {other:?}"),
            });
        }
    };
    let stem = format!("{}-{}", record.pack_id, record.pack_version);
    let source_output = output_directory.join(format!("{stem}-source.{source_extension}"));
    let keys_output = output_directory.join(format!("{stem}-keys.txt.gz"));
    let audit_output = output_directory.join(format!("{stem}-audit.jsonl.gz"));
    copy_noclobber(source_archive, &source_output)?;
    create_deterministic_gzip(&curated.join(KEYS_FILE), &keys_output)?;
    create_deterministic_gzip(&source_build.join(AUDIT_FILE), &audit_output)?;
    Ok(vec![source_output, keys_output, audit_output])
}

struct BuildInputs<'a> {
    source: &'a SourceRecord,
    source_revision: String,
    policy_id: String,
    policy_version: u32,
    curation_directory: std::path::PathBuf,
}

fn require_source<'a>(
    sources: &'a SourceRegistry,
    source_id: &str,
) -> Result<&'a SourceRecord, XtaskError> {
    sources
        .sources
        .iter()
        .find(|source| source.id == source_id)
        .ok_or_else(|| XtaskError::UnknownSource {
            source_id: source_id.to_owned(),
        })
}

fn verify_english_contract(
    record: &PackRecord,
    source: &SourceRecord,
    policy: &EnglishPolicy,
) -> Result<(), XtaskError> {
    verify_common_contract(
        record,
        source,
        &policy.pack_id,
        &policy.source_revision,
        &policy.source_archive_sha256,
        policy.source_archive_size_bytes,
        &policy.normalization_profile,
    )
}

fn verify_french_contract(
    record: &PackRecord,
    source: &SourceRecord,
    policy: &FrenchPolicy,
) -> Result<(), XtaskError> {
    verify_common_contract(
        record,
        source,
        &policy.pack_id,
        &policy.source_revision,
        &policy.source_archive_sha256,
        policy.source_archive_size_bytes,
        &policy.normalization_profile,
    )
}

fn verify_common_contract(
    record: &PackRecord,
    source: &SourceRecord,
    policy_pack_id: &str,
    policy_revision: &str,
    policy_archive_sha256: &str,
    policy_archive_size_bytes: u64,
    policy_normalization_profile: &str,
) -> Result<(), XtaskError> {
    let source_revision = source.revision.as_deref().unwrap_or(&source.version);
    let matches = record.pack_id == policy_pack_id
        && record.normalization_profile == policy_normalization_profile
        && record.license_id == source.license_id
        && policy_revision == source_revision
        && policy_archive_sha256 == source.archive_sha256
        && policy_archive_size_bytes == source.archive_size_bytes;
    if matches {
        Ok(())
    } else {
        Err(XtaskError::BuildContract {
            message: format!(
                "pack {}, its policy, and source {} do not declare the same identity, normalization, license, revision, archive size, and archive SHA-256",
                record.pack_id, source.id
            ),
        })
    }
}

fn verify_release_identity(
    record: &PackRecord,
    summary: &ArtifactBuildSummary,
) -> Result<(), XtaskError> {
    if record.content_sha256 == summary.content_sha256
        && record.artifact_sha256 == summary.archive_sha256
        && record.artifact_size_bytes == summary.archive_size_bytes
    {
        Ok(())
    } else {
        Err(XtaskError::RegistryArtifactMismatch {
            pack_id: record.pack_id.clone().into_boxed_str(),
            expected_content_sha256: record.content_sha256.clone().into_boxed_str(),
            expected_archive_sha256: record.artifact_sha256.clone().into_boxed_str(),
            expected_size_bytes: record.artifact_size_bytes,
            actual_content_sha256: summary.content_sha256.clone().into_boxed_str(),
            actual_archive_sha256: summary.archive_sha256.clone().into_boxed_str(),
            actual_size_bytes: summary.archive_size_bytes,
        })
    }
}
