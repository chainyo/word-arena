use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, AtomicUsize, Ordering},
    },
};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use word_arena_agent_runtime::{
    AgentDriver, DRIVER_CHECKPOINT_SCHEMA_VERSION, DRIVER_TELEMETRY_SCHEMA_VERSION, DriverClock,
    DriverError, DriverFuture, DriverLifecycleState, ExitStatus, GenericCommandDriver,
    ProcessAdapter, ProcessError, ProcessEvent, ProcessHandle, ProcessInstance, ProcessSpec,
    TURN_PROTOCOL_SCHEMA_VERSION, TerminationReason, TokioProcessAdapter, TurnRequest,
    ValidatedAgentManifest,
};

#[derive(Debug)]
struct StepClock(AtomicI64);

impl StepClock {
    const fn new(start: i64) -> Self {
        Self(AtomicI64::new(start))
    }
}

impl DriverClock for StepClock {
    fn now_unix_ms(&self) -> i64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
}

#[derive(Debug, Default)]
struct FakeAdapter {
    state: Mutex<FakeAdapterState>,
}

#[derive(Debug, Default)]
struct FakeAdapterState {
    scripts: VecDeque<VecDeque<Result<ProcessEvent, ProcessError>>>,
    processes: HashMap<ProcessHandle, Arc<FakeProcessState>>,
    spawn_calls: usize,
}

#[derive(Debug, Default)]
struct FakeProcessState {
    events: Mutex<VecDeque<Result<ProcessEvent, ProcessError>>>,
    writes: Mutex<Vec<Vec<u8>>>,
    terminations: AtomicUsize,
}

#[derive(Debug)]
struct FakeProcess {
    handle: ProcessHandle,
    state: Arc<FakeProcessState>,
}

impl FakeAdapter {
    fn with_scripts(scripts: Vec<Vec<Result<ProcessEvent, ProcessError>>>) -> Self {
        Self {
            state: Mutex::new(FakeAdapterState {
                scripts: scripts
                    .into_iter()
                    .map(VecDeque::from)
                    .collect::<VecDeque<_>>(),
                ..FakeAdapterState::default()
            }),
        }
    }

    fn spawn_calls(&self) -> usize {
        self.state.lock().unwrap().spawn_calls
    }

    fn process(&self, number: usize) -> Arc<FakeProcessState> {
        self.state
            .lock()
            .unwrap()
            .processes
            .get(&ProcessHandle(format!("fake:{number}")))
            .unwrap()
            .clone()
    }
}

impl ProcessAdapter for FakeAdapter {
    fn spawn<'a>(
        &'a self,
        _spec: &'a ProcessSpec,
    ) -> DriverFuture<'a, Result<Box<dyn ProcessInstance>, ProcessError>> {
        Box::pin(async move {
            let mut state = self.state.lock().unwrap();
            state.spawn_calls += 1;
            let number = state.spawn_calls;
            let events = state.scripts.pop_front().ok_or(ProcessError::Spawn)?;
            let handle = ProcessHandle(format!("fake:{number}"));
            let process_state = Arc::new(FakeProcessState {
                events: Mutex::new(events),
                ..FakeProcessState::default()
            });
            state
                .processes
                .insert(handle.clone(), process_state.clone());
            Ok(Box::new(FakeProcess {
                handle,
                state: process_state,
            }) as Box<dyn ProcessInstance>)
        })
    }

    fn reattach<'a>(
        &'a self,
        handle: &'a ProcessHandle,
    ) -> DriverFuture<'a, Result<Box<dyn ProcessInstance>, ProcessError>> {
        Box::pin(async move {
            let state = self
                .state
                .lock()
                .unwrap()
                .processes
                .get(handle)
                .cloned()
                .ok_or(ProcessError::Missing)?;
            Ok(Box::new(FakeProcess {
                handle: handle.clone(),
                state,
            }) as Box<dyn ProcessInstance>)
        })
    }
}

