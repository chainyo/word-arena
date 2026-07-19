use std::{collections::HashMap, fmt, fs, path::PathBuf, time::Duration};

use reqwest::Url;
use serde::Deserialize;

use crate::{args::ConfigOverrides, error::CliError};

const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:3000";
const DEFAULT_TIMEOUT_MS: u64 = 15_000;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    server_url: Option<String>,
    game_id: Option<String>,
    token: Option<String>,
    timeout_ms: Option<u64>,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ResolvedConfig {
    server_url: Url,
    game_id: Option<String>,
    token: Option<String>,
    timeout: Duration,
}

impl fmt::Debug for ResolvedConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedConfig")
            .field("server_url", &self.server_url)
            .field("game_id", &self.game_id)
            .field("token", &self.token.as_ref().map(|_| "[REDACTED]"))
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl ResolvedConfig {
    /// Loads process environment and the selected local config using fixed precedence.
    ///
    /// # Errors
    ///
    /// Returns a configuration error for unsafe files, malformed values, or URLs.
    pub fn load(
        overrides: ConfigOverrides,
        explicit_config_path: Option<PathBuf>,
    ) -> Result<Self, CliError> {
        let environment = std::env::vars().collect::<HashMap<_, _>>();
        Self::load_from(overrides, explicit_config_path, &environment)
    }

    /// Resolves configuration against an injected environment for deterministic tests.
    ///
    /// # Errors
    ///
    /// Returns a configuration error for unsafe files, malformed values, or URLs.
    pub fn load_from(
        overrides: ConfigOverrides,
        explicit_config_path: Option<PathBuf>,
        environment: &HashMap<String, String>,
    ) -> Result<Self, CliError> {
        let environment_path = environment.get("WORD_ARENA_CONFIG").map(PathBuf::from);
        let (config_path, required) = if let Some(path) = explicit_config_path {
            (Some(path), true)
        } else if let Some(path) = environment_path {
            (Some(path), true)
        } else {
            (default_config_path(environment), false)
        };
        let file = match config_path {
            Some(path) if path.exists() => load_file(&path)?,
            Some(path) if required => {
                return Err(CliError::Config(format!(
                    "config file does not exist: {}",
                    path.display()
                )));
            }
            _ => FileConfig::default(),
        };

        let server_url = first(
            overrides.server_url,
            environment.get("WORD_ARENA_SERVER").cloned(),
            file.server_url,
        )
        .unwrap_or_else(|| DEFAULT_SERVER_URL.to_owned());
        let game_id = first(
            overrides.game_id,
            environment.get("WORD_ARENA_GAME_ID").cloned(),
            file.game_id,
        )
        .map(validate_game_id)
        .transpose()?;
        let token = first(
            overrides.token,
            environment.get("WORD_ARENA_TOKEN").cloned(),
            file.token,
        )
        .map(validate_token)
        .transpose()?;
        let environment_timeout = environment
            .get("WORD_ARENA_TIMEOUT_MS")
            .map(|value| {
                value.parse::<u64>().map_err(|_| {
                    CliError::Config("WORD_ARENA_TIMEOUT_MS must be an integer".to_owned())
                })
            })
            .transpose()?;
        let timeout_ms = overrides
            .timeout_ms
            .or(environment_timeout)
            .or(file.timeout_ms)
            .unwrap_or(DEFAULT_TIMEOUT_MS);
        if timeout_ms == 0 || timeout_ms > 300_000 {
            return Err(CliError::Config(
                "timeout_ms must be between 1 and 300000".to_owned(),
            ));
        }
        Ok(Self {
            server_url: validate_server_url(&server_url)?,
            game_id,
            token,
            timeout: Duration::from_millis(timeout_ms),
        })
    }

    #[must_use]
    pub const fn server_url(&self) -> &Url {
        &self.server_url
    }

    /// Returns the required game identity.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when no game was configured.
    pub fn game_id(&self) -> Result<&str, CliError> {
        self.game_id
            .as_deref()
            .ok_or_else(|| CliError::Config("game_id is required".to_owned()))
    }

    /// Returns the required bearer capability.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when no capability was configured.
    pub fn token(&self) -> Result<&str, CliError> {
        self.token
            .as_deref()
            .ok_or_else(|| CliError::Config("token is required".to_owned()))
    }

    #[must_use]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Resolves one relative API path against the validated server origin.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when URL resolution fails.
    pub fn endpoint(&self, path: &str) -> Result<Url, CliError> {
        self.server_url
            .join(path)
            .map_err(|_| CliError::Config("server URL cannot resolve API paths".to_owned()))
    }
}

