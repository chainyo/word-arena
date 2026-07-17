use std::{
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter},
    path::{Path, PathBuf},
};

use fst::SetBuilder;
use word_arena_lexicon::{
    ENGLISH_NORMALIZATION_PROFILE, FRENCH_NORMALIZATION_PROFILE, normalize_key,
};

use crate::{BuilderError, util::sha256_file};

/// Reproducibility metadata for one compiled exact-membership index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexBuildSummary {
    /// Atomically published FST path.
    pub output_path: PathBuf,
    /// Number of unique keys encoded in the set.
    pub word_count: u64,
    /// Exact compiled byte length.
    pub size_bytes: u64,
    /// SHA-256 over the compiled bytes.
    pub sha256: String,
}

/// Compiles sorted normalized keys into a deterministic, read-only FST set.
///
/// Input is streamed one line at a time and must already be unique, strictly
/// ordered by UTF-8 bytes, and normalized under `normalization_profile`. The
/// output is staged beside its destination and published without overwriting an
/// existing file, so interruption cannot expose a partial runtime index.
///
/// # Errors
///
/// Returns [`BuilderError`] for an unsupported profile, malformed or unordered
/// key, existing destination, FST encoding failure, or filesystem failure.
pub fn compile_index(
    keys_path: &Path,
    output_path: &Path,
    normalization_profile: &str,
) -> Result<IndexBuildSummary, BuilderError> {
    validate_profile(normalization_profile)?;
    if output_path.exists() {
        return Err(BuilderError::OutputExists {
            path: output_path.to_path_buf(),
        });
    }

    let parent = output_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| BuilderError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let mut staging = tempfile::Builder::new()
        .prefix(".word-arena-index-")
        .tempfile_in(parent)
        .map_err(|source| BuilderError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    let staging_path = staging.path().to_path_buf();
    let word_count = write_fst(
        keys_path,
        staging.as_file_mut(),
        &staging_path,
        normalization_profile,
    )?;
    staging
        .as_file()
        .sync_all()
        .map_err(|source| BuilderError::Io {
            path: staging_path,
            source,
        })?;

    match staging.persist_noclobber(output_path) {
        Ok(_) => {}
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(BuilderError::OutputExists {
                path: output_path.to_path_buf(),
            });
        }
        Err(error) => {
            return Err(BuilderError::Io {
                path: output_path.to_path_buf(),
                source: error.error,
            });
        }
    }

    let size_bytes = fs::metadata(output_path)
        .map_err(|source| BuilderError::Io {
            path: output_path.to_path_buf(),
            source,
        })?
        .len();
    Ok(IndexBuildSummary {
        output_path: output_path.to_path_buf(),
        word_count,
        size_bytes,
        sha256: sha256_file(output_path)?,
    })
}

fn write_fst(
    keys_path: &Path,
    output: &mut File,
    output_path: &Path,
    normalization_profile: &str,
) -> Result<u64, BuilderError> {
    let input = File::open(keys_path).map_err(|source| BuilderError::Io {
        path: keys_path.to_path_buf(),
        source,
    })?;
    let mut reader = BufReader::new(input);
    let writer = BufWriter::new(output);
    let mut builder = SetBuilder::new(writer).map_err(|source| BuilderError::IndexFst {
        path: output_path.to_path_buf(),
        source,
    })?;
    let mut line = String::new();
    let mut line_number = 0_u64;
    let mut previous: Option<String> = None;
    loop {
        line.clear();
        let bytes_read = reader
            .read_line(&mut line)
            .map_err(|source| BuilderError::Io {
                path: keys_path.to_path_buf(),
                source,
            })?;
        if bytes_read == 0 {
            break;
        }
        line_number = line_number
            .checked_add(1)
            .ok_or(BuilderError::IndexCountOverflow {
                field: "input line number",
            })?;
        if line.ends_with('\n') {
            line.pop();
        }
        let normalized = normalize_key(normalization_profile, &line).map_err(|_| {
            BuilderError::InvalidIndexKey {
                path: keys_path.to_path_buf(),
                line: line_number,
                value: line.clone(),
                reason: "key must be nonempty and representable by the selected normalization profile",
            }
        })?;
        if normalized.as_ref() != line {
            return Err(BuilderError::InvalidIndexKey {
                path: keys_path.to_path_buf(),
                line: line_number,
                value: line.clone(),
                reason: "store the exact normalized board key",
            });
        }
        if previous
            .as_ref()
            .is_some_and(|prior| prior.as_bytes() >= line.as_bytes())
        {
            return Err(BuilderError::InvalidIndexKey {
                path: keys_path.to_path_buf(),
                line: line_number,
                value: line.clone(),
                reason: "keys must be unique and strictly sorted by unsigned UTF-8 bytes",
            });
        }
        builder
            .insert(normalized.as_bytes())
            .map_err(|source| BuilderError::IndexFst {
                path: output_path.to_path_buf(),
                source,
            })?;
        previous = Some(normalized.into_string());
    }
    builder.finish().map_err(|source| BuilderError::IndexFst {
        path: output_path.to_path_buf(),
        source,
    })?;
    Ok(line_number)
}

fn validate_profile(profile: &str) -> Result<(), BuilderError> {
    if matches!(
        profile,
        ENGLISH_NORMALIZATION_PROFILE | FRENCH_NORMALIZATION_PROFILE
    ) {
        Ok(())
    } else {
        Err(BuilderError::UnsupportedIndexProfile {
            profile: profile.to_owned(),
        })
    }
}
