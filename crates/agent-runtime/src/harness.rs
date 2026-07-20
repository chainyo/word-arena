use std::{
    collections::BTreeSet,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::{
    AgentDriver, AgentManifestIdentity, DRIVER_CHECKPOINT_SCHEMA_VERSION,
    DRIVER_TELEMETRY_SCHEMA_VERSION, DiagnosticRecord, DiagnosticStream, DriverCheckpoint,
    DriverClock, DriverError, DriverFuture, DriverLifecycleState, DriverTelemetry, ExitStatus,
    GenericCommandDriver, HarnessConfig, LifecycleTransition, LocalModelRuntime, ModelProvider,
    ModelSource, NetworkPolicy, ProcessAdapter, ProcessError, ProcessEvent, ProcessInstance,
    ProcessSpec, TURN_PROTOCOL_SCHEMA_VERSION, TerminationReason, TurnRequest, TurnTelemetry,
    ValidatedAgentManifest, VisibleToolCall, VisibleTurnOutput,
};

const MAX_NATIVE_STDOUT_BYTES: usize = 1_048_576;

pub const CODEX_MINIMUM_VERSION: &str = "0.144.0";
pub const CLAUDE_CODE_MINIMUM_VERSION: &str = "2.1.205";
pub const CLINE_MINIMUM_VERSION: &str = "3.0.46";
pub const PI_MINIMUM_VERSION: &str = "0.73.1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeHarnessKind {
    Codex,
    ClaudeCode,
    Cline,
    Pi,
}