fn first<T>(flag: Option<T>, environment: Option<T>, file: Option<T>) -> Option<T> {
    flag.or(environment).or(file)
}

fn validate_server_url(value: &str) -> Result<Url, CliError> {
    let mut url = Url::parse(value)
        .map_err(|_| CliError::Config("server_url must be a valid URL".to_owned()))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(CliError::Config(
            "server_url must be an HTTP(S) origin without credentials, query, or fragment"
                .to_owned(),
        ));
    }
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Ok(url)
}

fn validate_game_id(value: String) -> Result<String, CliError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !byte.is_ascii_whitespace())
    {
        Err(CliError::Config("game_id is invalid".to_owned()))
    } else {
        Ok(value)
    }
}

fn validate_token(value: String) -> Result<String, CliError> {
    if value.is_empty()
        || value.len() > 256
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !byte.is_ascii_whitespace())
    {
        Err(CliError::Config("token is invalid".to_owned()))
    } else {
        Ok(value)
    }
}

fn load_file(path: &PathBuf) -> Result<FileConfig, CliError> {
    ensure_private(path)?;
    let bytes = fs::read_to_string(path)
        .map_err(|error| CliError::Config(format!("failed to read {}: {error}", path.display())))?;
    toml::from_str(&bytes)
        .map_err(|_| CliError::Config(format!("invalid config {}", path.display())))
}

#[cfg(unix)]
fn ensure_private(path: &PathBuf) -> Result<(), CliError> {
    use std::os::unix::fs::MetadataExt;

    let mode = fs::metadata(path)
        .map_err(|error| CliError::Config(format!("failed to inspect config: {error}")))?
        .mode();
    if mode.trailing_zeros() >= 6 {
        Ok(())
    } else {
        Err(CliError::Config(format!(
            "config must not be accessible by group or others; run chmod 600 {}",
            path.display()
        )))
    }
}

#[cfg(not(unix))]
fn ensure_private(_path: &PathBuf) -> Result<(), CliError> {
    Ok(())
}

fn default_config_path(environment: &HashMap<String, String>) -> Option<PathBuf> {
    environment
        .get("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            environment
                .get("HOME")
                .map(|home| PathBuf::from(home).join(".config"))
        })
        .map(|base| base.join("word-arena/config.toml"))
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, fs};

    use tempfile::tempdir;

    use super::ResolvedConfig;
    use crate::args::ConfigOverrides;

    #[test]
    fn precedence_is_flags_then_environment_then_private_file_then_defaults() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            "server_url = \"http://file.example:3000\"\ngame_id = \"file-game\"\ntoken = \"file-token\"\ntimeout_ms = 1000\n",
        )
        .unwrap();
        private(&path);
        let environment = HashMap::from([
            (
                "WORD_ARENA_SERVER".to_owned(),
                "http://env.example:3000".to_owned(),
            ),
            ("WORD_ARENA_GAME_ID".to_owned(), "env-game".to_owned()),
            ("WORD_ARENA_TOKEN".to_owned(), "env-token".to_owned()),
            ("WORD_ARENA_TIMEOUT_MS".to_owned(), "2000".to_owned()),
        ]);
        let resolved = ResolvedConfig::load_from(
            ConfigOverrides {
                server_url: Some("http://flag.example:3000".to_owned()),
                game_id: Some("flag-game".to_owned()),
                token: Some("flag-token".to_owned()),
                timeout_ms: Some(3000),
            },
            Some(path),
            &environment,
        )
        .unwrap();
        assert_eq!(resolved.server_url().as_str(), "http://flag.example:3000/");
        assert_eq!(resolved.game_id().unwrap(), "flag-game");
        assert_eq!(resolved.token().unwrap(), "flag-token");
        assert_eq!(resolved.timeout().as_millis(), 3000);
        let debug = format!("{resolved:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("flag-token"));
    }

    #[test]
    fn insecure_or_unknown_config_and_url_credentials_fail_closed() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "unknown = true\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            let error = ResolvedConfig::load_from(
                ConfigOverrides::default(),
                Some(path.clone()),
                &HashMap::new(),
            )
            .unwrap_err();
            assert!(error.to_string().contains("chmod 600"));
        }
        private(&path);
        assert!(
            ResolvedConfig::load_from(ConfigOverrides::default(), Some(path), &HashMap::new())
                .is_err()
        );
        assert!(
            ResolvedConfig::load_from(
                ConfigOverrides {
                    server_url: Some("https://secret@example.com".to_owned()),
                    ..ConfigOverrides::default()
                },
                None,
                &HashMap::new()
            )
            .is_err()
        );
    }

    fn private(path: &std::path::Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
}
