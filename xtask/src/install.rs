use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fmt::Write as _,
    fs::{self, File},
    io::{BufReader, Read},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use word_arena_lexicon::{WordArenaPaths, load_lexicon};

use crate::{PackRecord, PackRegistry, XtaskError};

const COMPLIANCE_FILES: [&str; 3] = ["LICENSE", "SOURCE.md", "THIRD_PARTY_NOTICES"];
const HASH_BUFFER_SIZE: usize = 64 * 1024;

/// Result of installing one immutable registry record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallStatus {
    /// A verified artifact was atomically installed.
    Installed,
    /// The exact verified identity was already installed.
    AlreadyInstalled,
}

/// Registry and local path state for lexicon lifecycle operations.
#[derive(Clone, Debug)]
pub struct PackInstaller {
    registry: PackRegistry,
    paths: WordArenaPaths,
}

impl PackInstaller {
    /// Creates an installer from already validated configuration.
    #[must_use]
    pub const fn new(registry: PackRegistry, paths: WordArenaPaths) -> Self {
        Self { registry, paths }
    }

    /// Configured immutable registry.
    #[must_use]
    pub const fn registry(&self) -> &PackRegistry {
        &self.registry
    }

    /// Resolved local paths.
    #[must_use]
    pub const fn paths(&self) -> &WordArenaPaths {
        &self.paths
    }

    /// Installs or verifies one pinned registry pack.
    ///
    /// Downloads are content-addressed and verified before extraction. Archive
    /// extraction and complete pack/FST validation happen in a unique staging
    /// directory; only a final directory rename publishes the identity.
    ///
    /// # Errors
    ///
    /// Returns [`XtaskError`] without modifying an existing installation when
    /// download, checksum, archive, compliance, identity, or pack validation
    /// fails.
    pub fn install(&self, pack_id: &str, offline: bool) -> Result<InstallStatus, XtaskError> {
        let record = self.registry.require(pack_id)?.clone();
        Self::commit(self.prepare(record, offline)?)
    }

    /// Preflights every requested archive and pack before publishing any new
    /// identity, then atomically commits each verified pack directory.
    ///
    /// # Errors
    ///
    /// Returns [`XtaskError`] before publication when any download, checksum,
    /// archive, compliance, identity, or runtime-index check fails.
    pub fn install_many(
        &self,
        pack_ids: &[&str],
        offline: bool,
    ) -> Result<Vec<(String, InstallStatus)>, XtaskError> {
        let mut prepared = Vec::with_capacity(pack_ids.len());
        for pack_id in pack_ids {
            let record = self.registry.require(pack_id)?.clone();
            prepared.push(self.prepare(record, offline)?);
        }
        prepared
            .into_iter()
            .map(|prepared| {
                let pack_id = prepared.record().pack_id.clone();
                Self::commit(prepared).map(|status| (pack_id, status))
            })
            .collect()
    }

    /// Verifies one installed registry pack through the runtime loading path.
    ///
    /// # Errors
    ///
    /// Returns [`XtaskError::NotInstalled`] when absent or another validation
    /// error when bytes no longer match the registry.
    pub fn verify(&self, pack_id: &str) -> Result<(), XtaskError> {
        let record = self.registry.require(pack_id)?;
        let path = self.pack_path(record);
        if !path.exists() {
            return Err(XtaskError::NotInstalled {
                pack_id: pack_id.to_owned(),
                path,
            });
        }
        Self::verify_at(record, &path)
    }

    /// Loads one exact installed pack after compliance and identity checks.
    ///
    /// # Errors
    ///
    /// Returns [`XtaskError::NotInstalled`] when absent or another validation
    /// error when bytes, identity, license, or required notices are invalid.
    pub fn load_installed(
        &self,
        pack_id: &str,
    ) -> Result<word_arena_lexicon::LoadedLexicon, XtaskError> {
        let record = self.registry.require(pack_id)?;
        let path = self.pack_path(record);
        if !path.exists() {
            return Err(XtaskError::NotInstalled {
                pack_id: pack_id.to_owned(),
                path,
            });
        }
        verify_compliance_files(record, &path)?;
        let loaded = load_lexicon(&path)?;
        Self::verify_loaded_identity(record, &loaded)?;
        Ok(loaded)
    }