impl NativeHarnessKind {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude_code",
            Self::Cline => "cline",
            Self::Pi => "pi",
        }
    }

    #[must_use]
    pub const fn minimum_version(self) -> &'static str {
        match self {
            Self::Codex => CODEX_MINIMUM_VERSION,
            Self::ClaudeCode => CLAUDE_CODE_MINIMUM_VERSION,
            Self::Cline => CLINE_MINIMUM_VERSION,
            Self::Pi => PI_MINIMUM_VERSION,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessExecutables {
    pub codex: String,
    pub claude_code: String,
    pub cline: String,
    pub pi: String,
}

impl Default for HarnessExecutables {
    fn default() -> Self {
        Self {
            codex: "codex".to_owned(),
            claude_code: "claude".to_owned(),
            cline: "cline".to_owned(),
            pi: "pi".to_owned(),
        }
    }
}

impl HarnessExecutables {
    fn get(&self, kind: NativeHarnessKind) -> &str {
        match kind {
            NativeHarnessKind::Codex => &self.codex,
            NativeHarnessKind::ClaudeCode => &self.claude_code,
            NativeHarnessKind::Cline => &self.cline,
            NativeHarnessKind::Pi => &self.pi,
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessRuntimeConfig {
    pub workspace: PathBuf,
    pub state_directory: PathBuf,
    pub mcp_config: PathBuf,
    #[serde(default)]
    pub executables: HarnessExecutables,
}

impl fmt::Debug for HarnessRuntimeConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HarnessRuntimeConfig")
            .field("workspace", &"<redacted>")
            .field("state_directory", &"<redacted>")
            .field("mcp_config", &"<redacted>")
            .field("executables", &self.executables)
            .finish()
    }
}

impl HarnessRuntimeConfig {
    fn validate(&self) -> Result<(), DriverError> {
        if [&self.workspace, &self.state_directory, &self.mcp_config]
            .iter()
            .any(|path| !path.is_absolute())
            || [
                &self.executables.codex,
                &self.executables.claude_code,
                &self.executables.cline,
                &self.executables.pi,
            ]
            .iter()
            .any(|executable| executable.is_empty() || executable.chars().any(char::is_control))
        {
            return Err(DriverError::InvalidHarnessRuntime);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessPolicyTranslation {
    pub allowed_tools: BTreeSet<String>,
    pub denied_tools: BTreeSet<String>,
    pub network: NetworkPolicy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeHarnessCheckpoint {
    pub schema_version: u32,
    pub run_id: String,
    pub manifest: AgentManifestIdentity,
    pub harness: NativeHarnessKind,
    pub state: DriverLifecycleState,
    pub telemetry: DriverTelemetry,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "driver", rename_all = "snake_case", deny_unknown_fields)]
pub enum SupportedDriverCheckpoint {
    Generic { checkpoint: DriverCheckpoint },
    Native { checkpoint: NativeHarnessCheckpoint },
}

pub enum SupportedAgentDriver {
    Generic(GenericCommandDriver),
    Native(NativeHarnessDriver),
}

impl fmt::Debug for SupportedAgentDriver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Generic(driver) => formatter.debug_tuple("Generic").field(driver).finish(),
            Self::Native(driver) => formatter.debug_tuple("Native").field(driver).finish(),
        }
    }
}

impl SupportedAgentDriver {
    /// Builds the generic or native driver selected by the immutable manifest.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid trusted runtime paths or malformed generic
    /// command configuration.
    pub fn new(
        run_id: impl Into<String>,
        manifest: &ValidatedAgentManifest,
        runtime: HarnessRuntimeConfig,
        adapter: Arc<dyn ProcessAdapter>,
        clock: Arc<dyn DriverClock>,
    ) -> Result<Self, DriverError> {
        runtime.validate()?;
        let run_id = run_id.into();
        match manifest.manifest().harness {
            HarnessConfig::GenericCommand { .. } => {
                let process_spec = bound_generic_process_spec(manifest, &runtime)?;
                GenericCommandDriver::new_with_process_spec(
                    run_id,
                    manifest,
                    process_spec,
                    adapter,
                    clock,
                )
                .map(Self::Generic)
            }
            _ => NativeHarnessDriver::new(run_id, manifest, runtime, adapter, clock)
                .map(Self::Native),
        }
    }

    /// Restores either supported driver from its tagged stable checkpoint.
    ///
    /// # Errors
    ///
    /// Returns an error when the checkpoint kind, manifest identity, runtime,
    /// or lifecycle is incompatible.
    pub fn restore(
        manifest: &ValidatedAgentManifest,
        checkpoint: SupportedDriverCheckpoint,
        runtime: HarnessRuntimeConfig,
        adapter: Arc<dyn ProcessAdapter>,
        clock: Arc<dyn DriverClock>,
    ) -> Result<Self, DriverError> {
        runtime.validate()?;
        match checkpoint {
            SupportedDriverCheckpoint::Generic { checkpoint } => {
                let process_spec = bound_generic_process_spec(manifest, &runtime)?;
                GenericCommandDriver::restore_with_process_spec(
                    manifest,
                    checkpoint,
                    &process_spec,
                    adapter,
                    clock,
                )
                .map(Self::Generic)
            }
            SupportedDriverCheckpoint::Native { checkpoint } => {
                NativeHarnessDriver::restore(manifest, checkpoint, runtime, adapter, clock)
                    .map(Self::Native)
            }
        }
    }

    /// Captures a tagged checkpoint for application-level persistence.
    ///
    /// # Errors
    ///
    /// Returns an error during an unstable lifecycle transition.
    pub fn checkpoint(&self) -> Result<SupportedDriverCheckpoint, DriverError> {
        match self {
            Self::Generic(driver) => driver
                .checkpoint()
                .map(|checkpoint| SupportedDriverCheckpoint::Generic { checkpoint }),
            Self::Native(driver) => driver
                .checkpoint()
                .map(|checkpoint| SupportedDriverCheckpoint::Native { checkpoint }),
        }
    }
}

impl AgentDriver for SupportedAgentDriver {
    fn state(&self) -> &DriverLifecycleState {
        match self {
            Self::Generic(driver) => driver.state(),
            Self::Native(driver) => driver.state(),
        }
    }

    fn telemetry(&self) -> &DriverTelemetry {
        match self {
            Self::Generic(driver) => driver.telemetry(),
            Self::Native(driver) => driver.telemetry(),
        }
    }

    fn start<'a>(
        &'a mut self,
        cancel: &'a CancellationToken,
    ) -> DriverFuture<'a, Result<(), DriverError>> {
        match self {
            Self::Generic(driver) => driver.start(cancel),
            Self::Native(driver) => driver.start(cancel),
        }
    }

    fn request_turn<'a>(
        &'a mut self,
        request: TurnRequest,
        cancel: &'a CancellationToken,
    ) -> DriverFuture<'a, Result<VisibleTurnOutput, DriverError>> {
        match self {
            Self::Generic(driver) => driver.request_turn(request, cancel),
            Self::Native(driver) => driver.request_turn(request, cancel),
        }
    }

    fn resume<'a>(
        &'a mut self,
        cancel: &'a CancellationToken,
    ) -> DriverFuture<'a, Result<(), DriverError>> {
        match self {
            Self::Generic(driver) => driver.resume(cancel),
            Self::Native(driver) => driver.resume(cancel),
        }
    }

    fn terminate(
        &mut self,
        reason: TerminationReason,
    ) -> DriverFuture<'_, Result<(), DriverError>> {
        match self {
            Self::Generic(driver) => driver.terminate(reason),
            Self::Native(driver) => driver.terminate(reason),
        }
    }
}

pub struct NativeHarnessDriver {
    adapter: Arc<dyn ProcessAdapter>,
    clock: Arc<dyn DriverClock>,
    state: DriverLifecycleState,
    manifest: AgentManifestIdentity,
    kind: NativeHarnessKind,
    expected_version: String,
    model_id: String,
    model_source: ModelSource,
    runtime: HarnessRuntimeConfig,
    policy: HarnessPolicyTranslation,
    process: Option<Box<dyn ProcessInstance>>,
    run_id: String,
    telemetry: DriverTelemetry,
    needs_probe: bool,
}

impl fmt::Debug for NativeHarnessDriver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeHarnessDriver")
            .field("state", &self.state)
            .field("manifest", &self.manifest)
            .field("kind", &self.kind)
            .field("expected_version", &self.expected_version)
            .field("runtime", &self.runtime)
            .field("needs_probe", &self.needs_probe)
            .finish_non_exhaustive()
    }
}

