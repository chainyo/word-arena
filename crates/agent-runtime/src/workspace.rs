use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    AuthorityBoundaryAuditSink, AuthorityBoundarySurface, DriverClock, DriverFuture, ExitStatus,
    ForbiddenAuthorityKind, ForbiddenAuthorityPolicy, HarnessConfig, HarnessExecutables,
    HarnessRuntimeConfig, NetworkPolicy, ProcessAdapter, ProcessError, ProcessEvent, ProcessHandle,
    ProcessInstance, ProcessSpec, ValidatedAgentManifest, WorkspaceRetention,
    authority::{audit_denial, forbidden_authority_marker},
    driver::spawn_tokio_process,
};

pub const SEAT_WORKSPACE_SCHEMA_VERSION: u32 = 1;
pub const MAX_SEAT_CAPABILITY_TTL_MS: i64 = 3_600_000;
pub const SEAT_CAPABILITY_ENV: &str = "WORD_ARENA_SEAT_CAPABILITY";

const DIRECTORY_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;
const MAX_ID_BYTES: usize = 96;
const REDACTION_STREAM_CHUNK_BYTES: usize = 16 * 1024;

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("workspace root or managed path is invalid")]
    InvalidPath,
    #[error("workspace identifier is invalid")]
    InvalidIdentifier,
    #[error("seat capability is malformed, expired, or not short-lived")]
    InvalidCapability,
    #[error("MCP endpoint is invalid or contains credentials")]
    InvalidMcpEndpoint,
    #[error("workspace already exists")]
    AlreadyExists,
    #[error("workspace does not exist")]
    Missing,
    #[error("workspace metadata, ownership, permissions, or configuration is corrupt")]
    Corrupt,
    #[error("filesystem operation failed")]
    Filesystem,
    #[error("no supported fail-closed process sandbox is available")]
    SandboxUnavailable,
    #[error("human-spectator or administrator authority reached an agent boundary")]
    ForbiddenAuthority,
    #[error("forbidden-authority audit could not be recorded")]
    AuditUnavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SeatSandboxBackend {
    MacOsSandboxExec {
        executable: PathBuf,
        runtime_read_roots: Vec<PathBuf>,
    },
    Bubblewrap {
        executable: PathBuf,
        runtime_read_roots: Vec<PathBuf>,
    },
}

impl SeatSandboxBackend {
    /// Selects an explicit local sandbox executable and reviewed runtime roots.
    ///
    /// # Errors
    ///
    /// Returns an error instead of silently launching an unsandboxed agent when
    /// the current platform has no supported sandbox binary.
    pub fn detect() -> Result<Self, WorkspaceError> {
        #[cfg(target_os = "macos")]
        {
            let executable = PathBuf::from("/usr/bin/sandbox-exec");
            if executable.is_file() {
                return Ok(Self::MacOsSandboxExec {
                    executable,
                    runtime_read_roots: [
                        "/bin",
                        "/usr",
                        "/System",
                        "/Library",
                        "/opt/homebrew",
                        "/private/var/select",
                        "/dev",
                    ]
                    .into_iter()
                    .map(PathBuf::from)
                    .filter(|path| path.exists())
                    .collect(),
                });
            }
        }
        #[cfg(target_os = "linux")]
        {
            for executable in ["/usr/bin/bwrap", "/bin/bwrap"] {
                let executable = PathBuf::from(executable);
                if executable.is_file() {
                    return Ok(Self::Bubblewrap {
                        executable,
                        runtime_read_roots: [
                            "/bin",
                            "/usr",
                            "/lib",
                            "/lib64",
                            "/etc/ssl/certs",
                            "/etc/resolv.conf",
                        ]
                        .into_iter()
                        .map(PathBuf::from)
                        .filter(|path| path.exists())
                        .collect(),
                    });
                }
            }
        }
        Err(WorkspaceError::SandboxUnavailable)
    }

    fn validate(&self) -> Result<(), WorkspaceError> {
        let (executable, roots) = match self {
            Self::MacOsSandboxExec {
                executable,
                runtime_read_roots,
            }
            | Self::Bubblewrap {
                executable,
                runtime_read_roots,
            } => (executable, runtime_read_roots),
        };
        if !executable.is_absolute()
            || !executable.is_file()
            || roots.is_empty()
            || roots
                .iter()
                .any(|root| !root.is_absolute() || !root.exists())
        {
            return Err(WorkspaceError::SandboxUnavailable);
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct WorkspaceManagerConfig {
    pub root: PathBuf,
    pub safe_path: String,
    pub sandbox: SeatSandboxBackend,
    pub authority: AuthorityBoundaryConfig,
}

impl WorkspaceManagerConfig {
    /// Creates the default fail-closed platform configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when no supported OS sandbox is installed.
    pub fn detect(
        root: PathBuf,
        authority: AuthorityBoundaryConfig,
    ) -> Result<Self, WorkspaceError> {
        Ok(Self {
            root,
            safe_path: "/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin".to_owned(),
            sandbox: SeatSandboxBackend::detect()?,
            authority,
        })
    }
}

/// Digest-only forbidden-authority policy plus its mandatory denial sink.
#[derive(Clone)]
pub struct AuthorityBoundaryConfig {
    policy: Arc<ForbiddenAuthorityPolicy>,
    audit: Arc<dyn AuthorityBoundaryAuditSink>,
}

impl fmt::Debug for AuthorityBoundaryConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthorityBoundaryConfig")
            .field("policy", &self.policy)
            .field("audit", &"<authority-audit-sink>")
            .finish()
    }
}

impl AuthorityBoundaryConfig {
    #[must_use]
    pub fn new(
        policy: Arc<ForbiddenAuthorityPolicy>,
        audit: Arc<dyn AuthorityBoundaryAuditSink>,
    ) -> Self {
        Self { policy, audit }
    }
}

pub struct SeatCapability {
    raw: String,
    expires_at_unix_ms: i64,
}

impl fmt::Debug for SeatCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SeatCapability")
            .field("raw", &"<redacted>")
            .field("expires_at_unix_ms", &self.expires_at_unix_ms)
            .finish()
    }
}

