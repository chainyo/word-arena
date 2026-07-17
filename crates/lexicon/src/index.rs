use std::{
    fmt::Write as _,
    fs,
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
};

use fst::{Set, Streamer};
use sha2::{Digest, Sha256};

use crate::{
    INDEX_FILE, NormalizedKey, PackError, PackIdentity, PackManifest, ValidatedPack, normalize_key,
    validate_pack,
};

/// A completely verified immutable pack and its owned exact-membership index.
///
/// The FST bytes are read into owned memory only after complete pack integrity
/// validation. Membership checks borrow their query and allocate nothing. A
/// loaded instance never reopens the pack path, so installing or removing
/// other versions cannot hot-swap an in-use game's lexicon.
#[derive(Debug)]
pub struct LoadedLexicon {
    pack: ValidatedPack,
    index: Set<Vec<u8>>,
}

impl LoadedLexicon {
    /// Verifies and loads one unpacked pack before exposing membership queries.
    ///
    /// # Errors
    ///
    /// Returns [`PackError`] for any manifest, complete-file-set, checksum,
    /// FST-format, normalized-key, or word-count mismatch.
    pub fn open(root: &Path) -> Result<Self, PackError> {
        Self::from_validated(validate_pack(root)?)
    }

    /// Loads a pack that has already passed complete integrity validation.
    ///
    /// The index bytes are checked against their descriptor again after being
    /// read, closing the validation/read race for the bytes retained in memory.
    ///
    /// # Errors
    ///
    /// Returns [`PackError`] when the index changed after validation or its
    /// internal FST, keys, or count violates the manifest contract.
    pub fn from_validated(pack: ValidatedPack) -> Result<Self, PackError> {
        let descriptor = pack
            .manifest()
            .files
            .iter()
            .find(|descriptor| descriptor.path == INDEX_FILE)
            .ok_or(PackError::MissingRequiredFileRecord { path: INDEX_FILE })?;
        let path = pack.root().join(INDEX_FILE);
        let bytes = fs::read(&path).map_err(|source| PackError::Io {
            path: path.clone(),
            source,
        })?;
        let actual_size = u64::try_from(bytes.len()).map_err(|_| PackError::FileSizeMismatch {
            path: INDEX_FILE.to_owned(),
            expected: descriptor.size_bytes,
            actual: u64::MAX,
        })?;
        if actual_size != descriptor.size_bytes {
            return Err(PackError::FileSizeMismatch {
                path: INDEX_FILE.to_owned(),
                expected: descriptor.size_bytes,
                actual: actual_size,
            });
        }
        let actual_sha256 = sha256_hex(&bytes);
        if actual_sha256 != descriptor.sha256 {
            return Err(PackError::FileChecksumMismatch {
                path: INDEX_FILE.to_owned(),
                expected: descriptor.sha256.clone(),
                actual: actual_sha256,
            });
        }

        let index = parse_and_validate_index(
            bytes,
            &path,
            &pack.manifest().normalization.profile,
            pack.manifest().word_count,
        )?;
        Ok(Self { pack, index })
    }

    /// Tests one already-normalized key by exact bytes without allocating.
    #[must_use]
    pub fn contains(&self, key: &NormalizedKey) -> bool {
        self.index.contains(key.as_bytes())
    }

    /// Fully validated immutable manifest.
    #[must_use]
    pub const fn manifest(&self) -> &PackManifest {
        self.pack.manifest()
    }

    /// Complete pack identity to pin in a ruleset, game, or replay.
    #[must_use]
    pub const fn identity(&self) -> &PackIdentity {
        self.pack.identity()
    }

    /// Number of playable normalized keys.
    #[must_use]
    pub const fn word_count(&self) -> u64 {
        self.pack.manifest().word_count
    }
}

/// Verifies and loads one unpacked lexicon pack.
///
/// This is the canonical new-game loading boundary: no queryable lexicon is
/// returned until both the outer pack and internal FST have been checked.
///
/// # Errors
///
/// Returns [`PackError`] for any integrity or runtime-index contract failure.
pub fn load_lexicon(root: &Path) -> Result<LoadedLexicon, PackError> {
    LoadedLexicon::open(root)
}

fn parse_and_validate_index(
    bytes: Vec<u8>,
    path: &Path,
    profile: &str,
    expected_count: u64,
) -> Result<Set<Vec<u8>>, PackError> {
    let path = path.to_path_buf();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let index = Set::new(bytes).map_err(|error| PackError::InvalidIndex {
            path: path.clone(),
            reason: error.to_string(),
        })?;
        index
            .as_fst()
            .verify()
            .map_err(|error| PackError::InvalidIndex {
                path: path.clone(),
                reason: error.to_string(),
            })?;
        validate_all_keys(&index, &path, profile, expected_count)?;
        Ok(index)
    }));
    match outcome {
        Ok(result) => result,
        Err(_) => Err(PackError::InvalidIndex {
            path,
            reason: "FST parser panicked while validating untrusted index bytes".to_owned(),
        }),
    }
}

fn validate_all_keys(
    index: &Set<Vec<u8>>,
    path: &Path,
    profile: &str,
    expected_count: u64,
) -> Result<(), PackError> {
    let mut stream = index.stream();
    let mut actual_count = 0_u64;
    while let Some(bytes) = stream.next() {
        actual_count = actual_count
            .checked_add(1)
            .ok_or(PackError::IndexWordCountMismatch {
                expected: expected_count,
                actual: u64::MAX,
            })?;
        let key = std::str::from_utf8(bytes).map_err(|error| PackError::InvalidIndexKey {
            path: path.to_path_buf(),
            position: actual_count,
            reason: error.to_string(),
        })?;
        let normalized =
            normalize_key(profile, key).map_err(|error| PackError::InvalidIndexKey {
                path: path.to_path_buf(),
                position: actual_count,
                reason: error.to_string(),
            })?;
        if normalized.as_bytes() != bytes {
            return Err(PackError::InvalidIndexKey {
                path: path.to_path_buf(),
                position: actual_count,
                reason: "key is not the exact normalized board form".to_owned(),
            });
        }
    }
    if actual_count != expected_count {
        return Err(PackError::IndexWordCountMismatch {
            expected: expected_count,
            actual: actual_count,
        });
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}