impl NativeHarnessDriver {
    /// Creates a pending native harness driver.
    ///
    /// # Errors
    ///
    /// Returns an error for a generic manifest, invalid run ID, or invalid
    /// trusted runtime configuration.
    pub fn new(
        run_id: impl Into<String>,
        manifest: &ValidatedAgentManifest,
        runtime: HarnessRuntimeConfig,
        adapter: Arc<dyn ProcessAdapter>,
        clock: Arc<dyn DriverClock>,
    ) -> Result<Self, DriverError> {
        runtime.validate()?;
        let (kind, expected_version) = native_harness(&manifest.manifest().harness)?;
        let run_id = run_id.into();
        validate_run_id(&run_id)?;
        let state = DriverLifecycleState::Pending;
        let telemetry = DriverTelemetry {
            schema_version: DRIVER_TELEMETRY_SCHEMA_VERSION,
            run_id: run_id.clone(),
            manifest: manifest.identity().clone(),
            restarts: 0,
            lifecycle: vec![LifecycleTransition {
                sequence: 0,
                at_unix_ms: clock.now_unix_ms(),
                state: state.clone(),
            }],
            turns: Vec::new(),
            diagnostics: Vec::new(),
        };
        Ok(Self {
            adapter,
            clock,
            state,
            manifest: manifest.identity().clone(),
            kind,
            expected_version: expected_version.to_owned(),
            model_id: manifest.manifest().model.id.clone(),
            model_source: manifest.manifest().model.source.clone(),
            runtime,
            policy: HarnessPolicyTranslation {
                allowed_tools: manifest.manifest().tool_policy.allowed_tools.clone(),
                denied_tools: manifest.manifest().tool_policy.denied_tools.clone(),
                network: manifest.manifest().tool_policy.network.clone(),
            },
            process: None,
            run_id,
            telemetry,
            needs_probe: true,
        })
    }

    /// Restores visible telemetry and re-probes the executable on resume.
    ///
    /// # Errors
    ///
    /// Returns an error when the checkpoint or manifest is incompatible.
    pub fn restore(
        manifest: &ValidatedAgentManifest,
        checkpoint: NativeHarnessCheckpoint,
        runtime: HarnessRuntimeConfig,
        adapter: Arc<dyn ProcessAdapter>,
        clock: Arc<dyn DriverClock>,
    ) -> Result<Self, DriverError> {
        runtime.validate()?;
        let (kind, expected_version) = native_harness(&manifest.manifest().harness)?;
        validate_native_checkpoint(manifest, &checkpoint, kind)?;
        let needs_probe = matches!(checkpoint.state, DriverLifecycleState::Ready);
        Ok(Self {
            adapter,
            clock,
            state: checkpoint.state,
            manifest: checkpoint.manifest,
            kind,
            expected_version: expected_version.to_owned(),
            model_id: manifest.manifest().model.id.clone(),
            model_source: manifest.manifest().model.source.clone(),
            runtime,
            policy: HarnessPolicyTranslation {
                allowed_tools: manifest.manifest().tool_policy.allowed_tools.clone(),
                denied_tools: manifest.manifest().tool_policy.denied_tools.clone(),
                network: manifest.manifest().tool_policy.network.clone(),
            },
            process: None,
            run_id: checkpoint.run_id,
            telemetry: checkpoint.telemetry,
            needs_probe,
        })
    }

    #[must_use]
    pub const fn kind(&self) -> NativeHarnessKind {
        self.kind
    }

    #[must_use]
    pub const fn policy(&self) -> &HarnessPolicyTranslation {
        &self.policy
    }

    /// Captures stable visible lifecycle state without commands or credentials.
    ///
    /// # Errors
    ///
    /// Returns an error during a transient lifecycle operation.
    pub fn checkpoint(&self) -> Result<NativeHarnessCheckpoint, DriverError> {
        if matches!(
            self.state,
            DriverLifecycleState::Starting
                | DriverLifecycleState::TurnRunning { .. }
                | DriverLifecycleState::Terminating
        ) {
            return Err(DriverError::InvalidCheckpoint);
        }
        Ok(NativeHarnessCheckpoint {
            schema_version: DRIVER_CHECKPOINT_SCHEMA_VERSION,
            run_id: self.run_id.clone(),
            manifest: self.manifest.clone(),
            harness: self.kind,
            state: self.state.clone(),
            telemetry: self.telemetry.clone(),
        })
    }

    fn transition(&mut self, state: DriverLifecycleState) {
        self.state = state.clone();
        self.telemetry.lifecycle.push(LifecycleTransition {
            sequence: self.telemetry.lifecycle.len() as u64,
            at_unix_ms: self.clock.now_unix_ms(),
            state,
        });
    }

    fn diagnostic(&mut self, code: &str, visible_text: String) {
        self.telemetry.diagnostics.push(DiagnosticRecord {
            sequence: self.telemetry.diagnostics.len() as u64,
            at_unix_ms: self.clock.now_unix_ms(),
            stream: DiagnosticStream::Driver,
            code: code.to_owned(),
            visible_text,
        });
    }

    fn mark_crashed(&mut self, exit: ExitStatus) {
        self.process = None;
        self.transition(DriverLifecycleState::Crashed { exit });
    }

    async fn cancellation_error(&mut self) -> DriverError {
        if let Some(process) = self.process.as_mut()
            && let Err(error) = process.terminate().await
        {
            self.diagnostic(
                "cancellation_termination_failed",
                format!("{} process termination failed: {error}", self.kind.name()),
            );
        }
        self.process = None;
        self.transition(DriverLifecycleState::Terminated {
            reason: TerminationReason::Cancelled,
        });
        DriverError::Cancelled
    }