impl SeatCapability {
    /// Wraps one raw seat bearer without making it cloneable or serializable.
    ///
    /// # Errors
    ///
    /// Returns an error unless the token has the exact V1 wire shape and an
    /// expiry strictly within the configured one-hour V1 maximum.
    pub fn new(
        raw: String,
        expires_at_unix_ms: i64,
        now_unix_ms: i64,
    ) -> Result<Self, WorkspaceError> {
        let parts = raw.split('.').collect::<Vec<_>>();
        let valid_wire = parts.len() == 3
            && parts[0] == "wa_cap_v1"
            && is_lower_hex(parts[1], 32)
            && is_lower_hex(parts[2], 64);
        let ttl = expires_at_unix_ms.saturating_sub(now_unix_ms);
        if !valid_wire || ttl <= 0 || ttl > MAX_SEAT_CAPABILITY_TTL_MS {
            return Err(WorkspaceError::InvalidCapability);
        }
        Ok(Self {
            raw,
            expires_at_unix_ms,
        })
    }
}

impl Drop for SeatCapability {
    fn drop(&mut self) {
        // Stable Rust does not expose String's initialized bytes for safe
        // overwriting. Clearing prevents accidental reuse; short server-side
        // expiry and process isolation remain the security boundaries.
        self.raw.clear();
    }
}

pub struct SeatWorkspaceRequest {
    pub run_id: String,
    pub seat_id: String,
    pub game_id: String,
    pub mcp_url: String,
    pub capability: SeatCapability,
}

