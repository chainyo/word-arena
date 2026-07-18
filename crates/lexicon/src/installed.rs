use std::{fs, path::PathBuf};

use crate::{InstalledPackError, LoadedLexicon, PackIdentity, WordArenaPaths, load_lexicon};

/// Loads one exact installed identity from its deterministic immutable path.
///
/// Multiple other versions may exist side by side; none is considered. This is
/// the resume/replay boundary for consumers that already hold a complete pin.
///
/// # Errors
///
/// Returns [`InstalledPackError`] when the exact path is absent, fails complete
/// pack/FST validation, or its verified manifest differs from `expected`.
pub fn load_installed_lexicon_exact(
    paths: &WordArenaPaths,
    expected: &PackIdentity,
) -> Result<LoadedLexicon, InstalledPackError> {
    let pack_root = paths
        .lexicons_dir()
        .join(&expected.pack_id)
        .join(&expected.pack_version)
        .join(&expected.content_sha256);
    if !pack_root.is_dir() {
        return Err(InstalledPackError::NotInstalled {
            pack_id: expected.pack_id.clone(),
            path: pack_root,
        });
    }
    let loaded = load_lexicon(&pack_root).map_err(|source| InstalledPackError::InvalidPack {
        path: pack_root,
        source: Box::new(source),
    })?;
    if loaded.identity() != expected {
        return Err(InstalledPackError::ExactIdentityMismatch {
            expected: Box::new(expected.clone()),
            actual: Box::new(loaded.identity().clone()),
        });
    }
    Ok(loaded)
}

/// Discovers and fully loads the sole locally installed identity for a pack ID.
///
/// This convenience boundary is intended for startup validation before exact
/// identities are bound to new games. It refuses ambiguity instead of choosing
/// a version or checksum implicitly, and it never performs network access.
///
/// # Errors
///
/// Returns [`InstalledPackError`] when the pack is absent, the installation
/// layout is malformed or ambiguous, or complete pack/FST validation fails.
pub fn load_installed_lexicon(
    paths: &WordArenaPaths,
    pack_id: &str,
) -> Result<LoadedLexicon, InstalledPackError> {
    let family_root = paths.lexicons_dir().join(pack_id);
    if !family_root.is_dir() {
        return Err(InstalledPackError::NotInstalled {
            pack_id: pack_id.to_owned(),
            path: family_root,
        });
    }
    let candidates = discover_candidates(&family_root)?;
    let [pack_root] = candidates.as_slice() else {
        return if candidates.is_empty() {
            Err(InstalledPackError::NotInstalled {
                pack_id: pack_id.to_owned(),
                path: family_root,
            })
        } else {
            Err(InstalledPackError::Ambiguous {
                pack_id: pack_id.to_owned(),
                path: family_root,
            })
        };
    };
    let loaded = load_lexicon(pack_root).map_err(|source| InstalledPackError::InvalidPack {
        path: pack_root.clone(),
        source: Box::new(source),
    })?;
    let version = pack_root
        .parent()
        .and_then(|path| path.file_name())
        .and_then(|value| value.to_str());
    let content_sha256 = pack_root.file_name().and_then(|value| value.to_str());
    let identity = loaded.identity();
    if identity.pack_id != pack_id
        || version != Some(identity.pack_version.as_str())
        || content_sha256 != Some(identity.content_sha256.as_str())
    {
        return Err(InstalledPackError::IdentityPathMismatch {
            path: pack_root.clone(),
            identity: Box::new(identity.clone()),
        });
    }
    Ok(loaded)
}

fn discover_candidates(family_root: &std::path::Path) -> Result<Vec<PathBuf>, InstalledPackError> {
    let mut candidates = Vec::new();
    for version in directory_entries(family_root)? {
        for content in directory_entries(&version)? {
            candidates.push(content);
        }
    }
    candidates.sort_unstable();
    Ok(candidates)
}

fn directory_entries(root: &std::path::Path) -> Result<Vec<PathBuf>, InstalledPackError> {
    let entries = fs::read_dir(root).map_err(|source| InstalledPackError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| InstalledPackError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|source| InstalledPackError::Io {
            path: path.clone(),
            source,
        })?;
        if !kind.is_dir() || entry.file_name().to_str().is_none() {
            return Err(InstalledPackError::InvalidLayout {
                path,
                reason: "version and checksum entries must be portable directories",
            });
        }
        paths.push(path);
    }
    paths.sort_unstable();
    Ok(paths)
}