    async fn run_process(
        &mut self,
        spec: &ProcessSpec,
        cancel: &CancellationToken,
    ) -> Result<CapturedProcess, DriverError> {
        let spawned = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(self.cancellation_error().await),
            result = self.adapter.spawn(spec) => result,
        }?;
        self.process = Some(spawned);
        self.process
            .as_mut()
            .ok_or(DriverError::InvalidCheckpoint)?
            .close_input()
            .await?;
        let mut stdout = Vec::new();
        let mut stderr_bytes = 0_u64;
        let mut stderr_classification = NativeStderrClassification::default();
        loop {
            let event = {
                let process = self
                    .process
                    .as_mut()
                    .ok_or(DriverError::InvalidCheckpoint)?;
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => return Err(self.cancellation_error().await),
                    result = process.next_event() => result,
                }
            }?;
            match event {
                ProcessEvent::Stdout(bytes) => {
                    if stdout.len().saturating_add(bytes.len()) > MAX_NATIVE_STDOUT_BYTES {
                        if let Some(process) = self.process.as_mut() {
                            let _ = process.terminate().await;
                        }
                        self.process = None;
                        return Err(DriverError::FrameTooLarge);
                    }
                    stdout.extend_from_slice(&bytes);
                }
                ProcessEvent::Stderr(bytes) => {
                    stderr_bytes = stderr_bytes.saturating_add(bytes.len() as u64);
                    stderr_classification.observe(&bytes);
                }
                ProcessEvent::Exited(exit) => {
                    self.process = None;
                    return Ok(CapturedProcess {
                        stdout,
                        stderr_bytes,
                        stderr_classification,
                        exit,
                    });
                }
            }
        }
    }

    async fn probe(&mut self, cancel: &CancellationToken) -> Result<(), DriverError> {
        self.transition(DriverLifecycleState::Starting);
        let spec = ProcessSpec {
            executable: self.runtime.executables.get(self.kind).to_owned(),
            arguments: version_arguments(self.kind),
            working_directory: Some(self.runtime.workspace.clone()),
        };
        let captured = match self.run_process(&spec, cancel).await {
            Ok(captured) => captured,
            Err(DriverError::Process(ProcessError::Spawn)) => {
                let exit = synthetic_failure();
                self.mark_crashed(exit);
                return Err(DriverError::HarnessUnavailable {
                    harness: self.kind.name(),
                    executable: spec.executable,
                });
            }
            Err(error) => {
                if !matches!(error, DriverError::Cancelled) {
                    self.mark_crashed(synthetic_failure());
                }
                return Err(error);
            }
        };
        if !captured.exit.success {
            self.failed_process_diagnostic(&captured);
            self.mark_crashed(captured.exit.clone());
            return Err(DriverError::HarnessExit {
                harness: self.kind.name(),
                exit: captured.exit,
            });
        }
        let installed = parse_reported_version(&captured.stdout).ok_or_else(|| {
            DriverError::HarnessVersionUnparseable {
                harness: self.kind.name(),
            }
        });
        let installed = match installed {
            Ok(installed) => installed,
            Err(error) => {
                self.mark_crashed(synthetic_failure());
                return Err(error);
            }
        };
        let minimum = Version::parse(self.kind.minimum_version())
            .expect("reviewed minimum harness versions are valid semver");
        if installed < minimum {
            self.mark_crashed(synthetic_failure());
            return Err(DriverError::HarnessVersionUnsupported {
                harness: self.kind.name(),
                installed: installed.to_string(),
                minimum: self.kind.minimum_version(),
            });
        }
        if installed.to_string() != self.expected_version {
            self.mark_crashed(synthetic_failure());
            return Err(DriverError::HarnessVersionMismatch {
                harness: self.kind.name(),
                expected: self.expected_version.clone(),
                installed: installed.to_string(),
            });
        }
        if captured.stderr_bytes > 0 {
            self.diagnostic(
                "version_probe_stderr_redacted",
                format!(
                    "{} version probe emitted {} redacted stderr bytes",
                    self.kind.name(),
                    captured.stderr_bytes
                ),
            );
        }
        self.needs_probe = false;
        self.diagnostic(
            "harness_ready",
            format!("{} {} is compatible", self.kind.name(), installed),
        );
        self.transition(DriverLifecycleState::Ready);
        Ok(())
    }

    fn turn_spec(&self, request: &TurnRequest) -> ProcessSpec {
        let workspace = path_argument(&self.runtime.workspace);
        let state_directory = path_argument(&self.runtime.state_directory);
        let mcp_config = path_argument(&self.runtime.mcp_config);
        let mut arguments = match self.kind {
            NativeHarnessKind::Codex => vec![
                "--ask-for-approval".to_owned(),
                "never".to_owned(),
                "exec".to_owned(),
                "--json".to_owned(),
                "--ephemeral".to_owned(),
                "--ignore-rules".to_owned(),
                "--skip-git-repo-check".to_owned(),
                "--sandbox".to_owned(),
                "workspace-write".to_owned(),
                "--config".to_owned(),
                "web_search=\"disabled\"".to_owned(),
                "--config".to_owned(),
                "sandbox_workspace_write.network_access=false".to_owned(),
                "--cd".to_owned(),
                workspace,
            ],
            NativeHarnessKind::ClaudeCode => vec![
                "--print".to_owned(),
                "--output-format".to_owned(),
                "stream-json".to_owned(),
                "--verbose".to_owned(),
                "--permission-mode".to_owned(),
                "dontAsk".to_owned(),
                "--strict-mcp-config".to_owned(),
                "--mcp-config".to_owned(),
                mcp_config,
            ],
            NativeHarnessKind::Cline => {
                let arguments = vec![
                    "--json".to_owned(),
                    "--auto-approve".to_owned(),
                    "true".to_owned(),
                    "--cwd".to_owned(),
                    workspace,
                    "--data-dir".to_owned(),
                    state_directory,
                    "--config".to_owned(),
                    path_argument(
                        self.runtime
                            .mcp_config
                            .parent()
                            .unwrap_or(&self.runtime.mcp_config),
                    ),
                ];
                arguments
            }
            NativeHarnessKind::Pi => {
                let mut arguments = vec![
                    "--mode".to_owned(),
                    "json".to_owned(),
                    "--session-dir".to_owned(),
                    state_directory,
                ];
                arguments.push("--print".to_owned());
                arguments
            }
        };
        if !matches!(self.model_source, ModelSource::HarnessDefault) {
            arguments.extend(["--model".to_owned(), self.model_id.clone()]);
            if matches!(self.kind, NativeHarnessKind::Cline | NativeHarnessKind::Pi) {
                append_provider(&mut arguments, "--provider", &self.model_source);
            }
        }
        arguments.push(request.visible_input.clone());
        ProcessSpec {
            executable: self.runtime.executables.get(self.kind).to_owned(),
            arguments,
            working_directory: Some(self.runtime.workspace.clone()),
        }
    }

    async fn request_native_turn(
        &mut self,
        request: TurnRequest,
        cancel: &CancellationToken,
    ) -> Result<VisibleTurnOutput, DriverError> {
        if request.turn_id.is_empty() || request.turn_id.chars().any(char::is_control) {
            return Err(DriverError::InvalidFrame);
        }
        if cancel.is_cancelled() {
            return Err(self.cancellation_error().await);
        }
        let started_at_unix_ms = self.clock.now_unix_ms();
        self.transition(DriverLifecycleState::TurnRunning {
            turn_id: request.turn_id.clone(),
        });
        let captured = match self.run_process(&self.turn_spec(&request), cancel).await {
            Ok(captured) => captured,
            Err(error) => {
                if !matches!(error, DriverError::Cancelled) {
                    self.mark_crashed(synthetic_failure());
                }
                return Err(error);
            }
        };
        if !captured.exit.success {
            self.failed_process_diagnostic(&captured);
            self.mark_crashed(captured.exit.clone());
            return Err(DriverError::HarnessExit {
                harness: self.kind.name(),
                exit: captured.exit,
            });
        }
        let Ok(normalized) = normalize_output(self.kind, &captured.stdout) else {
            self.mark_crashed(synthetic_failure());
            self.diagnostic(
                "structured_output_rejected",
                format!(
                    "{} output was rejected without persisting raw bytes",
                    self.kind.name()
                ),
            );
            return Err(DriverError::HarnessOutput {
                harness: self.kind.name(),
            });
        };
        if captured.stderr_bytes > 0 {
            self.diagnostic(
                "harness_stderr_redacted",
                format!(
                    "{} emitted {} redacted stderr bytes",
                    self.kind.name(),
                    captured.stderr_bytes
                ),
            );
        }
        let output = VisibleTurnOutput {
            schema_version: TURN_PROTOCOL_SCHEMA_VERSION,
            turn_id: request.turn_id.clone(),
            visible_output: normalized.visible_output,
            tool_calls: normalized.tool_calls,
        };
        self.telemetry.turns.push(TurnTelemetry {
            turn_id: request.turn_id,
            started_at_unix_ms,
            completed_at_unix_ms: self.clock.now_unix_ms(),
            visible_input: request.visible_input,
            visible_output: output.visible_output.clone(),
            tool_calls: output.tool_calls.clone(),
        });
        self.transition(DriverLifecycleState::Ready);
        Ok(output)
    }

    fn failed_process_diagnostic(&mut self, captured: &CapturedProcess) {
        if captured.stderr_bytes == 0 {
            return;
        }
        let (code, summary) = match captured.stderr_classification {
            NativeStderrClassification::SandboxDenied => (
                "harness_sandbox_denied",
                "was blocked by the local process sandbox",
            ),
            NativeStderrClassification::TlsTrustFailed => (
                "harness_tls_trust_failed",
                "could not establish provider TLS trust inside the local sandbox",
            ),
            NativeStderrClassification::AuthenticationFailed => (
                "harness_authentication_failed",
                "could not authenticate the provider or required MCP session",
            ),
            NativeStderrClassification::McpInitializationFailed => (
                "harness_mcp_initialization_failed",
                "could not initialize the required Word Arena MCP session",
            ),
            NativeStderrClassification::Unclassified => (
                "harness_stderr_redacted",
                "exited after emitting private diagnostics",
            ),
        };
        self.diagnostic(
            code,
            format!(
                "{} {summary}; {} stderr bytes were redacted",
                self.kind.name(),
                captured.stderr_bytes
            ),
        );
    }
}