impl fmt::Debug for SeatWorkspaceRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SeatWorkspaceRequest")
            .field("run_id", &self.run_id)
            .field("seat_id", &self.seat_id)
            .field("game_id", &self.game_id)
            .field("mcp_url", &self.mcp_url)
            .field("capability", &self.capability)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceOutcome {
    Completed,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceDisposition {
    Deleted,
    Retained(PathBuf),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum WorkspaceHarnessKind {
    Codex,
    ClaudeCode,
    Cline,
    Pi,
    GenericCommand,
}

impl From<&HarnessConfig> for WorkspaceHarnessKind {
    fn from(value: &HarnessConfig) -> Self {
        match value {
            HarnessConfig::Codex { .. } => Self::Codex,
            HarnessConfig::ClaudeCode { .. } => Self::ClaudeCode,
            HarnessConfig::Cline { .. } => Self::Cline,
            HarnessConfig::Pi { .. } => Self::Pi,
            HarnessConfig::GenericCommand { .. } => Self::GenericCommand,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceMarker {
    schema_version: u32,
    run_id: String,
    seat_id: String,
    game_id: String,
    harness: WorkspaceHarnessKind,
    manifest_sha256: String,
    retention: WorkspaceRetention,
}

#[derive(Debug)]
pub struct SeatWorkspaceManager {
    root: PathBuf,
    safe_path: String,
    sandbox: SeatSandboxBackend,
    owner: u32,
    clock: Arc<dyn DriverClock>,
    authority: AuthorityBoundaryConfig,
}

impl SeatWorkspaceManager {
    /// Creates or validates one private manager root without following a root
    /// symlink.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe paths, permissions, owners, PATH values, or
    /// unavailable sandbox configuration.
    pub fn new(
        config: WorkspaceManagerConfig,
        clock: Arc<dyn DriverClock>,
    ) -> Result<Self, WorkspaceError> {
        config.sandbox.validate()?;
        validate_safe_path(&config.safe_path)?;
        let root = prepare_manager_root(&config.root)?;
        let metadata = fs::metadata(&root).map_err(|_| WorkspaceError::Filesystem)?;
        let owner = owner_id(&metadata);
        let runs = root.join("runs");
        ensure_private_directory(&runs, owner, true)?;
        Ok(Self {
            root,
            safe_path: config.safe_path,
            sandbox: config.sandbox,
            owner,
            clock,
            authority: config.authority,
        })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Allocates one new exclusive seat workspace and writes secret-free
    /// harness configuration.
    ///
    /// # Errors
    ///
    /// Returns an error on collision, unsafe identity, filesystem drift, or
    /// invalid capability/endpoint inputs.
    pub fn allocate(
        &self,
        manifest: &ValidatedAgentManifest,
        request: SeatWorkspaceRequest,
    ) -> Result<SeatWorkspaceLease, WorkspaceError> {
        self.provision(manifest, request, false)
    }

    /// Reopens an existing stable seat workspace with a newly supplied
    /// short-lived capability after verifying every marker and config file.
    ///
    /// # Errors
    ///
    /// Returns an error instead of adopting mismatched, symlinked, permissive,
    /// or tampered state.
    pub fn resume(
        &self,
        manifest: &ValidatedAgentManifest,
        request: SeatWorkspaceRequest,
    ) -> Result<SeatWorkspaceLease, WorkspaceError> {
        self.provision(manifest, request, true)
    }

    fn provision(
        &self,
        manifest: &ValidatedAgentManifest,
        request: SeatWorkspaceRequest,
        resume: bool,
    ) -> Result<SeatWorkspaceLease, WorkspaceError> {
        validate_identifier(&request.run_id)?;
        validate_identifier(&request.seat_id)?;
        validate_identifier(&request.game_id)?;
        validate_mcp_url(&request.mcp_url)?;
        if let Some(authority) = self
            .authority
            .policy
            .find(request.capability.raw.as_bytes())
        {
            self.record_authority_denial(
                &request.run_id,
                &request.seat_id,
                authority,
                AuthorityBoundarySurface::ProcessEnvironment,
            )?;
            return Err(WorkspaceError::ForbiddenAuthority);
        }
        if request.capability.expires_at_unix_ms <= self.clock.now_unix_ms() {
            return Err(WorkspaceError::InvalidCapability);
        }
        let run_root = self.root.join("runs").join(&request.run_id);
        ensure_private_directory(&run_root, self.owner, !resume)?;
        let seat_root = run_root.join(&request.seat_id);
        if resume {
            validate_private_directory(&seat_root, self.owner)?;
        } else {
            create_private_directory(&seat_root)?;
        }
        let seat_root = fs::canonicalize(&seat_root).map_err(|_| WorkspaceError::Filesystem)?;
        if !seat_root.starts_with(&run_root) || seat_root == run_root {
            return Err(WorkspaceError::InvalidPath);
        }
        let workspace = seat_root.join("workspace");
        let state = seat_root.join("state");
        let home = seat_root.join("home");
        let temporary = seat_root.join("tmp");
        let config = seat_root.join("config");
        for directory in [&workspace, &state, &home, &temporary, &config] {
            ensure_private_directory(directory, self.owner, !resume)?;
        }

        let harness = WorkspaceHarnessKind::from(&manifest.manifest().harness);
        let marker = WorkspaceMarker {
            schema_version: SEAT_WORKSPACE_SCHEMA_VERSION,
            run_id: request.run_id.clone(),
            seat_id: request.seat_id.clone(),
            game_id: request.game_id.clone(),
            harness,
            manifest_sha256: manifest.identity().manifest_sha256.clone(),
            retention: manifest.manifest().workspace.retention,
        };
        let marker_path = seat_root.join("workspace.json");
        let config_paths = managed_config_paths(&seat_root);
        if resume {
            verify_marker(&marker_path, &marker, self.owner)?;
            verify_harness_configs(&seat_root, &request.mcp_url, self.owner)?;
        } else {
            write_json_file(&marker_path, &marker)?;
            write_harness_configs(&seat_root, &request.mcp_url)?;
        }
        let integrity = config_paths
            .iter()
            .map(|path| integrity_file(path))
            .collect::<Result<Vec<_>, _>>()?;
        let mcp_config = config.join("mcp.json");
        let environment = Arc::new(SeatProcessEnvironment::new(
            &self.safe_path,
            &workspace,
            &state,
            &home,
            &temporary,
            &mcp_config,
            &request,
            harness,
        ));
        let redactions = vec![request.capability.raw.as_bytes().to_vec()];
        Ok(SeatWorkspaceLease {
            root: seat_root,
            workspace,
            state,
            mcp_config,
            retention: manifest.manifest().workspace.retention,
            sandbox: self.sandbox.clone(),
            network: manifest.manifest().tool_policy.network.clone(),
            environment,
            redactions,
            integrity,
            capability: Some(request.capability),
            authority: self.authority.clone(),
            clock: Arc::clone(&self.clock),
            run_id: request.run_id,
            seat_id: request.seat_id,
            workspace_scan_limit: manifest.manifest().workspace.max_bytes,
            active: true,
        })
    }

    fn record_authority_denial(
        &self,
        run_id: &str,
        seat_id: &str,
        authority: ForbiddenAuthorityKind,
        surface: AuthorityBoundarySurface,
    ) -> Result<(), WorkspaceError> {
        audit_denial(
            self.authority.audit.as_ref(),
            self.clock.as_ref(),
            run_id,
            seat_id,
            authority,
            surface,
        )
        .map_err(|_| WorkspaceError::AuditUnavailable)
    }
}

pub struct SeatWorkspaceLease {
    root: PathBuf,
    workspace: PathBuf,
    state: PathBuf,
    mcp_config: PathBuf,
    retention: WorkspaceRetention,
    sandbox: SeatSandboxBackend,
    network: NetworkPolicy,
    environment: Arc<SeatProcessEnvironment>,
    redactions: Vec<Vec<u8>>,
    integrity: Vec<IntegrityFile>,
    capability: Option<SeatCapability>,
    authority: AuthorityBoundaryConfig,
    clock: Arc<dyn DriverClock>,
    run_id: String,
    seat_id: String,
    workspace_scan_limit: u64,
    active: bool,
}

impl fmt::Debug for SeatWorkspaceLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SeatWorkspaceLease")
            .field("root", &self.root)
            .field("retention", &self.retention)
            .field("sandbox", &self.sandbox)
            .field("environment", &self.environment)
            .field("capability", &"<redacted>")
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

impl SeatWorkspaceLease {
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    #[must_use]
    pub fn state_directory(&self) -> &Path {
        &self.state
    }

    #[must_use]
    pub fn mcp_config(&self) -> &Path {
        &self.mcp_config
    }

    #[must_use]
    pub fn environment_keys(&self) -> Vec<&str> {
        self.environment
            .variables
            .keys()
            .map(String::as_str)
            .collect()
    }

    #[must_use]
    pub fn harness_runtime(&self, executables: HarnessExecutables) -> HarnessRuntimeConfig {
        HarnessRuntimeConfig {
            workspace: self.workspace.clone(),
            state_directory: self.state.clone(),
            mcp_config: self.mcp_config.clone(),
            executables,
        }
    }

    #[must_use]
    pub fn process_adapter(&self) -> Arc<dyn ProcessAdapter> {
        Arc::new(IsolatedSeatProcessAdapter {
            root: self.root.clone(),
            workspace: self.workspace.clone(),
            writable_roots: vec![
                self.workspace.clone(),
                self.state.clone(),
                self.root.join("home"),
                self.root.join("tmp"),
            ],
            sandbox: self.sandbox.clone(),
            network: self.network.clone(),
            environment: self.environment.clone(),
            redactions: self.redactions.clone(),
            integrity: self.integrity.clone(),
            authority: self.authority.clone(),
            clock: Arc::clone(&self.clock),
            run_id: self.run_id.clone(),
            seat_id: self.seat_id.clone(),
            workspace_scan_limit: self.workspace_scan_limit,
        })
    }

    /// Applies the manifest retention policy to a completed or failed run.
    ///
    /// # Errors
    ///
    /// Returns an error if a validated narrow workspace cannot be removed.
    pub fn finish(
        mut self,
        outcome: WorkspaceOutcome,
    ) -> Result<WorkspaceDisposition, WorkspaceError> {
        let retain = matches!(
            (self.retention, outcome),
            (
                WorkspaceRetention::RetainOnFailure,
                WorkspaceOutcome::Failed
            )
        );
        self.active = false;
        self.capability.take();
        if retain {
            Ok(WorkspaceDisposition::Retained(self.root.clone()))
        } else {
            safe_remove_workspace(&self.root)?;
            Ok(WorkspaceDisposition::Deleted)
        }
    }
}

impl Drop for SeatWorkspaceLease {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        self.capability.take();
        if self.retention == WorkspaceRetention::DeleteOnFinish {
            let _ = safe_remove_workspace(&self.root);
        }
        self.active = false;
    }
}

struct SeatProcessEnvironment {
    variables: BTreeMap<String, String>,
}

impl fmt::Debug for SeatProcessEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SeatProcessEnvironment")
            .field("keys", &self.variables.keys().collect::<Vec<_>>())
            .field("values", &"<redacted>")
            .finish()
    }
}

impl SeatProcessEnvironment {
    #[allow(clippy::too_many_arguments)]
    fn new(
        safe_path: &str,
        workspace: &Path,
        state: &Path,
        home: &Path,
        temporary: &Path,
        mcp_config: &Path,
        request: &SeatWorkspaceRequest,
        harness: WorkspaceHarnessKind,
    ) -> Self {
        let mut variables = BTreeMap::from([
            ("HOME".to_owned(), path_text(home)),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
            ("LC_ALL".to_owned(), "C.UTF-8".to_owned()),
            ("PATH".to_owned(), safe_path.to_owned()),
            ("TMPDIR".to_owned(), path_text(temporary)),
            (
                SEAT_CAPABILITY_ENV.to_owned(),
                request.capability.raw.clone(),
            ),
            ("WORD_ARENA_GAME_ID".to_owned(), request.game_id.clone()),
            ("WORD_ARENA_MCP_CONFIG".to_owned(), path_text(mcp_config)),
            ("WORD_ARENA_MCP_URL".to_owned(), request.mcp_url.clone()),
            ("WORD_ARENA_RUN_ID".to_owned(), request.run_id.clone()),
            ("WORD_ARENA_SEAT_ID".to_owned(), request.seat_id.clone()),
            ("WORD_ARENA_STATE_DIRECTORY".to_owned(), path_text(state)),
            ("WORD_ARENA_WORKSPACE".to_owned(), path_text(workspace)),
        ]);
        match harness {
            WorkspaceHarnessKind::Codex => {
                variables.insert("CODEX_HOME".to_owned(), path_text(state));
            }
            WorkspaceHarnessKind::Cline => {
                variables.insert("CLINE_DATA_DIR".to_owned(), path_text(state));
                variables.insert("CLINE_SANDBOX".to_owned(), "1".to_owned());
                variables.insert("CLINE_SANDBOX_DATA_DIR".to_owned(), path_text(state));
            }
            WorkspaceHarnessKind::ClaudeCode => {
                variables.insert("CLAUDE_CODE_SKIP_PROMPT_HISTORY".to_owned(), "1".to_owned());
            }
            WorkspaceHarnessKind::Pi | WorkspaceHarnessKind::GenericCommand => {}
        }
        Self { variables }
    }

    fn spawn_values(&self) -> Vec<(String, String)> {
        self.variables
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }
}

#[derive(Clone, Debug)]
struct IntegrityFile {
    path: PathBuf,
    sha256: [u8; 32],
}

#[derive(Clone)]
struct IsolatedSeatProcessAdapter {
    root: PathBuf,
    workspace: PathBuf,
    writable_roots: Vec<PathBuf>,
    sandbox: SeatSandboxBackend,
    network: NetworkPolicy,
    environment: Arc<SeatProcessEnvironment>,
    redactions: Vec<Vec<u8>>,
    integrity: Vec<IntegrityFile>,
    authority: AuthorityBoundaryConfig,
    clock: Arc<dyn DriverClock>,
    run_id: String,
    seat_id: String,
    workspace_scan_limit: u64,
}

impl fmt::Debug for IsolatedSeatProcessAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IsolatedSeatProcessAdapter")
            .field("root", &"<redacted>")
            .field("sandbox", &self.sandbox)
            .field("network", &self.network)
            .field("environment", &self.environment)
            .field("redactions", &self.redactions.len())
            .field("integrity_files", &self.integrity.len())
            .field("authority", &self.authority)
            .finish_non_exhaustive()
    }
}

impl ProcessAdapter for IsolatedSeatProcessAdapter {
    fn spawn<'a>(
        &'a self,
        spec: &'a ProcessSpec,
    ) -> DriverFuture<'a, Result<Box<dyn ProcessInstance>, ProcessError>> {
        Box::pin(async move {
            if !self.verify_integrity()
                || !self.spec_is_scoped(spec)
                || !self.authority_is_clean(spec)
            {
                return Err(ProcessError::Spawn);
            }
            let sandboxed = self.sandbox_spec(spec);
            let process = spawn_tokio_process(&sandboxed, &self.environment.spawn_values()).await?;
            Ok(
                Box::new(RedactingProcess::new(process, self.redactions.clone()))
                    as Box<dyn ProcessInstance>,
            )
        })
    }

    fn reattach<'a>(
        &'a self,
        _handle: &'a ProcessHandle,
    ) -> DriverFuture<'a, Result<Box<dyn ProcessInstance>, ProcessError>> {
        Box::pin(async { Err(ProcessError::ReattachUnsupported) })
    }
}

