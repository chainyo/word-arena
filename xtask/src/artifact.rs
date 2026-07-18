use std::{
    fmt::Write as _,
    fs::{self, File},
    io::{BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
};

use flate2::{Compression, GzBuilder};
use sha2::{Digest, Sha256};
use word_arena_lexicon::{
    BuilderDescriptor, FileDescriptor, NormalizationDescriptor, PackManifest, PolicyDescriptor,
    SourceDescriptor, calculate_content_sha256, load_lexicon,
};
use word_arena_lexicon_builder::{
    BUILD_METADATA_FILE, BUILDER_NAME, BUILDER_VERSION, CURATION_CHANGELOG_FILE,
    CURATION_REPORT_FILE, FILTER_REPORT_FILE, KEYS_FILE, compile_index,
};

use crate::{PackRecord, XtaskError};

const MANIFEST_FILE: &str = "manifest.toml";
const MODIFICATION_DATE: &str = "2026-07-17";
const HASH_BUFFER_SIZE: usize = 64 * 1024;

pub(crate) struct AssemblySpec<'a> {
    pub record: &'a PackRecord,
    pub source_id: &'a str,
    pub source_name: &'a str,
    pub source_revision: &'a str,
    pub source_archive_sha256: &'a str,
    pub source_url: &'a str,
    pub policy_id: &'a str,
    pub policy_version: u32,
    pub source_build_directory: &'a Path,
    pub curation_input_directory: &'a Path,
    pub curation_output_directory: &'a Path,
    pub license_path: &'a Path,
}

/// Deterministic separately licensed artifact produced by a source build.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactBuildSummary {
    /// Pack ID.
    pub pack_id: String,
    /// Unpacked payload identity.
    pub content_sha256: String,
    /// Final deterministic `.tar.gz` path.
    pub archive_path: PathBuf,
    /// Compressed archive length.
    pub archive_size_bytes: u64,
    /// Compressed archive SHA-256.
    pub archive_sha256: String,
    /// Fully enumerated runtime word count.
    pub word_count: u64,
    /// Optional corresponding source/legible/audit release assets.
    pub release_materials: Vec<PathBuf>,
}

pub(crate) fn assemble_artifact(
    spec: &AssemblySpec<'_>,
    output_path: &Path,
) -> Result<ArtifactBuildSummary, XtaskError> {
    if output_path.exists() {
        return Err(XtaskError::ArtifactOutputExists {
            path: output_path.to_path_buf(),
        });
    }
    let workspace = tempfile::TempDir::new().map_err(|source| XtaskError::Io {
        path: std::env::temp_dir(),
        source,
    })?;
    let pack = workspace.path().join("pack");
    fs::create_dir(&pack).map_err(|source| XtaskError::Io {
        path: pack.clone(),
        source,
    })?;

    let index = compile_index(
        &spec.curation_output_directory.join(KEYS_FILE),
        &pack.join("lexicon.fst"),
        &spec.record.normalization_profile,
    )?;
    copy_pack_materials(spec, &pack)?;
    let mut files = describe_payloads(&pack)?;
    files.sort_unstable_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));
    let mut manifest = PackManifest {
        format_version: spec.record.format_version,
        pack_id: spec.record.pack_id.clone(),
        pack_version: spec.record.pack_version.clone(),
        locale: spec.record.locale.clone(),
        word_count: index.word_count,
        content_sha256: "0".repeat(64),
        normalization: NormalizationDescriptor {
            algorithm: spec.record.normalization_algorithm.clone(),
            version: spec.record.normalization_version,
            profile: spec.record.normalization_profile.clone(),
        },
        source: SourceDescriptor {
            id: spec.source_id.to_owned(),
            revision: spec.source_revision.to_owned(),
            archive_sha256: spec.source_archive_sha256.to_owned(),
            license_id: spec.record.license_id.clone(),
        },
        policy: PolicyDescriptor {
            id: spec.policy_id.to_owned(),
            version: spec.policy_version,
        },
        builder: BuilderDescriptor {
            name: BUILDER_NAME.to_owned(),
            version: BUILDER_VERSION.to_owned(),
        },
        files,
    };
    manifest.validate_schema()?;
    manifest.content_sha256 = calculate_content_sha256(&pack, &manifest.files)?;
    write_toml(&pack.join(MANIFEST_FILE), &manifest)?;
    let loaded = load_lexicon(&pack)?;
    if loaded.identity() != &manifest.identity() || loaded.word_count() != index.word_count {
        return Err(XtaskError::BuildContract {
            message: "assembled pack did not reload to its generated manifest identity".to_owned(),
        });
    }

    create_deterministic_archive(&pack, output_path)?;
    let archive_size_bytes = fs::metadata(output_path)
        .map_err(|source| XtaskError::Io {
            path: output_path.to_path_buf(),
            source,
        })?
        .len();
    Ok(ArtifactBuildSummary {
        pack_id: spec.record.pack_id.clone(),
        content_sha256: manifest.content_sha256,
        archive_path: output_path.to_path_buf(),
        archive_size_bytes,
        archive_sha256: sha256_file(output_path)?,
        word_count: index.word_count,
        release_materials: Vec::new(),
    })
}