impl ProcessInstance for FakeProcess {
    fn handle(&self) -> ProcessHandle {
        self.handle.clone()
    }

    fn write<'a>(&'a mut self, bytes: &'a [u8]) -> DriverFuture<'a, Result<(), ProcessError>> {
        Box::pin(async move {
            self.state.writes.lock().unwrap().push(bytes.to_vec());
            Ok(())
        })
    }

    fn close_input(&mut self) -> DriverFuture<'_, Result<(), ProcessError>> {
        Box::pin(async { Ok(()) })
    }

    fn next_event(&mut self) -> DriverFuture<'_, Result<ProcessEvent, ProcessError>> {
        Box::pin(async move {
            self.state
                .events
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Err(ProcessError::Read))
        })
    }

    fn terminate(&mut self) -> DriverFuture<'_, Result<ExitStatus, ProcessError>> {
        Box::pin(async move {
            self.state.terminations.fetch_add(1, Ordering::SeqCst);
            Ok(ExitStatus {
                success: false,
                code: None,
                signal: Some(15),
            })
        })
    }
}

fn manifest() -> ValidatedAgentManifest {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/agents/generic-command-v1.json");
    ValidatedAgentManifest::from_json(&fs::read(path).unwrap()).unwrap()
}

fn output(turn_id: &str, visible_output: &str) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&json!({
        "schema_version": 1,
        "turn_id": turn_id,
        "visible_output": visible_output,
        "tool_calls": [{
            "tool": "word_arena.place_tiles",
            "arguments": {"row": 7, "column": 7, "tiles": "ETE"},
            "result": {"accepted": true}
        }]
    }))
    .unwrap();
    bytes.push(b'\n');
    bytes
}

fn request(turn_id: &str) -> TurnRequest {
    TurnRequest {
        turn_id: turn_id.to_owned(),
        visible_input: format!("Play {turn_id}"),
    }
}

fn new_driver(adapter: Arc<FakeAdapter>) -> GenericCommandDriver {
    GenericCommandDriver::new(
        "run-synthetic",
        &manifest(),
        Some(PathBuf::from("/isolated/seat-a")),
        adapter,
        Arc::new(StepClock::new(1_000)),
    )
    .unwrap()
}

#[test]
fn published_driver_contract_matches_runtime() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/agent-driver-v1.json");
    let contract: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(
        contract["checkpoint_schema_version"],
        DRIVER_CHECKPOINT_SCHEMA_VERSION
    );
    assert_eq!(
        contract["telemetry_schema_version"],
        DRIVER_TELEMETRY_SCHEMA_VERSION
    );
    assert_eq!(
        contract["turn_protocol_schema_version"],
        TURN_PROTOCOL_SCHEMA_VERSION
    );
    assert_eq!(contract["records_hidden_chain_of_thought"], false);
    assert_eq!(contract["output_frame"]["unknown_fields"], "rejected");
    assert_eq!(
        contract["termination_reasons"],
        serde_json::json!([
            "completed",
            "cancelled",
            "game_ended",
            "operator",
            "budget_exceeded"
        ])
    );
}