    /// Moves one exact installed identity to a recoverable local trash path.
    ///
    /// # Errors
    ///
    /// Returns [`XtaskError`] when the record is unknown, absent, invalid, or
    /// cannot be atomically moved.
    pub fn remove(&self, pack_id: &str) -> Result<PathBuf, XtaskError> {
        let record = self.registry.require(pack_id)?;
        let installed = self.pack_path(record);
        if !installed.exists() {
            return Err(XtaskError::NotInstalled {
                pack_id: pack_id.to_owned(),
                path: installed,
            });
        }
        Self::verify_at(record, &installed)?;
        let trash = self.paths.data_dir().join("trash/lexicons");
        fs::create_dir_all(&trash).map_err(|source| XtaskError::Io {
            path: trash.clone(),
            source,
        })?;
        for suffix in 0_u32..=u32::MAX {
            let destination = trash.join(format!(
                "{}-{}-{}-{suffix}",
                record.pack_id, record.pack_version, record.content_sha256
            ));
            if destination.exists() {
                continue;
            }
            fs::rename(&installed, &destination).map_err(|source| XtaskError::Io {
                path: installed,
                source,
            })?;
            return Ok(destination);
        }
        unreachable!("u32 trash suffix space cannot be exhausted in practice")
    }

    /// Deterministic installation path for one immutable record.
    #[must_use]
    pub fn pack_path(&self, record: &PackRecord) -> PathBuf {
        self.paths
            .lexicons_dir()
            .join(&record.pack_id)
            .join(&record.pack_version)
            .join(&record.content_sha256)
    }

    fn verify_at(record: &PackRecord, path: &Path) -> Result<(), XtaskError> {
        verify_compliance_files(record, path)?;
        let loaded = load_lexicon(path)?;
        Self::verify_loaded_identity(record, &loaded)
    }

    fn verify_loaded_identity(
        record: &PackRecord,
        loaded: &word_arena_lexicon::LoadedLexicon,
    ) -> Result<(), XtaskError> {
        let expected = record.identity();
        if loaded.identity() != &expected {
            return Err(XtaskError::ArtifactIdentityMismatch {
                expected: Box::new(expected),
                actual: Box::new(loaded.identity().clone()),
            });
        }
        if loaded.manifest().source.license_id != record.license_id {
            return Err(XtaskError::LicenseMismatch {
                pack_id: record.pack_id.clone(),
                expected: record.license_id.clone(),
                actual: loaded.manifest().source.license_id.clone(),
            });
        }
        Ok(())
    }

    fn prepare(&self, record: PackRecord, offline: bool) -> Result<PreparedInstall, XtaskError> {
        let final_path = self.pack_path(&record);
        if final_path.exists() {
            Self::verify_at(&record, &final_path)?;
            return Ok(PreparedInstall::Already { record });
        }
        let archive = self.obtain_archive(&record, offline)?;
        let staging_parent = self.paths.lexicons_dir().join(".staging");
        fs::create_dir_all(&staging_parent).map_err(|source| XtaskError::Io {
            path: staging_parent.clone(),
            source,
        })?;
        let staging = tempfile::Builder::new()
            .prefix("pack-")
            .tempdir_in(&staging_parent)
            .map_err(|source| XtaskError::Io {
                path: staging_parent,
                source,
            })?;
        extract_archive(&record, &archive, staging.path())?;
        Self::verify_at(&record, staging.path())?;
        Ok(PreparedInstall::Ready {
            record,
            staging,
            final_path,
        })
    }