impl IsolatedSeatProcessAdapter {
    fn verify_integrity(&self) -> bool {
        self.integrity.iter().all(|expected| {
            integrity_file(&expected.path).is_ok_and(|actual| actual.sha256 == expected.sha256)
        })
    }

    fn spec_is_scoped(&self, spec: &ProcessSpec) -> bool {
        spec.working_directory.as_ref().is_some_and(|directory| {
            fs::canonicalize(directory).is_ok_and(|path| path.starts_with(&self.root))
        })
    }

    fn authority_is_clean(&self, spec: &ProcessSpec) -> bool {
        let argument_authority = std::iter::once(spec.executable.as_str())
            .chain(spec.arguments.iter().map(String::as_str))
            .find_map(|value| {
                self.authority
                    .policy
                    .find(value.as_bytes())
                    .or_else(|| forbidden_authority_marker(value))
            });
        if let Some(authority) = argument_authority {
            self.record_denial(authority, AuthorityBoundarySurface::ProcessArgument);
            return false;
        }
        if let Some(authority) = self.environment.variables.iter().find_map(|(key, value)| {
            self.authority
                .policy
                .find(value.as_bytes())
                .or_else(|| forbidden_authority_marker(key))
        }) {
            self.record_denial(authority, AuthorityBoundarySurface::ProcessEnvironment);
            return false;
        }
        match find_forbidden_workspace_authority(
            &self.root,
            self.workspace_scan_limit,
            &self.authority.policy,
        ) {
            Ok(Some(authority)) => {
                self.record_denial(authority, AuthorityBoundarySurface::WorkspaceFile);
                false
            }
            Ok(None) => true,
            Err(_) => false,
        }
    }

