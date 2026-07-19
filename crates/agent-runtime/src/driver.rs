use std::{
    fmt,
    future::Future,
    path::PathBuf,
    pin::Pin,
    process::Stdio,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
};
use tokio_util::sync::CancellationToken;

use crate::{AgentManifestIdentity, HarnessConfig, ValidatedAgentManifest};

pub const DRIVER_CHECKPOINT_SCHEMA_VERSION: u32 = 1;
pub const DRIVER_TELEMETRY_SCHEMA_VERSION: u32 = 1;
pub const TURN_PROTOCOL_SCHEMA_VERSION: u32 = 1;
const MAX_FRAME_BYTES: usize = 1_048_576;

pub type DriverFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Monotonic-enough injected wall clock used only for attributable timestamps.
pub trait DriverClock: fmt::Debug + Send + Sync {
    fn now_unix_ms(&self) -> i64;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemDriverClock;

impl DriverClock for SystemDriverClock {
    fn now_unix_ms(&self) -> i64 {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        i64::try_from(millis).unwrap_or(i64::MAX)
    }
}

/// Direct-exec process input. It contains no shell string or inherited
/// environment.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessSpec {
    pub executable: String,
    pub arguments: Vec<String>,
    pub working_directory: Option<PathBuf>,
}

impl fmt::Debug for ProcessSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProcessSpec")
            .field("executable", &self.executable)
            .field("argument_count", &self.arguments.len())
            .field(
                "working_directory",
                &self.working_directory.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ProcessHandle(pub String);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExitStatus {
    pub success: bool,
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProcessEvent {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Exited(ExitStatus),
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ProcessError {
    #[error("process could not be spawned")]
    Spawn,
    #[error("process input could not be written")]
    Write,
    #[error("process output could not be read")]
    Read,
    #[error("process could not be terminated")]
    Terminate,
    #[error("process cannot be reattached by this adapter")]
    ReattachUnsupported,
    #[error("process handle no longer exists")]
    Missing,
}

pub trait ProcessInstance: fmt::Debug + Send {
    fn handle(&self) -> ProcessHandle;
    fn write<'a>(&'a mut self, bytes: &'a [u8]) -> DriverFuture<'a, Result<(), ProcessError>>;
    fn next_event(&mut self) -> DriverFuture<'_, Result<ProcessEvent, ProcessError>>;
    fn terminate(&mut self) -> DriverFuture<'_, Result<ExitStatus, ProcessError>>;
}

pub trait ProcessAdapter: fmt::Debug + Send + Sync {
    fn spawn<'a>(
        &'a self,
        spec: &'a ProcessSpec,
    ) -> DriverFuture<'a, Result<Box<dyn ProcessInstance>, ProcessError>>;

    fn reattach<'a>(
        &'a self,
        handle: &'a ProcessHandle,
    ) -> DriverFuture<'a, Result<Box<dyn ProcessInstance>, ProcessError>>;
}

/// Local Tokio direct-process adapter used by the generic command driver.
#[derive(Clone, Copy, Debug, Default)]
pub struct TokioProcessAdapter;

impl ProcessAdapter for TokioProcessAdapter {
    fn spawn<'a>(
        &'a self,
        spec: &'a ProcessSpec,
    ) -> DriverFuture<'a, Result<Box<dyn ProcessInstance>, ProcessError>> {
        Box::pin(spawn_tokio_process(spec, &[]))
    }

    fn reattach<'a>(
        &'a self,
        _handle: &'a ProcessHandle,
    ) -> DriverFuture<'a, Result<Box<dyn ProcessInstance>, ProcessError>> {
        Box::pin(async { Err(ProcessError::ReattachUnsupported) })
    }
}