    fn commit(prepared: PreparedInstall) -> Result<InstallStatus, XtaskError> {
        let PreparedInstall::Ready {
            record,
            staging,
            final_path,
        } = prepared
        else {
            return Ok(InstallStatus::AlreadyInstalled);
        };
        let parent = final_path
            .parent()
            .expect("validated pack path has a parent");
        fs::create_dir_all(parent).map_err(|source| XtaskError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        match fs::rename(staging.path(), &final_path) {
            Ok(()) => Ok(InstallStatus::Installed),
            Err(_) if final_path.exists() => {
                Self::verify_at(&record, &final_path)?;
                Ok(InstallStatus::AlreadyInstalled)
            }
            Err(source) => Err(XtaskError::Io {
                path: final_path,
                source,
            }),
        }
    }

    fn obtain_archive(&self, record: &PackRecord, offline: bool) -> Result<PathBuf, XtaskError> {
        let cache_path = self.paths.artifact_cache_path(&record.artifact_sha256);
        if cache_path.exists() {
            match verify_archive(record, &cache_path) {
                Ok(()) => return Ok(cache_path),
                Err(error) if offline => return Err(error),
                Err(_) => {}
            }
        } else if offline {
            return Err(XtaskError::OfflineArtifactUnavailable {
                pack_id: record.pack_id.clone(),
                cache_path,
            });
        }

        let parent = cache_path.expect_parent();
        fs::create_dir_all(parent).map_err(|source| XtaskError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        let staging = tempfile::Builder::new()
            .prefix(".download-")
            .tempfile_in(parent)
            .map_err(|source| XtaskError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        download_with_curl(&record.artifact_url, staging.path())?;
        verify_archive(record, staging.path())?;
        staging
            .persist(&cache_path)
            .map_err(|error| XtaskError::Io {
                path: cache_path.clone(),
                source: error.error,
            })?;
        Ok(cache_path)
    }
}

#[derive(Debug)]
enum PreparedInstall {
    Already {
        record: PackRecord,
    },
    Ready {
        record: PackRecord,
        staging: tempfile::TempDir,
        final_path: PathBuf,
    },
}

impl PreparedInstall {
    const fn record(&self) -> &PackRecord {
        match self {
            Self::Already { record } | Self::Ready { record, .. } => record,
        }
    }
}

/// Requires one executable to start successfully with `--version`.
///
/// # Errors
///
/// Returns [`XtaskError::MissingTool`] or [`XtaskError::ToolFailed`].
pub fn verify_tool(tool: &'static str, recovery: &'static str) -> Result<(), XtaskError> {
    let output = Command::new(tool)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                XtaskError::MissingTool { tool, recovery }
            } else {
                XtaskError::Io {
                    path: PathBuf::from(tool),
                    source,
                }
            }
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(XtaskError::ToolFailed {
            command: format!("{tool} --version"),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}

pub(crate) fn download_with_curl(url: &str, destination: &Path) -> Result<(), XtaskError> {
    let output = Command::new("curl")
        .args([
            OsStr::new("--fail"),
            OsStr::new("--location"),
            OsStr::new("--silent"),
            OsStr::new("--show-error"),
            OsStr::new("--proto"),
            OsStr::new("=http,https"),
            OsStr::new("--proto-redir"),
            OsStr::new("=http,https"),
            OsStr::new("--output"),
            destination.as_os_str(),
            OsStr::new(url),
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                XtaskError::MissingTool {
                    tool: "curl",
                    recovery: "install curl or use --offline with a previously verified cache",
                }
            } else {
                XtaskError::Io {
                    path: PathBuf::from("curl"),
                    source,
                }
            }
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(XtaskError::DownloadFailed {
            url: url.to_owned(),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}

fn verify_archive(record: &PackRecord, path: &Path) -> Result<(), XtaskError> {
    let actual_size = fs::metadata(path)
        .map_err(|source| XtaskError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    if actual_size != record.artifact_size_bytes {
        return Err(XtaskError::ArtifactSizeMismatch {
            pack_id: record.pack_id.clone(),
            expected: record.artifact_size_bytes,
            actual: actual_size,
        });
    }
    let actual = sha256_file(path)?;
    if actual != record.artifact_sha256 {
        return Err(XtaskError::ArtifactChecksumMismatch {
            pack_id: record.pack_id.clone(),
            expected: record.artifact_sha256.clone(),
            actual,
        });
    }
    Ok(())
}

fn extract_archive(
    record: &PackRecord,
    archive_path: &Path,
    destination: &Path,
) -> Result<(), XtaskError> {
    let file = File::open(archive_path).map_err(|source| XtaskError::Io {
        path: archive_path.to_path_buf(),
        source,
    })?;
    let decoder = GzDecoder::new(BufReader::new(file));
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| invalid_archive(record, archive_path, &error))?;
    let mut files = BTreeSet::new();
    for entry in entries {
        let mut entry = entry.map_err(|error| invalid_archive(record, archive_path, &error))?;
        let relative = entry
            .path()
            .map_err(|error| invalid_archive(record, archive_path, &error))?
            .into_owned();
        validate_archive_path(record, archive_path, &relative)?;
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            fs::create_dir_all(destination.join(&relative)).map_err(|source| XtaskError::Io {
                path: destination.join(&relative),
                source,
            })?;
            continue;
        }
        if !entry_type.is_file() {
            return Err(XtaskError::InvalidArchive {
                pack_id: record.pack_id.clone(),
                path: archive_path.to_path_buf(),
                reason: format!(
                    "entry {} is not a regular file or directory",
                    relative.display()
                ),
            });
        }
        if !files.insert(relative.clone()) {
            return Err(XtaskError::InvalidArchive {
                pack_id: record.pack_id.clone(),
                path: archive_path.to_path_buf(),
                reason: format!("duplicate file entry {}", relative.display()),
            });
        }
        let target = destination.join(&relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|source| XtaskError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        entry.unpack(&target).map_err(|source| XtaskError::Io {
            path: target,
            source,
        })?;
    }
    Ok(())
}

fn validate_archive_path(
    record: &PackRecord,
    archive_path: &Path,
    relative: &Path,
) -> Result<(), XtaskError> {
    let valid = !relative.as_os_str().is_empty()
        && relative.components().all(
            |component| matches!(component, Component::Normal(value) if value.to_str().is_some()),
        );
    if valid {
        Ok(())
    } else {
        Err(XtaskError::InvalidArchive {
            pack_id: record.pack_id.clone(),
            path: archive_path.to_path_buf(),
            reason: format!(
                "entry path {} is not safe portable UTF-8",
                relative.display()
            ),
        })
    }
}

fn verify_compliance_files(record: &PackRecord, root: &Path) -> Result<(), XtaskError> {
    for relative in COMPLIANCE_FILES {
        let path = root.join(relative);
        let metadata = fs::symlink_metadata(&path).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                XtaskError::InvalidComplianceFile {
                    pack_id: record.pack_id.clone(),
                    path: relative,
                    reason: "file is missing",
                }
            } else {
                XtaskError::Io { path, source }
            }
        })?;
        if !metadata.file_type().is_file() || metadata.len() == 0 {
            return Err(XtaskError::InvalidComplianceFile {
                pack_id: record.pack_id.clone(),
                path: relative,
                reason: "file must be a nonempty regular file",
            });
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, XtaskError> {
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

fn invalid_archive(record: &PackRecord, path: &Path, error: &std::io::Error) -> XtaskError {
    XtaskError::InvalidArchive {
        pack_id: record.pack_id.clone(),
        path: path.to_path_buf(),
        reason: error.to_string(),
    }
}

trait PathExt {
    fn expect_parent(&self) -> &Path;
}

impl PathExt for PathBuf {
    fn expect_parent(&self) -> &Path {
        self.parent()
            .expect("content-addressed cache path always has a parent")
    }
}
