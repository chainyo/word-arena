use std::{
    collections::BTreeSet,
    fmt::Write as _,
    fs::{self, File},
    io::{self, BufReader, Read},
    path::{Component, Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::{
    FileDescriptor, MANIFEST_FILE, PackError, PackIdentity, PackManifest,
    manifest::validate_file_path,
};

const CONTENT_HASH_DOMAIN: &[u8] = b"word-arena-pack-content-v1\0";
const READ_BUFFER_SIZE: usize = 64 * 1024;

/// A pack whose schema, complete file set, sizes, and checksums are verified.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedPack {
    root: PathBuf,
    manifest: PackManifest,
    identity: PackIdentity,
}

impl ValidatedPack {
    /// Pack root that was validated.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Fully validated manifest.
    #[must_use]
    pub const fn manifest(&self) -> &PackManifest {
        &self.manifest
    }

    /// Immutable identity suitable for rulesets, games, and replays.
    #[must_use]
    pub const fn identity(&self) -> &PackIdentity {
        &self.identity
    }
}

/// Parses and validates a complete unpacked lexicon pack directory.
///
/// Validation is strict: unknown TOML fields, unsafe paths, unlisted regular
/// files, symlinks, unsupported versions, missing files, size mismatches, and
/// SHA-256 mismatches all fail before the pack is returned.
///
/// # Errors
///
/// Returns an actionable [`PackError`] identifying the manifest field, path, or
/// digest that failed.
pub fn validate_pack(root: &Path) -> Result<ValidatedPack, PackError> {
    let manifest_path = root.join(MANIFEST_FILE);
    let manifest_toml = match fs::read_to_string(&manifest_path) {
        Ok(value) => value,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Err(PackError::MissingManifest {
                path: manifest_path,
                expected_name: MANIFEST_FILE,
            });
        }
        Err(source) => {
            return Err(PackError::Io {
                path: manifest_path,
                source,
            });
        }
    };

    let manifest: PackManifest =
        toml::from_str(&manifest_toml).map_err(|source| PackError::ManifestSyntax {
            path: manifest_path,
            source,
        })?;
    manifest.validate_schema()?;
    validate_complete_file_set(root, &manifest.files)?;

    let calculated = calculate_content_sha256(root, &manifest.files)?;
    if calculated != manifest.content_sha256 {
        return Err(PackError::ContentChecksumMismatch {
            expected: manifest.content_sha256,
            actual: calculated,
        });
    }

    let identity = manifest.identity();
    Ok(ValidatedPack {
        root: root.to_path_buf(),
        manifest,
        identity,
    })
}