fn copy_pack_materials(spec: &AssemblySpec<'_>, pack: &Path) -> Result<(), XtaskError> {
    copy_file(spec.license_path, &pack.join("LICENSE"))?;
    let curation = pack.join("curation");
    let build = pack.join("build");
    fs::create_dir_all(&curation).map_err(|source| XtaskError::Io {
        path: curation.clone(),
        source,
    })?;
    fs::create_dir_all(&build).map_err(|source| XtaskError::Io {
        path: build.clone(),
        source,
    })?;
    for filename in ["additions.toml", "removals.toml", "governance.toml"] {
        copy_file(
            &spec.curation_input_directory.join(filename),
            &curation.join(filename),
        )?;
    }
    for filename in [CURATION_CHANGELOG_FILE, CURATION_REPORT_FILE] {
        copy_file(
            &spec.curation_output_directory.join(filename),
            &curation.join(filename),
        )?;
    }
    for filename in [BUILD_METADATA_FILE, FILTER_REPORT_FILE] {
        copy_file(
            &spec.source_build_directory.join(filename),
            &build.join(filename),
        )?;
    }
    fs::write(pack.join("SOURCE.md"), source_notice(spec)).map_err(|source| XtaskError::Io {
        path: pack.join("SOURCE.md"),
        source,
    })?;
    fs::write(pack.join("THIRD_PARTY_NOTICES"), third_party_notice(spec)).map_err(|source| {
        XtaskError::Io {
            path: pack.join("THIRD_PARTY_NOTICES"),
            source,
        }
    })?;
    Ok(())
}

fn source_notice(spec: &AssemblySpec<'_>) -> String {
    format!(
        "# Word Arena lexicon source\n\nPack: `{}`\nPack version: `{}`\nSource: {} (`{}`)\nSource revision: `{}`\nSource archive SHA-256: `{}`\nSource URL: <{}>\nPolicy: `{}` v{}\nNormalization: `{}` v{}\nBuilder: `{}` v{}\nModified by Word Arena on {} through filtering, normalization, reviewed curation, and FST compilation.\n",
        spec.record.pack_id,
        spec.record.pack_version,
        spec.source_name,
        spec.source_id,
        spec.source_revision,
        spec.source_archive_sha256,
        spec.source_url,
        spec.policy_id,
        spec.policy_version,
        spec.record.normalization_profile,
        spec.record.normalization_version,
        BUILDER_NAME,
        BUILDER_VERSION,
        MODIFICATION_DATE,
    )
}

fn third_party_notice(spec: &AssemblySpec<'_>) -> String {
    if spec.record.locale == "en" {
        format!(
            "Word Arena English lexicon\n\nDerived from {} at revision {}. The complete SCOWLv1 collective-work and incorporated-source notices, including UKACD, WordNet, VarCon, and Ispell terms, are reproduced verbatim in LICENSE. This modified resource was generated on {} under policy {} v{}. No official tournament-list compatibility is claimed.\n",
            spec.source_name,
            spec.source_revision,
            MODIFICATION_DATE,
            spec.policy_id,
            spec.policy_version,
        )
    } else {
        format!(
            "Word Arena French lexicon\n\nDerived from {} {} by ATILF/ORTOLANG and distributed as a modified linguistic resource under LGPL-LR. Word Arena filtering, normalization, reviewed curation, and FST compilation were applied on {} under policy {} v{}. The complete LGPL-LR text is in LICENSE. Corresponding legible source and build materials accompany the release. No official tournament-list compatibility is claimed.\n",
            spec.source_name,
            spec.source_revision,
            MODIFICATION_DATE,
            spec.policy_id,
            spec.policy_version,
        )
    }
}