#[tokio::test]
async fn synthetic_match_survives_checkpoint_reattach_and_preserves_visible_telemetry() {
    let first = output("turn-1", "Placed ETE for 3 points.");
    let split = first.len() / 2;
    let adapter = Arc::new(FakeAdapter::with_scripts(vec![vec![
        Ok(ProcessEvent::Stdout(first[..split].to_vec())),
        Ok(ProcessEvent::Stderr(
            b"visible fixture diagnostic\n".to_vec(),
        )),
        Ok(ProcessEvent::Stdout(first[split..].to_vec())),
        Ok(ProcessEvent::Stdout(output(
            "turn-2",
            "Passed because the bag is empty.",
        ))),
    ]]));
    let cancel = CancellationToken::new();
    let mut initial = new_driver(adapter.clone());

    initial.start(&cancel).await.unwrap();
    let first_output = initial
        .request_turn(request("turn-1"), &cancel)
        .await
        .unwrap();
    assert_eq!(first_output.visible_output, "Placed ETE for 3 points.");

    let checkpoint_json = serde_json::to_vec(&initial.checkpoint().unwrap()).unwrap();
    drop(initial);
    let checkpoint = serde_json::from_slice(&checkpoint_json).unwrap();
    let mut resumed = GenericCommandDriver::restore(
        &manifest(),
        checkpoint,
        adapter.clone(),
        Arc::new(StepClock::new(2_000)),
    )
    .unwrap();
    resumed.resume(&cancel).await.unwrap();
    let second_output = resumed
        .request_turn(request("turn-2"), &cancel)
        .await
        .unwrap();
    assert_eq!(second_output.tool_calls.len(), 1);
    resumed
        .terminate(TerminationReason::Completed)
        .await
        .unwrap();
    resumed
        .terminate(TerminationReason::Operator)
        .await
        .unwrap();

    assert_eq!(resumed.telemetry().turns.len(), 2);
    assert_eq!(resumed.telemetry().diagnostics.len(), 1);
    assert_eq!(resumed.telemetry().restarts, 0);
    assert!(matches!(
        resumed.state(),
        DriverLifecycleState::Terminated {
            reason: TerminationReason::Completed
        }
    ));
    let process = adapter.process(1);
    assert_eq!(process.writes.lock().unwrap().len(), 2);
    assert_eq!(process.terminations.load(Ordering::SeqCst), 1);
    let telemetry = serde_json::to_string(resumed.telemetry()).unwrap();
    assert!(!telemetry.contains("reasoning"));
    assert!(!telemetry.contains("chain_of_thought"));
}

#[tokio::test]
async fn lifecycle_rejects_invalid_transitions_and_termination_is_idempotent() {
    let adapter = Arc::new(FakeAdapter::with_scripts(vec![vec![]]));
    let cancel = CancellationToken::new();
    let mut driver = new_driver(adapter.clone());

    assert!(matches!(
        driver.request_turn(request("early"), &cancel).await,
        Err(DriverError::InvalidTransition { .. })
    ));
    assert!(matches!(
        driver.resume(&cancel).await,
        Err(DriverError::InvalidTransition { .. })
    ));
    driver.start(&cancel).await.unwrap();
    assert!(matches!(
        driver.start(&cancel).await,
        Err(DriverError::InvalidTransition { .. })
    ));
    driver.resume(&cancel).await.unwrap();
    driver
        .terminate(TerminationReason::GameEnded)
        .await
        .unwrap();
    driver.terminate(TerminationReason::Operator).await.unwrap();
    assert_eq!(adapter.process(1).terminations.load(Ordering::SeqCst), 1);
    assert!(matches!(
        driver.request_turn(request("late"), &cancel).await,
        Err(DriverError::InvalidTransition { .. })
    ));
    assert!(matches!(
        driver.resume(&cancel).await,
        Err(DriverError::InvalidTransition { .. })
    ));
}

#[tokio::test]
async fn cancellation_wins_signal_races_before_spawn_or_visible_output() {
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let never_spawned = Arc::new(FakeAdapter::with_scripts(vec![vec![]]));
    let mut pending = new_driver(never_spawned.clone());
    assert!(matches!(
        pending.start(&cancelled).await,
        Err(DriverError::Cancelled)
    ));
    assert_eq!(never_spawned.spawn_calls(), 0);

    let ready_adapter = Arc::new(FakeAdapter::with_scripts(vec![vec![Ok(
        ProcessEvent::Stdout(output("turn-race", "must not be accepted")),
    )]]));
    let mut ready = new_driver(ready_adapter.clone());
    ready.start(&CancellationToken::new()).await.unwrap();
    assert!(matches!(
        ready.request_turn(request("turn-race"), &cancelled).await,
        Err(DriverError::Cancelled)
    ));
    assert!(ready.telemetry().turns.is_empty());
    assert!(ready_adapter.process(1).writes.lock().unwrap().is_empty());
    assert_eq!(
        ready_adapter.process(1).terminations.load(Ordering::SeqCst),
        1
    );
}