    fn record_denial(&self, authority: ForbiddenAuthorityKind, surface: AuthorityBoundarySurface) {
        let _ = audit_denial(
            self.authority.audit.as_ref(),
            self.clock.as_ref(),
            &self.run_id,
            &self.seat_id,
            authority,
            surface,
        );
    }

    fn sandbox_spec(&self, spec: &ProcessSpec) -> ProcessSpec {
        match &self.sandbox {
            SeatSandboxBackend::MacOsSandboxExec {
                executable,
                runtime_read_roots,
            } => {
                let mut profile = String::from(
                    "(version 1) (deny default) (import \"system.sb\") \
                     (allow process*) (allow file-read-metadata)",
                );
                profile.push_str(" (allow file-read*");
                for root in runtime_read_roots.iter().chain([&self.root]) {
                    profile.push_str(" (subpath ");
                    profile.push_str(&sandbox_literal(root));
                    profile.push(')');
                }
                profile.push(')');
                profile.push_str(" (allow file-write*");
                for root in &self.writable_roots {
                    profile.push_str(" (subpath ");
                    profile.push_str(&sandbox_literal(root));
                    profile.push(')');
                }
                profile.push(')');
                if !matches!(self.network, NetworkPolicy::Deny) {
                    profile.push_str(" (allow network-outbound)");
                }
                let mut arguments = vec!["-p".to_owned(), profile, spec.executable.clone()];
                arguments.extend(spec.arguments.clone());
                ProcessSpec {
                    executable: path_text(executable),
                    arguments,
                    working_directory: Some(self.workspace.clone()),
                }
            }
            SeatSandboxBackend::Bubblewrap {
                executable,
                runtime_read_roots,
            } => {
                let mut arguments = vec![
                    "--die-with-parent".to_owned(),
                    "--new-session".to_owned(),
                    "--unshare-all".to_owned(),
                    "--proc".to_owned(),
                    "/proc".to_owned(),
                    "--dev".to_owned(),
                    "/dev".to_owned(),
                    "--tmpfs".to_owned(),
                    "/tmp".to_owned(),
                ];
                if !matches!(self.network, NetworkPolicy::Deny) {
                    arguments.push("--share-net".to_owned());
                }
                for root in runtime_read_roots {
                    arguments.extend(["--ro-bind".to_owned(), path_text(root), path_text(root)]);
                }
                arguments.extend([
                    "--ro-bind".to_owned(),
                    path_text(&self.root),
                    path_text(&self.root),
                ]);
                for root in &self.writable_roots {
                    arguments.extend(["--bind".to_owned(), path_text(root), path_text(root)]);
                }
                arguments.extend([
                    "--chdir".to_owned(),
                    path_text(&self.workspace),
                    "--".to_owned(),
                    spec.executable.clone(),
                ]);
                arguments.extend(spec.arguments.clone());
                ProcessSpec {
                    executable: path_text(executable),
                    arguments,
                    working_directory: Some(self.workspace.clone()),
                }
            }
        }
    }
}