fn describe_payloads(root: &Path) -> Result<Vec<FileDescriptor>, XtaskError> {
    let relative_paths = [
        "LICENSE",
        "SOURCE.md",
        "THIRD_PARTY_NOTICES",
        "build/build.toml",
        "build/filter-report.toml",
        "curation/additions.toml",
        "curation/curation-changelog.md",
        "curation/curation-report.toml",
        "curation/governance.toml",
        "curation/removals.toml",
        "lexicon.fst",
    ];
    relative_paths
        .into_iter()
        .map(|relative| {
            let path = root.join(relative);
            let size_bytes = fs::metadata(&path)
                .map_err(|source| XtaskError::Io {
                    path: path.clone(),
                    source,
                })?
                .len();
            Ok(FileDescriptor {
                path: relative.to_owned(),
                size_bytes,
                sha256: sha256_file(&path)?,
            })
        })
        .collect()
}

pub(crate) fn create_deterministic_archive(
    root: &Path,
    output_path: &Path,
) -> Result<(), XtaskError> {
    let parent = output_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| XtaskError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let mut staging = tempfile::Builder::new()
        .prefix(".artifact-")
        .tempfile_in(parent)
        .map_err(|source| XtaskError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    {
        let writer = BufWriter::new(staging.as_file_mut());
        let encoder = GzBuilder::new()
            .mtime(0)
            .operating_system(255)
            .write(writer, Compression::best());
        let mut archive = tar::Builder::new(encoder);
        let mut paths = collect_regular_files(root)?;
        paths.sort_unstable_by(|left, right| left.as_os_str().cmp(right.as_os_str()));
        for relative in paths {
            let source_path = root.join(&relative);
            let mut source = File::open(&source_path).map_err(|source| XtaskError::Io {
                path: source_path.clone(),
                source,
            })?;
            let size = source
                .metadata()
                .map_err(|source| XtaskError::Io {
                    path: source_path.clone(),
                    source,
                })?
                .len();
            let mut header = tar::Header::new_gnu();
            header
                .set_path(&relative)
                .map_err(|source| XtaskError::Io {
                    path: relative.clone(),
                    source,
                })?;
            header.set_size(size);
            header.set_mode(0o644);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            header.set_cksum();
            archive
                .append(&header, &mut source)
                .map_err(|source| XtaskError::Io {
                    path: output_path.to_path_buf(),
                    source,
                })?;
        }
        let encoder = archive.into_inner().map_err(|source| XtaskError::Io {
            path: output_path.to_path_buf(),
            source,
        })?;
        let mut writer = encoder.finish().map_err(|source| XtaskError::Io {
            path: output_path.to_path_buf(),
            source,
        })?;
        writer.flush().map_err(|source| XtaskError::Io {
            path: output_path.to_path_buf(),
            source,
        })?;
    }
    staging
        .as_file()
        .sync_all()
        .map_err(|source| XtaskError::Io {
            path: staging.path().to_path_buf(),
            source,
        })?;
    staging.persist_noclobber(output_path).map_err(|error| {
        if error.error.kind() == std::io::ErrorKind::AlreadyExists {
            XtaskError::ArtifactOutputExists {
                path: output_path.to_path_buf(),
            }
        } else {
            XtaskError::Io {
                path: output_path.to_path_buf(),
                source: error.error,
            }
        }
    })?;
    Ok(())
}

pub(crate) fn create_deterministic_gzip(
    input_path: &Path,
    output_path: &Path,
) -> Result<(), XtaskError> {
    if output_path.exists() {
        return Err(XtaskError::ArtifactOutputExists {
            path: output_path.to_path_buf(),
        });
    }
    let parent = output_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| XtaskError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let input = File::open(input_path).map_err(|source| XtaskError::Io {
        path: input_path.to_path_buf(),
        source,
    })?;
    let mut reader = BufReader::new(input);
    let mut staging = tempfile::Builder::new()
        .prefix(".gzip-")
        .tempfile_in(parent)
        .map_err(|source| XtaskError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    {
        let writer = BufWriter::new(staging.as_file_mut());
        let mut encoder = GzBuilder::new()
            .mtime(0)
            .operating_system(255)
            .write(writer, Compression::best());
        std::io::copy(&mut reader, &mut encoder).map_err(|source| XtaskError::Io {
            path: output_path.to_path_buf(),
            source,
        })?;
        let mut writer = encoder.finish().map_err(|source| XtaskError::Io {
            path: output_path.to_path_buf(),
            source,
        })?;
        writer.flush().map_err(|source| XtaskError::Io {
            path: output_path.to_path_buf(),
            source,
        })?;
    }
    staging.persist_noclobber(output_path).map_err(|error| {
        if error.error.kind() == std::io::ErrorKind::AlreadyExists {
            XtaskError::ArtifactOutputExists {
                path: output_path.to_path_buf(),
            }
        } else {
            XtaskError::Io {
                path: output_path.to_path_buf(),
                source: error.error,
            }
        }
    })?;
    Ok(())
}

pub(crate) fn copy_noclobber(source_path: &Path, output_path: &Path) -> Result<(), XtaskError> {
    if output_path.exists() {
        return Err(XtaskError::ArtifactOutputExists {
            path: output_path.to_path_buf(),
        });
    }
    let parent = output_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| XtaskError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let mut input = File::open(source_path).map_err(|source| XtaskError::Io {
        path: source_path.to_path_buf(),
        source,
    })?;
    let mut staging = tempfile::Builder::new()
        .prefix(".copy-")
        .tempfile_in(parent)
        .map_err(|source| XtaskError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    std::io::copy(&mut input, staging.as_file_mut()).map_err(|source| XtaskError::Io {
        path: output_path.to_path_buf(),
        source,
    })?;
    staging.persist_noclobber(output_path).map_err(|error| {
        if error.error.kind() == std::io::ErrorKind::AlreadyExists {
            XtaskError::ArtifactOutputExists {
                path: output_path.to_path_buf(),
            }
        } else {
            XtaskError::Io {
                path: output_path.to_path_buf(),
                source: error.error,
            }
        }
    })?;
    Ok(())
}

fn collect_regular_files(root: &Path) -> Result<Vec<PathBuf>, XtaskError> {
    let mut files = Vec::new();
    collect_from(root, root, &mut files)?;
    Ok(files)
}

fn collect_from(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), XtaskError> {
    let entries = fs::read_dir(directory).map_err(|source| XtaskError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| XtaskError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| XtaskError::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_dir() {
            collect_from(root, &path, files)?;
        } else if file_type.is_file() {
            files.push(
                path.strip_prefix(root)
                    .expect("recursive entry remains beneath root")
                    .to_path_buf(),
            );
        } else {
            return Err(XtaskError::BuildContract {
                message: format!("pack staging contains non-regular entry {}", path.display()),
            });
        }
    }
    Ok(())
}

fn copy_file(source: &Path, destination: &Path) -> Result<(), XtaskError> {
    fs::copy(source, destination)
        .map(|_| ())
        .map_err(|source_error| XtaskError::Io {
            path: source.to_path_buf(),
            source: source_error,
        })
}

fn write_toml<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), XtaskError> {
    let mut encoded = toml::to_string_pretty(value)?;
    if !encoded.ends_with('\n') {
        encoded.push('\n');
    }
    fs::write(path, encoded).map_err(|source| XtaskError::Io {
        path: path.to_path_buf(),
        source,
    })
}

pub(crate) fn sha256_file(path: &Path) -> Result<String, XtaskError> {
    let file = File::open(path).map_err(|source| XtaskError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_SIZE].into_boxed_slice();
    loop {
        let read = reader.read(&mut buffer).map_err(|source| XtaskError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        let _ = write!(encoded, "{byte:02x}");
    }
    Ok(encoded)
}
