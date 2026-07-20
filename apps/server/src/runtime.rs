use std::{
    fs::{self, OpenOptions},
    io::Write,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use thiserror::Error;
use word_arena_application::{
    ApplicationClock, ApplicationRuntime, CapabilityAdapters, CapabilityDigestKey, GameId,
    GameIdSource, GameRepository, LexiconResolver, SeedSource, SystemCapabilityTokenSource,
    UnixMillis,
};
use word_arena_engine::GameSeed;
use word_arena_lexicon::WordArenaPaths;
use word_arena_persistence::{
    MigrationError, SqliteCapabilityRepository, SqliteGameRepository, SqliteLocalMatchRepository,
    connect_and_migrate,
};

use crate::{AgentMatchManager, AgentMatchManagerConfig, RuntimeLexicons, ServerState};
use word_arena_agent_runtime::HarnessExecutables;

const DATABASE_FILE: &str = "word-arena.sqlite3";
const CAPABILITY_KEY_FILE: &str = "server-capability-hmac.key";

/// Production local-runtime initialization failure.
#[derive(Debug, Error)]
pub enum ProductionRuntimeError {
    /// Durable local directory or key file failed.
    #[error("local runtime I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// `SQLite` connection or migration failed.
    #[error(transparent)]
    Migration(#[from] MigrationError),
    /// Persisted local match index could not be restored safely.
    #[error("local match index initialization failed: {0}")]
    MatchIndex(&'static str),
}

/// Builds the persistent local application runtime from validated lexicons.
///
/// The `SQLite` database and an untracked 0600 capability HMAC key live beneath
/// the platform data directory. Existing keys are reused across restarts.
///
/// # Errors
///
/// Returns when the data directory, HMAC key, database, or migrations cannot be
/// initialized safely.
pub async fn build_production_state(
    paths: &WordArenaPaths,
    lexicons: Arc<RuntimeLexicons>,
) -> Result<Arc<ServerState>, ProductionRuntimeError> {
    fs::create_dir_all(paths.data_dir())?;
    let database_path = paths.data_dir().join(DATABASE_FILE);
    let pool = connect_and_migrate(&format!("sqlite://{}", database_path.display())).await?;
    let digest_key = CapabilityDigestKey::new(load_or_create_key(
        &paths.data_dir().join(CAPABILITY_KEY_FILE),
    )?);
    let game_repository: Arc<dyn GameRepository> =
        Arc::new(SqliteGameRepository::new(pool.clone()));
    let capability_repository = Arc::new(SqliteCapabilityRepository::new(pool.clone()));
    let lexicon_resolver: Arc<dyn LexiconResolver> = lexicons;
    let runtime = Arc::new(ApplicationRuntime::new(
        game_repository,
        lexicon_resolver,
        Arc::new(SystemGameIds),
        Arc::new(SystemSeeds),
        Arc::new(SystemClock),
        CapabilityAdapters::new(
            capability_repository,
            Arc::new(SystemCapabilityTokenSource),
            digest_key,
        ),
    ));
    let agents = AgentMatchManager::new(agent_manager_config(paths, pool));
    agents
        .restore()
        .await
        .map_err(ProductionRuntimeError::MatchIndex)?;
    Ok(Arc::new(ServerState::with_agent_manager(runtime, agents)))
}

fn agent_manager_config(paths: &WordArenaPaths, pool: sqlx::SqlitePool) -> AgentMatchManagerConfig {
    let executables = HarnessExecutables {
        codex: std::env::var("WORD_ARENA_CODEX_BIN").unwrap_or_else(|_| "codex".to_owned()),
        claude_code: std::env::var("WORD_ARENA_CLAUDE_BIN").unwrap_or_else(|_| "claude".to_owned()),
        cline: std::env::var("WORD_ARENA_CLINE_BIN").unwrap_or_else(|_| "cline".to_owned()),
        pi: std::env::var("WORD_ARENA_PI_BIN").unwrap_or_else(|_| "pi".to_owned()),
    };
    let codex_auth_file = std::env::var_os("WORD_ARENA_CODEX_AUTH_FILE")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .map(|home| home.join(".codex/auth.json"))
        })
        .filter(|path| path.is_file());
    AgentMatchManagerConfig {
        executables,
        workspace_root: paths.data_dir().join("agent-workspaces"),
        mcp_origin: std::env::var("WORD_ARENA_AGENT_SERVER_ORIGIN")
            .unwrap_or_else(|_| "http://127.0.0.1:3000".to_owned()),
        codex_auth_file,
        match_repository: Some(SqliteLocalMatchRepository::new(pool)),
    }
}

#[derive(Debug)]
struct SystemGameIds;

impl GameIdSource for SystemGameIds {
    fn next_game_id(&self) -> GameId {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes).expect("operating-system game ID entropy is unavailable");
        GameId::new(format!("game-{}", encode_hex(&bytes)))
            .expect("generated game ID satisfies the static contract")
    }
}

#[derive(Debug)]
struct SystemSeeds;

impl SeedSource for SystemSeeds {
    fn next_seed(&self) -> GameSeed {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).expect("operating-system game seed entropy is unavailable");
        GameSeed::from_bytes(bytes)
    }
}

#[derive(Debug)]
struct SystemClock;

impl ApplicationClock for SystemClock {
    fn now(&self) -> UnixMillis {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        UnixMillis(i64::try_from(millis).unwrap_or(i64::MAX))
    }
}

fn load_or_create_key(path: &std::path::Path) -> Result<[u8; 32], std::io::Error> {
    match open_new_secret(path) {
        Ok(mut file) => {
            let mut key = [0_u8; 32];
            getrandom::fill(&mut key).map_err(std::io::Error::other)?;
            file.write_all(&key)?;
            file.sync_all()?;
            Ok(key)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let bytes = fs::read(path)?;
            bytes.try_into().map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "capability HMAC key must contain exactly 32 bytes",
                )
            })
        }
        Err(error) => Err(error),
    }
}

fn open_new_secret(path: &std::path::Path) -> Result<std::fs::File, std::io::Error> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::load_or_create_key;

    #[test]
    fn local_capability_key_is_created_once_and_reused_exactly() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("capability.key");
        let first = load_or_create_key(&path).unwrap();
        let second = load_or_create_key(&path).unwrap();
        assert_eq!(first, second);
        assert_eq!(fs::read(&path).unwrap(), first);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn malformed_existing_capability_key_fails_closed() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("capability.key");
        fs::write(&path, [7_u8; 31]).unwrap();
        let error = load_or_create_key(&path).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }
}