#[tokio::test]
async fn partial_output_is_discarded_on_crash_and_resume_starts_cleanly() {
    let adapter = Arc::new(FakeAdapter::with_scripts(vec![
        vec![
            Ok(ProcessEvent::Stdout(
                b"{\"visible_output\":\"secret partial".to_vec(),
            )),
            Ok(ProcessEvent::Exited(ExitStatus {
                success: false,
                code: Some(23),
                signal: None,
            })),
        ],
        vec![Ok(ProcessEvent::Stdout(output("turn-retry", "Recovered.")))],
    ]));
    let cancel = CancellationToken::new();
    let mut driver = new_driver(adapter.clone());
    driver.start(&cancel).await.unwrap();

    assert!(matches!(
        driver.request_turn(request("turn-crash"), &cancel).await,
        Err(DriverError::UnexpectedExit(ExitStatus {
            code: Some(23),
            ..
        }))
    ));
    assert!(driver.telemetry().turns.is_empty());
    driver.resume(&cancel).await.unwrap();
    let recovered = driver
        .request_turn(request("turn-retry"), &cancel)
        .await
        .unwrap();
    assert_eq!(recovered.visible_output, "Recovered.");
    assert_eq!(driver.telemetry().restarts, 1);
    assert_eq!(adapter.spawn_calls(), 2);
    assert!(
        !serde_json::to_string(driver.telemetry())
            .unwrap()
            .contains("secret partial")
    );
}

#[tokio::test]
async fn hidden_reasoning_and_ambiguous_frames_fail_closed_without_persistence() {
    let mut hidden = serde_json::to_vec(&json!({
        "schema_version": 1,
        "turn_id": "turn-hidden",
        "visible_output": "public",
        "chain_of_thought": "private deliberation"
    }))
    .unwrap();
    hidden.push(b'\n');
    let adapter = Arc::new(FakeAdapter::with_scripts(vec![vec![Ok(
        ProcessEvent::Stdout(hidden),
    )]]));
    let cancel = CancellationToken::new();
    let mut driver = new_driver(adapter);
    driver.start(&cancel).await.unwrap();
    assert!(matches!(
        driver.request_turn(request("turn-hidden"), &cancel).await,
        Err(DriverError::InvalidFrame)
    ));
    let telemetry = serde_json::to_string(driver.telemetry()).unwrap();
    assert!(!telemetry.contains("private deliberation"));
    assert!(!telemetry.contains("chain_of_thought"));

    let mut valid_plus_partial = output("turn-extra", "public");
    valid_plus_partial.push(b'{');
    let adapter = Arc::new(FakeAdapter::with_scripts(vec![vec![Ok(
        ProcessEvent::Stdout(valid_plus_partial),
    )]]));
    let mut driver = new_driver(adapter);
    driver.start(&cancel).await.unwrap();
    assert!(matches!(
        driver.request_turn(request("turn-extra"), &cancel).await,
        Err(DriverError::InvalidFrame)
    ));
}

#[tokio::test]
async fn oversized_stdout_and_turn_mismatch_are_protocol_crashes() {
    let adapter = Arc::new(FakeAdapter::with_scripts(vec![vec![Ok(
        ProcessEvent::Stdout(vec![b'x'; 1_048_577]),
    )]]));
    let cancel = CancellationToken::new();
    let mut driver = new_driver(adapter);
    driver.start(&cancel).await.unwrap();
    assert!(matches!(
        driver.request_turn(request("turn-large"), &cancel).await,
        Err(DriverError::FrameTooLarge)
    ));

    let adapter = Arc::new(FakeAdapter::with_scripts(vec![vec![Ok(
        ProcessEvent::Stdout(output("wrong-turn", "public")),
    )]]));
    let mut driver = new_driver(adapter);
    driver.start(&cancel).await.unwrap();
    assert!(matches!(
        driver.request_turn(request("expected-turn"), &cancel).await,
        Err(DriverError::TurnMismatch)
    ));
}

