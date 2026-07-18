use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::DataPathError;

/// Resolved platform-local locations for installed packs and download cache.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WordArenaPaths {
    data: PathBuf,
    cache: PathBuf,
}

impl WordArenaPaths {
    /// Resolves the supported OS convention, honoring `WORD_ARENA_DATA_DIR`.
    ///
    /// An override keeps both durable data and cache beneath one explicit root,
    /// which makes local automation, containers, and tests self-contained.
    ///
    /// # Errors
    ///
    /// Returns [`DataPathError`] when the override is empty or the platform's
    /// required home/data variable is unavailable.
    pub fn discover() -> Result<Self, DataPathError> {
        if let Some(override_path) = env::var_os("WORD_ARENA_DATA_DIR") {
            if override_path.is_empty() {
                return Err(DataPathError::EmptyOverride);
            }
            return Ok(Self::from_base(PathBuf::from(override_path)));
        }
        platform_paths()
    }

    /// Creates deterministic paths beneath an explicit base directory.
    #[must_use]
    pub fn from_base(base: PathBuf) -> Self {
        Self {
            cache: base.join("cache"),
            data: base,
        }
    }

    /// Durable Word Arena data root.
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data
    }

    /// Download cache root.
    #[must_use]
    pub fn cache_dir(&self) -> &Path {
        &self.cache
    }

    /// Root containing immutable installed lexicon identities.
    #[must_use]
    pub fn lexicons_dir(&self) -> PathBuf {
        self.data.join("lexicons")
    }

    /// Cache location for one content-addressed artifact archive.
    #[must_use]
    pub fn artifact_cache_path(&self, archive_sha256: &str) -> PathBuf {
        self.cache
            .join("lexicons")
            .join(format!("{archive_sha256}.tar.gz"))
    }
}

#[cfg(target_os = "macos")]
fn platform_paths() -> Result<WordArenaPaths, DataPathError> {
    let home = required_var("data", "HOME")?;
    Ok(WordArenaPaths {
        data: PathBuf::from(&home)
            .join("Library/Application Support")
            .join("Word Arena"),
        cache: PathBuf::from(home)
            .join("Library/Caches")
            .join("Word Arena"),
    })
}

#[cfg(target_os = "windows")]
fn platform_paths() -> Result<WordArenaPaths, DataPathError> {
    let local = required_var("data", "LOCALAPPDATA")?;
    let data = PathBuf::from(local).join("Word Arena");
    Ok(WordArenaPaths {
        cache: data.join("cache"),
        data,
    })
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn platform_paths() -> Result<WordArenaPaths, DataPathError> {
    let home = required_var("data", "HOME")?;
    let data = env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&home).join(".local/share"))
        .join("word-arena");
    let cache = env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(home).join(".cache"))
        .join("word-arena");
    Ok(WordArenaPaths { data, cache })
}

fn required_var(kind: &'static str, variable: &'static str) -> Result<OsString, DataPathError> {
    env::var_os(variable)
        .filter(|value| !value.is_empty())
        .ok_or(DataPathError::MissingPlatformDirectory { kind, variable })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::WordArenaPaths;

    #[test]
    fn explicit_base_keeps_durable_data_and_cache_isolated() {
        let base = PathBuf::from("word-arena-test-data");
        let paths = WordArenaPaths::from_base(base.clone());

        assert_eq!(paths.data_dir(), base);
        assert_eq!(paths.cache_dir(), base.join("cache"));
        assert_eq!(paths.lexicons_dir(), base.join("lexicons"));
        assert_eq!(
            paths.artifact_cache_path(&"a".repeat(64)),
            base.join("cache/lexicons")
                .join(format!("{}.tar.gz", "a".repeat(64)))
        );
    }
}