impl AgentDriver for NativeHarnessDriver {
    fn state(&self) -> &DriverLifecycleState {
        &self.state
    }

    fn telemetry(&self) -> &DriverTelemetry {
        &self.telemetry
    }

    fn start<'a>(
        &'a mut self,
        cancel: &'a CancellationToken,
    ) -> DriverFuture<'a, Result<(), DriverError>> {
        Box::pin(async move {
            if self.state != DriverLifecycleState::Pending {
                return Err(DriverError::InvalidTransition {
                    operation: "start",
                    state: self.state.clone(),
                });
            }
            self.probe(cancel).await
        })
    }

    fn request_turn<'a>(
        &'a mut self,
        request: TurnRequest,
        cancel: &'a CancellationToken,
    ) -> DriverFuture<'a, Result<VisibleTurnOutput, DriverError>> {
        Box::pin(async move {
            if self.state != DriverLifecycleState::Ready || self.needs_probe {
                return Err(DriverError::InvalidTransition {
                    operation: "request_turn",
                    state: self.state.clone(),
                });
            }
            self.request_native_turn(request, cancel).await
        })
    }

    fn resume<'a>(
        &'a mut self,
        cancel: &'a CancellationToken,
    ) -> DriverFuture<'a, Result<(), DriverError>> {
        Box::pin(async move {
            match self.state {
                DriverLifecycleState::Ready if !self.needs_probe => Ok(()),
                DriverLifecycleState::Ready => self.probe(cancel).await,
                DriverLifecycleState::Crashed { .. } => {
                    self.telemetry.restarts = self.telemetry.restarts.saturating_add(1);
                    self.probe(cancel).await
                }
                _ => Err(DriverError::InvalidTransition {
                    operation: "resume",
                    state: self.state.clone(),
                }),
            }
        })
    }

    fn terminate(
        &mut self,
        reason: TerminationReason,
    ) -> DriverFuture<'_, Result<(), DriverError>> {
        Box::pin(async move {
            if matches!(self.state, DriverLifecycleState::Terminated { .. }) {
                return Ok(());
            }
            let previous = self.state.clone();
            self.transition(DriverLifecycleState::Terminating);
            if let Some(process) = self.process.as_mut()
                && let Err(error) = process.terminate().await
            {
                self.diagnostic(
                    "termination_failed",
                    format!("{} process termination failed: {error}", self.kind.name()),
                );
                self.transition(previous);
                return Err(DriverError::Process(error));
            }
            self.process = None;
            self.transition(DriverLifecycleState::Terminated { reason });
            Ok(())
        })
    }
}