pub(crate) async fn spawn_tokio_process(
    spec: &ProcessSpec,
    environment: &[(String, String)],
) -> Result<Box<dyn ProcessInstance>, ProcessError> {
    let mut command = Command::new(&spec.executable);
    command
        .args(&spec.arguments)
        .env_clear()
        .envs(environment.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(directory) = &spec.working_directory {
        command.current_dir(directory);
    }
    let mut child = command.spawn().map_err(|_| ProcessError::Spawn)?;
    let stdin = child.stdin.take().ok_or(ProcessError::Spawn)?;
    let stdout = child.stdout.take().ok_or(ProcessError::Spawn)?;
    let stderr = child.stderr.take().ok_or(ProcessError::Spawn)?;
    Ok(Box::new(TokioProcess {
        child,
        stdin,
        stdout,
        stderr,
        stdout_closed: false,
        stderr_closed: false,
    }))
}

struct TokioProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    stderr: ChildStderr,
    stdout_closed: bool,
    stderr_closed: bool,
}

impl fmt::Debug for TokioProcess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokioProcess")
            .field("id", &self.child.id())
            .field("stdout_closed", &self.stdout_closed)
            .field("stderr_closed", &self.stderr_closed)
            .finish_non_exhaustive()
    }
}

impl ProcessInstance for TokioProcess {
    fn handle(&self) -> ProcessHandle {
        ProcessHandle(
            self.child
                .id()
                .map_or_else(|| "exited".to_owned(), |id| format!("pid:{id}")),
        )
    }

    fn write<'a>(&'a mut self, bytes: &'a [u8]) -> DriverFuture<'a, Result<(), ProcessError>> {
        Box::pin(async move {
            self.stdin
                .write_all(bytes)
                .await
                .map_err(|_| ProcessError::Write)?;
            self.stdin.flush().await.map_err(|_| ProcessError::Write)
        })
    }

    fn next_event(&mut self) -> DriverFuture<'_, Result<ProcessEvent, ProcessError>> {
        Box::pin(async move {
            loop {
                let mut stdout = [0_u8; 4096];
                let mut stderr = [0_u8; 4096];
                tokio::select! {
                    biased;
                    read = self.stdout.read(&mut stdout), if !self.stdout_closed => {
                        let count = read.map_err(|_| ProcessError::Read)?;
                        if count == 0 {
                            self.stdout_closed = true;
                        } else {
                            return Ok(ProcessEvent::Stdout(stdout[..count].to_vec()));
                        }
                    }
                    read = self.stderr.read(&mut stderr), if !self.stderr_closed => {
                        let count = read.map_err(|_| ProcessError::Read)?;
                        if count == 0 {
                            self.stderr_closed = true;
                        } else {
                            return Ok(ProcessEvent::Stderr(stderr[..count].to_vec()));
                        }
                    }
                    status = self.child.wait() => {
                        let status = status.map_err(|_| ProcessError::Read)?;
                        return Ok(ProcessEvent::Exited(exit_status(status)));
                    }
                }
            }
        })
    }

    fn terminate(&mut self) -> DriverFuture<'_, Result<ExitStatus, ProcessError>> {
        Box::pin(async move {
            self.child
                .start_kill()
                .map_err(|_| ProcessError::Terminate)?;
            self.child
                .wait()
                .await
                .map(exit_status)
                .map_err(|_| ProcessError::Terminate)
        })
    }
}