struct RedactingProcess {
    inner: Box<dyn ProcessInstance>,
    secrets: Vec<Vec<u8>>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    queued: VecDeque<ProcessEvent>,
}

impl fmt::Debug for RedactingProcess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RedactingProcess")
            .field("inner", &self.inner)
            .field("secret_count", &self.secrets.len())
            .finish_non_exhaustive()
    }
}

impl RedactingProcess {
    fn new(inner: Box<dyn ProcessInstance>, secrets: Vec<Vec<u8>>) -> Self {
        Self {
            inner,
            secrets,
            stdout: Vec::new(),
            stderr: Vec::new(),
            queued: VecDeque::new(),
        }
    }

    fn take_line(buffer: &mut Vec<u8>, secrets: &[Vec<u8>]) -> Option<Vec<u8>> {
        let newline = buffer.iter().position(|byte| *byte == b'\n')?;
        let line = buffer.drain(..=newline).collect::<Vec<_>>();
        Some(redact_bytes(line, secrets))
    }

    fn take_bounded_chunk(buffer: &mut Vec<u8>, secrets: &[Vec<u8>]) -> Option<Vec<u8>> {
        if buffer.len() <= REDACTION_STREAM_CHUNK_BYTES {
            return None;
        }
        let retained = secrets
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or_default()
            .saturating_sub(1);
        let emitted = buffer.len().saturating_sub(retained);
        Some(buffer.drain(..emitted).collect())
    }

    fn append_redacted(buffer: &mut Vec<u8>, bytes: &[u8], secrets: &[Vec<u8>]) {
        buffer.extend_from_slice(bytes);
        *buffer = redact_bytes(std::mem::take(buffer), secrets);
    }

    fn queue_exit(&mut self, exit: ExitStatus) {
        if !self.stdout.is_empty() {
            self.queued.push_back(ProcessEvent::Stdout(redact_bytes(
                std::mem::take(&mut self.stdout),
                &self.secrets,
            )));
        }
        if !self.stderr.is_empty() {
            self.queued.push_back(ProcessEvent::Stderr(redact_bytes(
                std::mem::take(&mut self.stderr),
                &self.secrets,
            )));
        }
        self.queued.push_back(ProcessEvent::Exited(exit));
    }
}

impl ProcessInstance for RedactingProcess {
    fn handle(&self) -> ProcessHandle {
        self.inner.handle()
    }

    fn write<'a>(&'a mut self, bytes: &'a [u8]) -> DriverFuture<'a, Result<(), ProcessError>> {
        self.inner.write(bytes)
    }

    fn next_event(&mut self) -> DriverFuture<'_, Result<ProcessEvent, ProcessError>> {
        Box::pin(async move {
            loop {
                if let Some(event) = self.queued.pop_front() {
                    return Ok(event);
                }
                if let Some(line) = Self::take_line(&mut self.stdout, &self.secrets) {
                    return Ok(ProcessEvent::Stdout(line));
                }
                if let Some(line) = Self::take_line(&mut self.stderr, &self.secrets) {
                    return Ok(ProcessEvent::Stderr(line));
                }
                if let Some(bytes) = Self::take_bounded_chunk(&mut self.stdout, &self.secrets) {
                    return Ok(ProcessEvent::Stdout(bytes));
                }
                if let Some(bytes) = Self::take_bounded_chunk(&mut self.stderr, &self.secrets) {
                    return Ok(ProcessEvent::Stderr(bytes));
                }
                match self.inner.next_event().await? {
                    ProcessEvent::Stdout(bytes) => {
                        Self::append_redacted(&mut self.stdout, &bytes, &self.secrets);
                    }
                    ProcessEvent::Stderr(bytes) => {
                        Self::append_redacted(&mut self.stderr, &bytes, &self.secrets);
                    }
                    ProcessEvent::Exited(exit) => self.queue_exit(exit),
                }
            }
        })
    }

    fn terminate(&mut self) -> DriverFuture<'_, Result<ExitStatus, ProcessError>> {
        self.inner.terminate()
    }
}