#[derive(Debug)]
struct CapturedProcess {
    stdout: Vec<u8>,
    stderr_bytes: u64,
    stderr_classification: NativeStderrClassification,
    exit: ExitStatus,
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
enum NativeStderrClassification {
    #[default]
    Unclassified,
    McpInitializationFailed,
    AuthenticationFailed,
    TlsTrustFailed,
    SandboxDenied,
}

impl NativeStderrClassification {
    fn observe(&mut self, bytes: &[u8]) {
        let observed = if contains_bytes(bytes, b"Operation not permitted") {
            Self::SandboxDenied
        } else if contains_bytes(bytes, b"UnknownIssuer") {
            Self::TlsTrustFailed
        } else if contains_bytes(bytes, b"HTTP 401")
            || contains_bytes(bytes, b"AuthorizationRequired")
            || contains_bytes(bytes, b"authentication failed")
        {
            Self::AuthenticationFailed
        } else if contains_bytes(bytes, b"required MCP servers failed to initialize") {
            Self::McpInitializationFailed
        } else {
            Self::Unclassified
        };
        *self = (*self).max(observed);
    }
}

fn contains_bytes(value: &[u8], needle: &[u8]) -> bool {
    value.windows(needle.len()).any(|window| window == needle)
}

#[derive(Debug)]
struct NormalizedOutput {
    visible_output: String,
    tool_calls: Vec<VisibleToolCall>,
}

fn native_harness(harness: &HarnessConfig) -> Result<(NativeHarnessKind, &str), DriverError> {
    match harness {
        HarnessConfig::Codex { version } => Ok((NativeHarnessKind::Codex, version)),
        HarnessConfig::ClaudeCode { version } => Ok((NativeHarnessKind::ClaudeCode, version)),
        HarnessConfig::Cline { version } => Ok((NativeHarnessKind::Cline, version)),
        HarnessConfig::Pi { version } => Ok((NativeHarnessKind::Pi, version)),
        HarnessConfig::GenericCommand { .. } => Err(DriverError::UnsupportedHarness),
    }
}

fn bound_generic_process_spec(
    manifest: &ValidatedAgentManifest,
    runtime: &HarnessRuntimeConfig,
) -> Result<ProcessSpec, DriverError> {
    let HarnessConfig::GenericCommand {
        executable,
        arguments,
        ..
    } = &manifest.manifest().harness
    else {
        return Err(DriverError::UnsupportedHarness);
    };
    let workspace = path_argument(&runtime.workspace);
    let mcp_config = path_argument(&runtime.mcp_config);
    let state_directory = path_argument(&runtime.state_directory);
    let arguments = arguments
        .iter()
        .map(|argument| match argument.as_str() {
            "{workspace}" => workspace.clone(),
            "{mcp_config}" => mcp_config.clone(),
            "{state_directory}" => state_directory.clone(),
            _ => argument.clone(),
        })
        .collect();
    Ok(ProcessSpec {
        executable: executable.clone(),
        arguments,
        working_directory: Some(runtime.workspace.clone()),
    })
}

fn validate_run_id(run_id: &str) -> Result<(), DriverError> {
    if run_id.is_empty() || run_id.chars().any(char::is_control) {
        return Err(DriverError::InvalidCheckpoint);
    }
    Ok(())
}

fn validate_native_checkpoint(
    manifest: &ValidatedAgentManifest,
    checkpoint: &NativeHarnessCheckpoint,
    kind: NativeHarnessKind,
) -> Result<(), DriverError> {
    let lifecycle_valid = !checkpoint.telemetry.lifecycle.is_empty()
        && checkpoint
            .telemetry
            .lifecycle
            .iter()
            .enumerate()
            .all(|(index, transition)| transition.sequence == index as u64)
        && checkpoint
            .telemetry
            .lifecycle
            .last()
            .is_some_and(|transition| transition.state == checkpoint.state);
    let diagnostics_valid = checkpoint
        .telemetry
        .diagnostics
        .iter()
        .enumerate()
        .all(|(index, diagnostic)| diagnostic.sequence == index as u64);
    if checkpoint.schema_version != DRIVER_CHECKPOINT_SCHEMA_VERSION
        || checkpoint.manifest != *manifest.identity()
        || checkpoint.harness != kind
        || checkpoint.telemetry.schema_version != DRIVER_TELEMETRY_SCHEMA_VERSION
        || checkpoint.telemetry.run_id != checkpoint.run_id
        || checkpoint.telemetry.manifest != checkpoint.manifest
        || !lifecycle_valid
        || !diagnostics_valid
        || matches!(
            checkpoint.state,
            DriverLifecycleState::Starting
                | DriverLifecycleState::TurnRunning { .. }
                | DriverLifecycleState::Terminating
        )
    {
        return Err(DriverError::InvalidCheckpoint);
    }
    validate_run_id(&checkpoint.run_id)
}

fn version_arguments(_kind: NativeHarnessKind) -> Vec<String> {
    vec!["--version".to_owned()]
}

fn parse_reported_version(output: &[u8]) -> Option<Version> {
    String::from_utf8_lossy(output)
        .split_whitespace()
        .find_map(|part| {
            let candidate = part.trim_matches(|character: char| {
                !character.is_ascii_alphanumeric() && character != '.' && character != '-'
            });
            let candidate = candidate.trim_start_matches('v');
            Version::parse(candidate).ok()
        })
}

fn path_argument(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn append_provider(arguments: &mut Vec<String>, flag: &str, source: &ModelSource) {
    let provider = match source {
        ModelSource::HarnessDefault => return,
        ModelSource::Provider { provider } => match provider {
            ModelProvider::OpenAi => "openai",
            ModelProvider::Anthropic => "anthropic",
            ModelProvider::Google => "google",
            ModelProvider::OpenRouter => "openrouter",
        },
        ModelSource::Local { runtime } => match runtime {
            LocalModelRuntime::Ollama => "ollama",
            LocalModelRuntime::LlamaCpp => "llama-cpp",
            LocalModelRuntime::Vllm => "vllm",
        },
    };
    arguments.push(flag.to_owned());
    arguments.push(provider.to_owned());
}

fn synthetic_failure() -> ExitStatus {
    ExitStatus {
        success: false,
        code: None,
        signal: None,
    }
}

fn normalize_output(kind: NativeHarnessKind, bytes: &[u8]) -> Result<NormalizedOutput, ()> {
    let events = json_lines(bytes)?;
    match kind {
        NativeHarnessKind::Codex => normalize_codex(&events),
        NativeHarnessKind::ClaudeCode => normalize_claude(&events),
        NativeHarnessKind::Cline => normalize_cline(&events),
        NativeHarnessKind::Pi => normalize_pi(&events),
    }
}

fn json_lines(bytes: &[u8]) -> Result<Vec<Value>, ()> {
    let text = std::str::from_utf8(bytes).map_err(|_| ())?;
    let mut values = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        values.push(serde_json::from_str(line).map_err(|_| ())?);
    }
    if values.is_empty() {
        return Err(());
    }
    Ok(values)
}

fn normalize_codex(events: &[Value]) -> Result<NormalizedOutput, ()> {
    let mut visible_output = None;
    let mut tool_calls = Vec::new();
    let mut completed = false;
    for event in events {
        match event.get("type").and_then(Value::as_str) {
            Some("item.completed") => {
                let item = event.get("item").and_then(Value::as_object).ok_or(())?;
                match item.get("type").and_then(Value::as_str) {
                    Some("agent_message") => {
                        visible_output =
                            item.get("text").and_then(Value::as_str).map(str::to_owned);
                    }
                    Some("mcp_tool_call") => tool_calls.push(tool_from_object(item)),
                    _ => {}
                }
            }
            Some("turn.completed") => completed = true,
            Some("turn.failed" | "error") => return Err(()),
            _ => {}
        }
    }
    finish_normalization(visible_output, tool_calls, completed)
}

fn normalize_claude(events: &[Value]) -> Result<NormalizedOutput, ()> {
    let mut visible_output = None;
    let mut tool_calls = Vec::new();
    let mut completed = false;
    for event in events {
        match event.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                if let Some(content) = event.pointer("/message/content").and_then(Value::as_array) {
                    let (text, calls) = content_blocks(content);
                    if !text.is_empty() {
                        visible_output = Some(text);
                    }
                    tool_calls.extend(calls);
                }
            }
            Some("result") => {
                if event.get("is_error").and_then(Value::as_bool) == Some(true) {
                    return Err(());
                }
                if let Some(result) = event.get("result").and_then(Value::as_str) {
                    visible_output = Some(result.to_owned());
                }
                completed = true;
            }
            _ => {}
        }
    }
    finish_normalization(visible_output, tool_calls, completed)
}