fn exit_status(status: std::process::ExitStatus) -> ExitStatus {
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        status.signal()
    };
    #[cfg(not(unix))]
    let signal = None;
    ExitStatus {
        success: status.success(),
        code: status.code(),
        signal,
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum DriverLifecycleState {
    Pending,
    Starting,
    Ready,
    TurnRunning { turn_id: String },
    Terminating,
    Terminated { reason: TerminationReason },
    Crashed { exit: ExitStatus },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminationReason {
    Completed,
    Cancelled,
    GameEnded,
    Operator,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticStream {
    Driver,
    Stderr,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticRecord {
    pub sequence: u64,
    pub at_unix_ms: i64,
    pub stream: DiagnosticStream,
    pub code: String,
    pub visible_text: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VisibleToolCall {
    pub tool: String,
    #[serde(default)]
    pub arguments: Value,
    #[serde(default)]
    pub result: Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VisibleTurnOutput {
    pub schema_version: u32,
    pub turn_id: String,
    pub visible_output: String,
    #[serde(default)]
    pub tool_calls: Vec<VisibleToolCall>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TurnRequest {
    pub turn_id: String,
    pub visible_input: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TurnTelemetry {
    pub turn_id: String,
    pub started_at_unix_ms: i64,
    pub completed_at_unix_ms: i64,
    pub visible_input: String,
    pub visible_output: String,
    pub tool_calls: Vec<VisibleToolCall>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleTransition {
    pub sequence: u64,
    pub at_unix_ms: i64,
    pub state: DriverLifecycleState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DriverTelemetry {
    pub schema_version: u32,
    pub run_id: String,
    pub manifest: AgentManifestIdentity,
    pub restarts: u32,
    pub lifecycle: Vec<LifecycleTransition>,
    pub turns: Vec<TurnTelemetry>,
    pub diagnostics: Vec<DiagnosticRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DriverCheckpoint {
    pub schema_version: u32,
    pub run_id: String,
    pub manifest: AgentManifestIdentity,
    pub state: DriverLifecycleState,
    pub process: Option<ProcessHandle>,
    pub process_spec: ProcessSpec,
    pub telemetry: DriverTelemetry,
}

#[derive(Debug, Error)]
pub enum DriverError {
    #[error("agent manifest is not a generic-command manifest")]
    UnsupportedHarness,
    #[error("driver lifecycle does not allow {operation} from {state:?}")]
    InvalidTransition {
        operation: &'static str,
        state: DriverLifecycleState,
    },
    #[error("driver operation was cancelled")]
    Cancelled,
    #[error("process adapter failed: {0}")]
    Process(#[from] ProcessError),
    #[error("agent process exited before returning a turn: {0:?}")]
    UnexpectedExit(ExitStatus),
    #[error("agent stdout frame is invalid or contains unsupported fields")]
    InvalidFrame,
    #[error("agent stdout frame exceeds the configured bound")]
    FrameTooLarge,
    #[error("agent returned a response for a different turn")]
    TurnMismatch,
    #[error("driver checkpoint is corrupt or incompatible")]
    InvalidCheckpoint,
    #[error("driver protocol serialization failed")]
    Serialization,
    #[error("{harness} executable is unavailable: {executable}")]
    HarnessUnavailable {
        harness: &'static str,
        executable: String,
    },
    #[error("{harness} did not report a semantic version")]
    HarnessVersionUnparseable { harness: &'static str },
    #[error("{harness} version {installed} is below supported minimum {minimum}")]
    HarnessVersionUnsupported {
        harness: &'static str,
        installed: String,
        minimum: &'static str,
    },
    #[error("{harness} version mismatch: manifest requires {expected}, installed {installed}")]
    HarnessVersionMismatch {
        harness: &'static str,
        expected: String,
        installed: String,
    },
    #[error("{harness} exited before producing a successful turn: {exit:?}")]
    HarnessExit {
        harness: &'static str,
        exit: ExitStatus,
    },
    #[error("{harness} structured output is invalid or incomplete")]
    HarnessOutput { harness: &'static str },
    #[error("trusted harness runtime paths or executable overrides are invalid")]
    InvalidHarnessRuntime,
}

pub trait AgentDriver {
    fn state(&self) -> &DriverLifecycleState;
    fn telemetry(&self) -> &DriverTelemetry;
    fn start<'a>(
        &'a mut self,
        cancel: &'a CancellationToken,
    ) -> DriverFuture<'a, Result<(), DriverError>>;
    fn request_turn<'a>(
        &'a mut self,
        request: TurnRequest,
        cancel: &'a CancellationToken,
    ) -> DriverFuture<'a, Result<VisibleTurnOutput, DriverError>>;
    fn resume<'a>(
        &'a mut self,
        cancel: &'a CancellationToken,
    ) -> DriverFuture<'a, Result<(), DriverError>>;
    fn terminate(&mut self, reason: TerminationReason)
    -> DriverFuture<'_, Result<(), DriverError>>;
}

pub struct GenericCommandDriver {
    adapter: Arc<dyn ProcessAdapter>,
    clock: Arc<dyn DriverClock>,
    state: DriverLifecycleState,
    manifest: AgentManifestIdentity,
    process: Option<Box<dyn ProcessInstance>>,
    resume_handle: Option<ProcessHandle>,
    process_spec: ProcessSpec,
    run_id: String,
    telemetry: DriverTelemetry,
    decoder: JsonLineDecoder,
}

impl fmt::Debug for GenericCommandDriver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GenericCommandDriver")
            .field("state", &self.state)
            .field("manifest", &self.manifest)
            .field("process_spec", &self.process_spec)
            .field("run_id", &self.run_id)
            .finish_non_exhaustive()
    }
}

impl GenericCommandDriver {
    /// Creates a pending generic-command driver from an already validated
    /// immutable manifest.
    ///
    /// # Errors
    ///
    /// Returns an error when the manifest is for another harness or the run ID
    /// is empty or contains control characters.
    pub fn new(
        run_id: impl Into<String>,
        manifest: &ValidatedAgentManifest,
        working_directory: Option<PathBuf>,
        adapter: Arc<dyn ProcessAdapter>,
        clock: Arc<dyn DriverClock>,
    ) -> Result<Self, DriverError> {
        let HarnessConfig::GenericCommand {
            executable,
            arguments,
            ..
        } = &manifest.manifest().harness
        else {
            return Err(DriverError::UnsupportedHarness);
        };
        let run_id = run_id.into();
        if run_id.is_empty() || run_id.chars().any(char::is_control) {
            return Err(DriverError::InvalidCheckpoint);
        }
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
            process: None,
            resume_handle: None,
            process_spec: ProcessSpec {
                executable: executable.clone(),
                arguments: arguments.clone(),
                working_directory,
            },
            run_id,
            telemetry,
            decoder: JsonLineDecoder::default(),
        })
    }

    pub(crate) fn new_with_process_spec(
        run_id: impl Into<String>,
        manifest: &ValidatedAgentManifest,
        process_spec: ProcessSpec,
        adapter: Arc<dyn ProcessAdapter>,
        clock: Arc<dyn DriverClock>,
    ) -> Result<Self, DriverError> {
        let mut driver = Self::new(
            run_id,
            manifest,
            process_spec.working_directory.clone(),
            adapter,
            clock,
        )?;
        driver.process_spec = process_spec;
        Ok(driver)
    }

    /// Reconstructs a stopped in-memory driver from a durable checkpoint.
    ///
    /// # Errors
    ///
    /// Returns an error when any manifest, command, lifecycle, process, or
    /// telemetry identity in the checkpoint is inconsistent.
    pub fn restore(
        manifest: &ValidatedAgentManifest,
        checkpoint: DriverCheckpoint,
        adapter: Arc<dyn ProcessAdapter>,
        clock: Arc<dyn DriverClock>,
    ) -> Result<Self, DriverError> {
        let HarnessConfig::GenericCommand {
            executable,
            arguments,
            ..
        } = &manifest.manifest().harness
        else {
            return Err(DriverError::UnsupportedHarness);
        };
        let expected_process_spec = ProcessSpec {
            executable: executable.clone(),
            arguments: arguments.clone(),
            working_directory: checkpoint.process_spec.working_directory.clone(),
        };
        Self::restore_with_process_spec(
            manifest,
            checkpoint,
            &expected_process_spec,
            adapter,
            clock,
        )
    }

    pub(crate) fn restore_with_process_spec(
        manifest: &ValidatedAgentManifest,
        checkpoint: DriverCheckpoint,
        expected_process_spec: &ProcessSpec,
        adapter: Arc<dyn ProcessAdapter>,
        clock: Arc<dyn DriverClock>,
    ) -> Result<Self, DriverError> {
        if !matches!(
            manifest.manifest().harness,
            HarnessConfig::GenericCommand { .. }
        ) {
            return Err(DriverError::UnsupportedHarness);
        }
        let lifecycle_is_valid = !checkpoint.telemetry.lifecycle.is_empty()
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
        let diagnostics_are_valid = checkpoint
            .telemetry
            .diagnostics
            .iter()
            .enumerate()
            .all(|(index, diagnostic)| diagnostic.sequence == index as u64);
        if checkpoint.schema_version != DRIVER_CHECKPOINT_SCHEMA_VERSION
            || checkpoint.manifest != *manifest.identity()
            || checkpoint.telemetry.schema_version != DRIVER_TELEMETRY_SCHEMA_VERSION
            || checkpoint.telemetry.run_id != checkpoint.run_id
            || checkpoint.telemetry.manifest != checkpoint.manifest
            || checkpoint.run_id.is_empty()
            || checkpoint.run_id.chars().any(char::is_control)
            || checkpoint.process_spec != *expected_process_spec
            || !lifecycle_is_valid
            || !diagnostics_are_valid
            || matches!(
                checkpoint.state,
                DriverLifecycleState::Starting
                    | DriverLifecycleState::TurnRunning { .. }
                    | DriverLifecycleState::Terminating
            )
        {
            return Err(DriverError::InvalidCheckpoint);
        }
        let resume_handle = checkpoint.process;
        let process_required = matches!(checkpoint.state, DriverLifecycleState::Ready);
        if process_required != resume_handle.is_some() {
            return Err(DriverError::InvalidCheckpoint);
        }
        Ok(Self {
            adapter,
            clock,
            state: checkpoint.state,
            manifest: checkpoint.manifest,
            process: None,
            resume_handle,
            process_spec: checkpoint.process_spec,
            run_id: checkpoint.run_id,
            telemetry: checkpoint.telemetry,
            decoder: JsonLineDecoder::default(),
        })
    }

    /// Captures lifecycle and visible telemetry at a stable process boundary.
    ///
    /// # Errors
    ///
    /// Returns an error while a start, turn, or termination transition is in
    /// progress because those states cannot be reconstructed safely.
    pub fn checkpoint(&self) -> Result<DriverCheckpoint, DriverError> {
        if matches!(
            self.state,
            DriverLifecycleState::Starting
                | DriverLifecycleState::TurnRunning { .. }
                | DriverLifecycleState::Terminating
        ) {
            return Err(DriverError::InvalidCheckpoint);
        }
        Ok(DriverCheckpoint {
            schema_version: DRIVER_CHECKPOINT_SCHEMA_VERSION,
            run_id: self.run_id.clone(),
            manifest: self.manifest.clone(),
            state: self.state.clone(),
            process: self
                .process
                .as_ref()
                .map(|process| process.handle())
                .or_else(|| self.resume_handle.clone()),
            process_spec: self.process_spec.clone(),
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

    fn diagnostic(&mut self, stream: DiagnosticStream, code: &str, visible_text: String) {
        self.telemetry.diagnostics.push(DiagnosticRecord {
            sequence: self.telemetry.diagnostics.len() as u64,
            at_unix_ms: self.clock.now_unix_ms(),
            stream,
            code: code.to_owned(),
            visible_text,
        });
    }

    async fn cancellation_error(&mut self) -> DriverError {
        if let Some(process) = self.process.as_mut()
            && let Err(error) = process.terminate().await
        {
            self.diagnostic(
                DiagnosticStream::Driver,
                "cancellation_termination_failed",
                error.to_string(),
            );
        }
        self.process = None;
        self.resume_handle = None;
        self.decoder.clear();
        self.transition(DriverLifecycleState::Terminated {
            reason: TerminationReason::Cancelled,
        });
        DriverError::Cancelled
    }

    async fn protocol_failure(&mut self, error: DriverError) -> DriverError {
        if let Some(process) = self.process.as_mut()
            && let Err(termination_error) = process.terminate().await
        {
            self.diagnostic(
                DiagnosticStream::Driver,
                "protocol_termination_failed",
                termination_error.to_string(),
            );
        }
        self.process = None;
        self.resume_handle = None;
        self.decoder.clear();
        let exit = ExitStatus {
            success: false,
            code: None,
            signal: None,
        };
        self.transition(DriverLifecycleState::Crashed { exit });
        self.diagnostic(
            DiagnosticStream::Driver,
            "protocol_failure",
            error.to_string(),
        );
        error
    }

    fn encode_turn_request(&self, request: &TurnRequest) -> Result<Vec<u8>, DriverError> {
        if request.turn_id.is_empty() || request.turn_id.chars().any(char::is_control) {
            return Err(DriverError::InvalidFrame);
        }
        let wire = TurnInputFrame {
            schema_version: TURN_PROTOCOL_SCHEMA_VERSION,
            kind: "turn",
            run_id: &self.run_id,
            turn_id: &request.turn_id,
            visible_input: &request.visible_input,
        };
        let mut bytes = serde_json::to_vec(&wire).map_err(|_| DriverError::Serialization)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    async fn reattach_or_restart(
        &mut self,
        handle: &ProcessHandle,
        cancel: &CancellationToken,
    ) -> Result<Box<dyn ProcessInstance>, DriverError> {
        let reattached = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(self.cancellation_error().await),
            result = self.adapter.reattach(handle) => result,
        };
        match reattached {
            Ok(process) => Ok(process),
            Err(ProcessError::ReattachUnsupported | ProcessError::Missing) => {
                self.telemetry.restarts = self.telemetry.restarts.saturating_add(1);
                self.diagnostic(
                    DiagnosticStream::Driver,
                    "process_restart_after_restore",
                    "saved process could not be reattached; starting a replacement".to_owned(),
                );
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => Err(self.cancellation_error().await),
                    result = self.adapter.spawn(&self.process_spec) => result.map_err(DriverError::Process),
                }
            }
            Err(error) => Err(DriverError::Process(error)),
        }
    }
}

impl AgentDriver for GenericCommandDriver {
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
            if cancel.is_cancelled() {
                return Err(self.cancellation_error().await);
            }
            self.transition(DriverLifecycleState::Starting);
            let spawned = tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(self.cancellation_error().await),
                result = self.adapter.spawn(&self.process_spec) => result,
            };
            match spawned {
                Ok(process) => {
                    self.process = Some(process);
                    self.resume_handle = None;
                    self.transition(DriverLifecycleState::Ready);
                    Ok(())
                }
                Err(error) => {
                    let exit = ExitStatus {
                        success: false,
                        code: None,
                        signal: None,
                    };
                    self.transition(DriverLifecycleState::Crashed { exit });
                    Err(DriverError::Process(error))
                }
            }
        })
    }

    fn request_turn<'a>(
        &'a mut self,
        request: TurnRequest,
        cancel: &'a CancellationToken,
    ) -> DriverFuture<'a, Result<VisibleTurnOutput, DriverError>> {
        Box::pin(async move {
            if self.state != DriverLifecycleState::Ready || self.process.is_none() {
                return Err(DriverError::InvalidTransition {
                    operation: "request_turn",
                    state: self.state.clone(),
                });
            }
            let bytes = self.encode_turn_request(&request)?;
            if cancel.is_cancelled() {
                return Err(self.cancellation_error().await);
            }
            let started = self.clock.now_unix_ms();
            self.transition(DriverLifecycleState::TurnRunning {
                turn_id: request.turn_id.clone(),
            });
            let write = {
                let process = self
                    .process
                    .as_mut()
                    .ok_or(DriverError::InvalidCheckpoint)?;
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => return Err(self.cancellation_error().await),
                    result = process.write(&bytes) => result,
                }
            };
            if let Err(error) = write {
                return Err(self.protocol_failure(DriverError::Process(error)).await);
            }
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
                };
                match event {
                    Err(error) => {
                        return Err(self.protocol_failure(DriverError::Process(error)).await);
                    }
                    Ok(ProcessEvent::Stderr(bytes)) => self.diagnostic(
                        DiagnosticStream::Stderr,
                        "process_stderr",
                        String::from_utf8_lossy(&bytes).into_owned(),
                    ),
                    Ok(ProcessEvent::Exited(exit)) => {
                        self.process = None;
                        self.resume_handle = None;
                        self.decoder.clear();
                        self.transition(DriverLifecycleState::Crashed { exit: exit.clone() });
                        return Err(DriverError::UnexpectedExit(exit));
                    }
                    Ok(ProcessEvent::Stdout(bytes)) => {
                        let frames = match self.decoder.push(&bytes) {
                            Ok(frames) => frames,
                            Err(error) => return Err(self.protocol_failure(error).await),
                        };
                        if frames.len() > 1 {
                            return Err(self.protocol_failure(DriverError::InvalidFrame).await);
                        }
                        let Some(frame) = frames.into_iter().next() else {
                            continue;
                        };
                        if !self.decoder.is_empty() {
                            return Err(self.protocol_failure(DriverError::InvalidFrame).await);
                        }
                        let output: VisibleTurnOutput = match serde_json::from_slice(&frame) {
                            Ok(output) => output,
                            Err(_) => {
                                return Err(self.protocol_failure(DriverError::InvalidFrame).await);
                            }
                        };
                        if output.schema_version != TURN_PROTOCOL_SCHEMA_VERSION {
                            return Err(self.protocol_failure(DriverError::InvalidFrame).await);
                        }
                        if output.turn_id != request.turn_id {
                            return Err(self.protocol_failure(DriverError::TurnMismatch).await);
                        }
                        self.telemetry.turns.push(TurnTelemetry {
                            turn_id: request.turn_id,
                            started_at_unix_ms: started,
                            completed_at_unix_ms: self.clock.now_unix_ms(),
                            visible_input: request.visible_input,
                            visible_output: output.visible_output.clone(),
                            tool_calls: output.tool_calls.clone(),
                        });
                        self.transition(DriverLifecycleState::Ready);
                        return Ok(output);
                    }
                }
            }
        })
    }

    fn resume<'a>(
        &'a mut self,
        cancel: &'a CancellationToken,
    ) -> DriverFuture<'a, Result<(), DriverError>> {
        Box::pin(async move {
            if self.process.is_some() {
                return Ok(());
            }
            if cancel.is_cancelled() {
                return Err(self.cancellation_error().await);
            }
            let previous = self.state.clone();
            let result = match previous {
                DriverLifecycleState::Ready => {
                    let handle = self
                        .resume_handle
                        .clone()
                        .ok_or(DriverError::InvalidCheckpoint)?;
                    return match self.reattach_or_restart(&handle, cancel).await {
                        Ok(process) => {
                            self.process = Some(process);
                            self.resume_handle = None;
                            self.transition(DriverLifecycleState::Ready);
                            Ok(())
                        }
                        Err(error) => Err(error),
                    };
                }
                DriverLifecycleState::Crashed { .. } => {
                    self.telemetry.restarts = self.telemetry.restarts.saturating_add(1);
                    tokio::select! {
                        biased;
                        () = cancel.cancelled() => return Err(self.cancellation_error().await),
                        result = self.adapter.spawn(&self.process_spec) => result,
                    }
                }
                _ => {
                    return Err(DriverError::InvalidTransition {
                        operation: "resume",
                        state: previous,
                    });
                }
            };
            let process = result.map_err(DriverError::Process)?;
            self.process = Some(process);
            self.resume_handle = None;
            self.transition(DriverLifecycleState::Ready);
            Ok(())
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
                    DiagnosticStream::Driver,
                    "termination_failed",
                    error.to_string(),
                );
                self.transition(previous);
                return Err(DriverError::Process(error));
            }
            self.process = None;
            self.resume_handle = None;
            self.decoder.clear();
            self.transition(DriverLifecycleState::Terminated { reason });
            Ok(())
        })
    }
}

#[derive(Serialize)]
struct TurnInputFrame<'a> {
    schema_version: u32,
    #[serde(rename = "type")]
    kind: &'static str,
    run_id: &'a str,
    turn_id: &'a str,
    visible_input: &'a str,
}

#[derive(Debug, Default)]
struct JsonLineDecoder {
    buffer: Vec<u8>,
}

impl JsonLineDecoder {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, DriverError> {
        if self.buffer.len().saturating_add(bytes.len()) > MAX_FRAME_BYTES {
            return Err(DriverError::FrameTooLarge);
        }
        self.buffer.extend_from_slice(bytes);
        let mut frames = Vec::new();
        while let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let mut frame = self.buffer.drain(..=newline).collect::<Vec<_>>();
            frame.pop();
            if frame.last() == Some(&b'\r') {
                frame.pop();
            }
            if frame.is_empty() {
                return Err(DriverError::InvalidFrame);
            }
            frames.push(frame);
        }
        Ok(frames)
    }

    fn clear(&mut self) {
        self.buffer.clear();
    }

    fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}