#[tokio::test]
async fn checkpoint_rejects_manifest_process_spec_and_lifecycle_drift() {
    let adapter = Arc::new(FakeAdapter::with_scripts(vec![vec![]]));
    let cancel = CancellationToken::new();
    let mut driver = new_driver(adapter.clone());
    driver.start(&cancel).await.unwrap();
    let checkpoint = driver.checkpoint().unwrap();

    let mut no_process = checkpoint.clone();
    no_process.process = None;
    assert!(matches!(
        GenericCommandDriver::restore(
            &manifest(),
            no_process,
            adapter.clone(),
            Arc::new(StepClock::new(1))
        ),
        Err(DriverError::InvalidCheckpoint)
    ));

    let mut command_drift = checkpoint.clone();
    command_drift.process_spec.executable = "/different/agent".to_owned();
    assert!(matches!(
        GenericCommandDriver::restore(
            &manifest(),
            command_drift,
            adapter.clone(),
            Arc::new(StepClock::new(1))
        ),
        Err(DriverError::InvalidCheckpoint)
    ));

    let mut lifecycle_drift = checkpoint;
    lifecycle_drift
        .telemetry
        .lifecycle
        .last_mut()
        .unwrap()
        .state = DriverLifecycleState::Pending;
    assert!(matches!(
        GenericCommandDriver::restore(
            &manifest(),
            lifecycle_drift,
            adapter,
            Arc::new(StepClock::new(1))
        ),
        Err(DriverError::InvalidCheckpoint)
    ));
}

#[tokio::test]
async fn missing_saved_process_restarts_with_reconstructed_telemetry() {
    let adapter = Arc::new(FakeAdapter::with_scripts(vec![
        vec![],
        vec![Ok(ProcessEvent::Stdout(output(
            "turn-after-restart",
            "Replacement ready.",
        )))],
    ]));
    let cancel = CancellationToken::new();
    let mut initial = new_driver(adapter.clone());
    initial.start(&cancel).await.unwrap();
    let mut checkpoint = initial.checkpoint().unwrap();
    checkpoint.process = Some(ProcessHandle("fake:missing".to_owned()));
    drop(initial);

    let mut restored = GenericCommandDriver::restore(
        &manifest(),
        checkpoint,
        adapter.clone(),
        Arc::new(StepClock::new(3_000)),
    )
    .unwrap();
    restored.resume(&cancel).await.unwrap();
    restored
        .request_turn(request("turn-after-restart"), &cancel)
        .await
        .unwrap();

    assert_eq!(adapter.spawn_calls(), 2);
    assert_eq!(restored.telemetry().restarts, 1);
    assert_eq!(
        restored.telemetry().diagnostics[0].code,
        "process_restart_after_restore"
    );
    assert_eq!(restored.telemetry().turns.len(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn tokio_adapter_direct_exec_frames_stdout_and_maps_termination() {
    let adapter = TokioProcessAdapter;
    let mut process = adapter
        .spawn(&ProcessSpec {
            executable: "/bin/cat".to_owned(),
            arguments: Vec::new(),
            working_directory: None,
        })
        .await
        .unwrap();
    process.write(b"visible line\n").await.unwrap();
    assert_eq!(
        process.next_event().await.unwrap(),
        ProcessEvent::Stdout(b"visible line\n".to_vec())
    );
    let exit = process.terminate().await.unwrap();
    assert!(!exit.success);
}