fn normalize_cline(events: &[Value]) -> Result<NormalizedOutput, ()> {
    let mut visible_output = None;
    let mut tool_calls = Vec::new();
    let mut completed = false;
    for event in events {
        if event.get("partial").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let category = event.get("type").and_then(Value::as_str);
        let subtype = event
            .get("say")
            .or_else(|| event.get("ask"))
            .and_then(Value::as_str);
        if category == Some("say") && subtype == Some("completion_result") {
            visible_output = event.get("text").and_then(Value::as_str).map(str::to_owned);
            completed = true;
        } else if matches!(subtype, Some("use_mcp_server" | "mcp_server_use")) {
            tool_calls.push(VisibleToolCall {
                tool: "cline.mcp".to_owned(),
                arguments: event.get("text").cloned().unwrap_or(Value::Null),
                result: event.get("result").cloned().unwrap_or(Value::Null),
            });
        }
    }
    finish_normalization(visible_output, tool_calls, completed)
}

fn normalize_pi(events: &[Value]) -> Result<NormalizedOutput, ()> {
    let mut visible_output = None;
    let mut tool_calls = Vec::new();
    let mut completed = false;
    for event in events {
        match event.get("type").and_then(Value::as_str) {
            Some("message_end") => {
                let message = event.get("message").and_then(Value::as_object).ok_or(())?;
                if message.get("role").and_then(Value::as_str) == Some("assistant")
                    && let Some(content) = message.get("content").and_then(Value::as_array)
                {
                    let (text, calls) = content_blocks(content);
                    if !text.is_empty() {
                        visible_output = Some(text);
                    }
                    tool_calls.extend(calls);
                }
            }
            Some("agent_end") => completed = true,
            _ => {}
        }
    }
    finish_normalization(visible_output, tool_calls, completed)
}