/// Calculates the format-V1 deterministic checksum over listed payload bytes.
///
/// Descriptors are sorted by their canonical UTF-8 path bytes. The SHA-256
/// stream starts with `word-arena-pack-content-v1\0`; every file then contributes
/// its big-endian path length, path bytes, big-endian byte length, and exact
/// bytes. Manifest order, directory enumeration order, host path separators,
/// and line-ending conventions cannot alter the result.
///
/// Individual file sizes and checksums are verified during the same streaming
/// pass.
///
/// # Errors
///
/// Returns [`PackError`] for unsafe paths, missing/non-regular files, I/O
/// failures, and descriptor size or checksum mismatches.
pub fn calculate_content_sha256(
    root: &Path,
    files: &[FileDescriptor],
) -> Result<String, PackError> {
    let mut unique_paths = BTreeSet::new();
    for descriptor in files {
        validate_file_path(&descriptor.path)?;
        if !unique_paths.insert(descriptor.path.as_str()) {
            return Err(PackError::DuplicateFileRecord {
                path: descriptor.path.clone(),
            });
        }
    }

    let mut ordered = files.iter().collect::<Vec<_>>();
    ordered.sort_unstable_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));

    let mut content_hasher = Sha256::new();
    content_hasher.update(CONTENT_HASH_DOMAIN);

    for descriptor in ordered {
        let path = resolve_payload_path(root, &descriptor.path);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(value) => value,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Err(PackError::MissingPayloadFile {
                    relative_path: descriptor.path.clone(),
                    path,
                });
            }
            Err(source) => return Err(PackError::Io { path, source }),
        };
        if !metadata.file_type().is_file() {
            return Err(PackError::PayloadNotRegularFile {
                relative_path: descriptor.path.clone(),
                path,
            });
        }
        if metadata.len() != descriptor.size_bytes {
            return Err(PackError::FileSizeMismatch {
                path: descriptor.path.clone(),
                expected: descriptor.size_bytes,
                actual: metadata.len(),
            });
        }

        let path_bytes = descriptor.path.as_bytes();
        let path_length =
            u64::try_from(path_bytes.len()).map_err(|_| PackError::InvalidManifestField {
                field: "files.path",
                value: descriptor.path.clone(),
                reason: "path length cannot exceed the portable u64 framing limit",
            })?;
        content_hasher.update(path_length.to_be_bytes());
        content_hasher.update(path_bytes);
        content_hasher.update(descriptor.size_bytes.to_be_bytes());

        let file = File::open(&path).map_err(|source| PackError::Io {
            path: path.clone(),
            source,
        })?;
        let mut reader = BufReader::new(file);
        let mut file_hasher = Sha256::new();
        let mut observed_size = 0_u64;
        let mut buffer = vec![0_u8; READ_BUFFER_SIZE].into_boxed_slice();
        loop {
            let bytes_read = reader.read(&mut buffer).map_err(|source| PackError::Io {
                path: path.clone(),
                source,
            })?;
            if bytes_read == 0 {
                break;
            }
            let chunk = &buffer[..bytes_read];
            file_hasher.update(chunk);
            content_hasher.update(chunk);
            let chunk_size = u64::try_from(bytes_read).map_err(|_| PackError::Io {
                path: path.clone(),
                source: io::Error::other("read chunk length exceeds u64"),
            })?;
            observed_size = observed_size
                .checked_add(chunk_size)
                .ok_or_else(|| PackError::Io {
                    path: path.clone(),
                    source: io::Error::other("payload length exceeds u64"),
                })?;
        }

        if observed_size != descriptor.size_bytes {
            return Err(PackError::FileSizeMismatch {
                path: descriptor.path.clone(),
                expected: descriptor.size_bytes,
                actual: observed_size,
            });
        }

        let file_sha256 = digest_hex(file_hasher);
        if file_sha256 != descriptor.sha256 {
            return Err(PackError::FileChecksumMismatch {
                path: descriptor.path.clone(),
                expected: descriptor.sha256.clone(),
                actual: file_sha256,
            });
        }
    }

    Ok(digest_hex(content_hasher))
}

fn validate_complete_file_set(root: &Path, files: &[FileDescriptor]) -> Result<(), PackError> {
    let mut expected = files
        .iter()
        .map(|descriptor| descriptor.path.clone())
        .collect::<BTreeSet<_>>();
    expected.insert(MANIFEST_FILE.to_owned());

    let actual = collect_pack_files(root)?;
    for descriptor in files {
        if !actual.contains(&descriptor.path) {
            return Err(PackError::MissingPayloadFile {
                relative_path: descriptor.path.clone(),
                path: resolve_payload_path(root, &descriptor.path),
            });
        }
    }
    if let Some(path) = actual.difference(&expected).next() {
        return Err(PackError::UnexpectedPayloadFile { path: path.clone() });
    }
    Ok(())
}

fn collect_pack_files(root: &Path) -> Result<BTreeSet<String>, PackError> {
    let mut files = BTreeSet::new();
    collect_pack_files_from(root, root, &mut files)?;
    Ok(files)
}

fn collect_pack_files_from(
    root: &Path,
    directory: &Path,
    files: &mut BTreeSet<String>,
) -> Result<(), PackError> {
    let entries = fs::read_dir(directory).map_err(|source| PackError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| PackError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let relative = portable_relative_path(root, &path)?;
        let file_type = entry.file_type().map_err(|source| PackError::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_dir() {
            collect_pack_files_from(root, &path, files)?;
        } else if file_type.is_file() {
            files.insert(relative);
        } else {
            return Err(PackError::PayloadNotRegularFile {
                relative_path: relative,
                path,
            });
        }
    }
    Ok(())
}

fn portable_relative_path(root: &Path, path: &Path) -> Result<String, PackError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| PackError::UnsafeFilePath {
            path: path.display().to_string(),
            reason: "filesystem entry escaped the pack root",
        })?;
    let mut segments = Vec::new();
    for component in relative.components() {
        if let Component::Normal(segment) = component {
            let segment = segment.to_str().ok_or_else(|| PackError::NonUtf8Path {
                path: path.to_path_buf(),
            })?;
            segments.push(segment);
        } else {
            return Err(PackError::UnsafeFilePath {
                path: path.display().to_string(),
                reason: "filesystem path is not canonically relative",
            });
        }
    }
    Ok(segments.join("/"))
}

fn resolve_payload_path(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |path, component| path.join(component))
}

fn digest_hex(hasher: Sha256) -> String {
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}