fn redact_bytes(mut value: Vec<u8>, secrets: &[Vec<u8>]) -> Vec<u8> {
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        let mut offset = 0_usize;
        while offset.saturating_add(secret.len()) <= value.len() {
            if value[offset..].starts_with(secret) {
                value.splice(offset..offset + secret.len(), b"[REDACTED]".iter().copied());
                offset += b"[REDACTED]".len();
            } else {
                offset += 1;
            }
        }
    }
    value
}

fn prepare_manager_root(requested: &Path) -> Result<PathBuf, WorkspaceError> {
    if !requested.is_absolute() || requested.file_name().is_none() {
        return Err(WorkspaceError::InvalidPath);
    }
    match fs::symlink_metadata(requested) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(WorkspaceError::InvalidPath);
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = requested.parent().ok_or(WorkspaceError::InvalidPath)?;
            if !parent.is_dir() {
                return Err(WorkspaceError::InvalidPath);
            }
            create_private_directory(requested)?;
        }
        Err(_) => return Err(WorkspaceError::Filesystem),
    }
    set_private_directory_permissions(requested)?;
    fs::canonicalize(requested).map_err(|_| WorkspaceError::Filesystem)
}

fn ensure_private_directory(
    path: &Path,
    owner: u32,
    create_if_missing: bool,
) -> Result<(), WorkspaceError> {
    match fs::symlink_metadata(path) {
        Ok(_) => validate_private_directory(path, owner),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create_if_missing => {
            create_private_directory(path)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(WorkspaceError::Missing),
        Err(_) => Err(WorkspaceError::Filesystem),
    }
}

fn create_private_directory(path: &Path) -> Result<(), WorkspaceError> {
    match fs::create_dir(path) {
        Ok(()) => set_private_directory_permissions(path),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(WorkspaceError::AlreadyExists)
        }
        Err(_) => Err(WorkspaceError::Filesystem),
    }
}

fn validate_private_directory(path: &Path, owner: u32) -> Result<(), WorkspaceError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| WorkspaceError::Missing)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || owner_id(&metadata) != owner
        || permission_mode(&metadata) & 0o077 != 0
    {
        return Err(WorkspaceError::Corrupt);
    }
    Ok(())
}

fn set_private_directory_permissions(path: &Path) -> Result<(), WorkspaceError> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(DIRECTORY_MODE))
        .map_err(|_| WorkspaceError::Filesystem)?;
    Ok(())
}

fn write_json_file(path: &Path, value: &impl Serialize) -> Result<(), WorkspaceError> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|_| WorkspaceError::Corrupt)?;
    bytes.push(b'\n');
    write_private_file(path, &bytes)
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), WorkspaceError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(FILE_MODE);
    let mut file = options.open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            WorkspaceError::AlreadyExists
        } else {
            WorkspaceError::Filesystem
        }
    })?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| WorkspaceError::Filesystem)
}

fn write_harness_configs(root: &Path, mcp_url: &str) -> Result<(), WorkspaceError> {
    for (relative, bytes) in harness_config_payloads(mcp_url)? {
        write_private_file(&root.join(relative), &bytes)?;
    }
    Ok(())
}

fn harness_config_payloads(mcp_url: &str) -> Result<Vec<(PathBuf, Vec<u8>)>, WorkspaceError> {
    let common = json!({
        "schema_version": 1,
        "mcpServers": {
            "word_arena": {
                "type": "http",
                "url": mcp_url,
                "headers": {
                    "Authorization": format!("Bearer ${{{SEAT_CAPABILITY_ENV}}}")
                }
            }
        }
    });
    let mut common_bytes =
        serde_json::to_vec_pretty(&common).map_err(|_| WorkspaceError::Corrupt)?;
    common_bytes.push(b'\n');
    let codex = format!(
        "[mcp_servers.word_arena]\nurl = \"{mcp_url}\"\nbearer_token_env_var = \"{SEAT_CAPABILITY_ENV}\"\nrequired = true\n"
    );
    let cli = json!({
        "schema_version": 1,
        "server_url": mcp_url,
        "authorization": { "kind": "bearer_env", "environment": SEAT_CAPABILITY_ENV }
    });
    let mut cli_bytes = serde_json::to_vec_pretty(&cli).map_err(|_| WorkspaceError::Corrupt)?;
    cli_bytes.push(b'\n');
    Ok(vec![
        (PathBuf::from("config/mcp.json"), common_bytes),
        (
            PathBuf::from("config/codex.toml"),
            codex.as_bytes().to_vec(),
        ),
        (PathBuf::from("config/word-arena-cli.json"), cli_bytes),
        (
            PathBuf::from("state/config.toml"),
            codex.as_bytes().to_vec(),
        ),
    ])
}

fn managed_config_paths(root: &Path) -> Vec<PathBuf> {
    [
        "config/mcp.json",
        "config/codex.toml",
        "config/word-arena-cli.json",
        "state/config.toml",
    ]
    .into_iter()
    .map(|relative| root.join(relative))
    .collect()
}

fn verify_harness_configs(root: &Path, mcp_url: &str, owner: u32) -> Result<(), WorkspaceError> {
    for (relative, expected) in harness_config_payloads(mcp_url)? {
        let path = root.join(relative);
        validate_private_file(&path, owner)?;
        let actual = fs::read(path).map_err(|_| WorkspaceError::Filesystem)?;
        if actual != expected {
            return Err(WorkspaceError::Corrupt);
        }
    }
    Ok(())
}