fn content_blocks(content: &[Value]) -> (String, Vec<VisibleToolCall>) {
    let mut text = Vec::new();
    let mut calls = Vec::new();
    for block in content {
        let Some(object) = block.as_object() else {
            continue;
        };
        match object.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(value) = object.get("text").and_then(Value::as_str) {
                    text.push(value);
                }
            }
            Some("tool_use" | "toolCall" | "tool_call") => calls.push(tool_from_object(object)),
            _ => {}
        }
    }
    (text.join("\n"), calls)
}

fn tool_from_object(object: &Map<String, Value>) -> VisibleToolCall {
    let tool = ["name", "tool", "tool_name"]
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
        .unwrap_or("unknown_visible_tool")
        .to_owned();
    let arguments = ["input", "arguments", "args"]
        .iter()
        .find_map(|key| object.get(*key))
        .cloned()
        .unwrap_or(Value::Null);
    let result = ["result", "output"]
        .iter()
        .find_map(|key| object.get(*key))
        .cloned()
        .unwrap_or(Value::Null);
    VisibleToolCall {
        tool,
        arguments,
        result,
    }
}

fn finish_normalization(
    visible_output: Option<String>,
    tool_calls: Vec<VisibleToolCall>,
    completed: bool,
) -> Result<NormalizedOutput, ()> {
    let visible_output = visible_output.filter(|output| !output.trim().is_empty());
    if !completed {
        return Err(());
    }
    Ok(NormalizedOutput {
        visible_output: visible_output.unwrap_or_default(),
        tool_calls,
    })
}

#[cfg(test)]
mod tests {
    use super::NativeStderrClassification;

    #[test]
    fn classifies_private_native_failures_without_retaining_their_text() {
        let mut classification = NativeStderrClassification::default();
        classification.observe(b"required MCP servers failed to initialize");
        assert_eq!(
            classification,
            NativeStderrClassification::McpInitializationFailed
        );

        classification.observe(b"required MCP server returned HTTP 401");
        assert_eq!(
            classification,
            NativeStderrClassification::AuthenticationFailed
        );

        classification.observe(b"invalid peer certificate: UnknownIssuer");
        assert_eq!(classification, NativeStderrClassification::TlsTrustFailed);

        classification.observe(b"Error: Operation not permitted (os error 1)");
        assert_eq!(classification, NativeStderrClassification::SandboxDenied);
    }
}