fn verify_marker(
    path: &Path,
    expected: &WorkspaceMarker,
    owner: u32,
) -> Result<(), WorkspaceError> {
    validate_private_file(path, owner)?;
    let bytes = fs::read(path).map_err(|_| WorkspaceError::Filesystem)?;
    let actual: WorkspaceMarker =
        serde_json::from_slice(&bytes).map_err(|_| WorkspaceError::Corrupt)?;
    if actual.schema_version != expected.schema_version
        || actual.run_id != expected.run_id
        || actual.seat_id != expected.seat_id
        || actual.game_id != expected.game_id
        || actual.harness != expected.harness
        || actual.manifest_sha256 != expected.manifest_sha256
        || actual.retention != expected.retention
    {
        return Err(WorkspaceError::Corrupt);
    }
    Ok(())
}

fn validate_private_file(path: &Path, owner: u32) -> Result<(), WorkspaceError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| WorkspaceError::Missing)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || owner_id(&metadata) != owner
        || permission_mode(&metadata) & 0o077 != 0
    {
        return Err(WorkspaceError::Corrupt);
    }
    Ok(())
}

fn integrity_file(path: &Path) -> Result<IntegrityFile, WorkspaceError> {
    let mut file = File::open(path).map_err(|_| WorkspaceError::Filesystem)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| WorkspaceError::Filesystem)?;
    Ok(IntegrityFile {
        path: path.to_owned(),
        sha256: Sha256::digest(bytes).into(),
    })
}

fn find_forbidden_workspace_authority(
    root: &Path,
    maximum_bytes: u64,
    policy: &ForbiddenAuthorityPolicy,
) -> Result<Option<ForbiddenAuthorityKind>, WorkspaceError> {
    let mut pending = vec![root.to_owned()];
    let mut scanned_bytes = 0_u64;
    let mut entry_count = 0_u64;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).map_err(|_| WorkspaceError::Filesystem)? {
            let entry = entry.map_err(|_| WorkspaceError::Filesystem)?;
            entry_count = entry_count.saturating_add(1);
            if entry_count > 100_000 {
                return Err(WorkspaceError::Corrupt);
            }
            let path = entry.path();
            let file_type = entry.file_type().map_err(|_| WorkspaceError::Filesystem)?;
            if file_type.is_symlink() {
                continue;
            }
            if let Some(authority) = entry
                .file_name()
                .to_str()
                .and_then(forbidden_authority_marker)
            {
                return Ok(Some(authority));
            }
            if file_type.is_dir() {
                pending.push(path);
                continue;
            }
            if !file_type.is_file() {
                return Err(WorkspaceError::Corrupt);
            }
            let metadata = entry.metadata().map_err(|_| WorkspaceError::Filesystem)?;
            scanned_bytes = scanned_bytes.saturating_add(metadata.len());
            if scanned_bytes > maximum_bytes {
                return Err(WorkspaceError::Corrupt);
            }
            let bytes = fs::read(path).map_err(|_| WorkspaceError::Filesystem)?;
            if let Some(authority) = policy.find(&bytes) {
                return Ok(Some(authority));
            }
        }
    }
    Ok(None)
}

fn safe_remove_workspace(path: &Path) -> Result<(), WorkspaceError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| WorkspaceError::Missing)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || path.file_name().is_none()
        || path.parent().and_then(Path::parent).is_none()
    {
        return Err(WorkspaceError::InvalidPath);
    }
    fs::remove_dir_all(path).map_err(|_| WorkspaceError::Filesystem)
}

fn validate_identifier(value: &str) -> Result<(), WorkspaceError> {
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || matches!(value, "." | "..")
    {
        return Err(WorkspaceError::InvalidIdentifier);
    }
    Ok(())
}

fn validate_mcp_url(value: &str) -> Result<(), WorkspaceError> {
    let scheme_end = value
        .find("://")
        .ok_or(WorkspaceError::InvalidMcpEndpoint)?;
    let scheme = &value[..scheme_end];
    let authority_and_path = &value[scheme_end + 3..];
    let authority = authority_and_path.split('/').next().unwrap_or_default();
    if !matches!(scheme, "http" | "https")
        || authority.is_empty()
        || authority.contains('@')
        || value.contains(['\0', '\n', '\r', '"', '#', '?'])
        || value.len() > 2_048
    {
        return Err(WorkspaceError::InvalidMcpEndpoint);
    }
    Ok(())
}

fn validate_safe_path(value: &str) -> Result<(), WorkspaceError> {
    if value.is_empty()
        || value.contains(['\0', '\n', '\r'])
        || value
            .split(':')
            .any(|component| !Path::new(component).is_absolute())
    {
        return Err(WorkspaceError::InvalidPath);
    }
    Ok(())
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn sandbox_literal(path: &Path) -> String {
    let value = path_text(path).replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{value}\"")
}

#[cfg(unix)]
fn owner_id(metadata: &fs::Metadata) -> u32 {
    metadata.uid()
}

#[cfg(not(unix))]
fn owner_id(_metadata: &fs::Metadata) -> u32 {
    0
}

#[cfg(unix)]
fn permission_mode(metadata: &fs::Metadata) -> u32 {
    metadata.permissions().mode()
}

#[cfg(not(unix))]
fn permission_mode(_metadata: &fs::Metadata) -> u32 {
    0
}
