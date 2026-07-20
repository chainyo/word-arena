use std::{
    collections::VecDeque,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use serde_json::json;
use tokio_util::sync::CancellationToken;
use word_arena_agent_runtime::{
    AgentDriver, BUDGET_CAPABILITY_SCHEMA_VERSION, BUDGET_TELEMETRY_SCHEMA_VERSION,
    BudgetController, BudgetDimension, BudgetEnforcementStatus, BudgetError, BudgetedAgentDriver,
    BudgetedProcessAdapter, DiagnosticRecord, DriverClock, DriverError, DriverFuture,
    DriverLifecycleState, DriverTelemetry, ExitStatus, NetworkPolicy, PlatformBudgetCapabilities,
    ProcessAdapter, ProcessError, ProcessEvent, ProcessHandle, ProcessInstance, ProcessSpec,
    ResourceBudgets, TerminationReason, TokioProcessAdapter, TurnRequest, UnenforcedBudgetPolicy,
    VisibleToolCall, VisibleTurnOutput,
};

#[derive(Debug)]
struct FixedClock;

impl DriverClock for FixedClock {
    fn now_unix_ms(&self) -> i64 {
        10_000
    }
}

fn budgets() -> ResourceBudgets {
    ResourceBudgets {
        wall_time_ms: 100,
        cpu_time_ms: 50,
        memory_bytes: 1_024,
        network_bytes: 100,
        input_tokens: 10,
        output_tokens: 10,
        attempts: 2,
        tool_calls: 2,
        output_bytes: 8,
        cost_microusd: 10,
    }
}

fn controller(mut values: ResourceBudgets) -> Arc<BudgetController> {
    if values.wall_time_ms == 0 {
        values.wall_time_ms = 1;
    }
    Arc::new(
        BudgetController::new(
            values,
            PlatformBudgetCapabilities::detect(&NetworkPolicy::Deny),
            UnenforcedBudgetPolicy::AllowReported,
            Arc::new(FixedClock),
        )
        .unwrap(),
    )
}

#[test]
fn capability_report_is_versioned_complete_and_policy_can_fail_closed() {
    let contract_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/agent-budget-v1.json");
    let contract: serde_json::Value =
        serde_json::from_slice(&fs::read(contract_path).unwrap()).unwrap();
    assert_eq!(
        contract["capability_schema_version"],
        json!(BUDGET_CAPABILITY_SCHEMA_VERSION)
    );
    assert_eq!(
        contract["telemetry_schema_version"],
        json!(BUDGET_TELEMETRY_SCHEMA_VERSION)
    );
    assert_eq!(contract["dimensions"].as_array().unwrap().len(), 10);
    let report = PlatformBudgetCapabilities::detect(&NetworkPolicy::Deny);
    assert_eq!(report.schema_version, BUDGET_CAPABILITY_SCHEMA_VERSION);
    assert_eq!(report.capabilities.len(), 10);
    assert_eq!(
        report.status(BudgetDimension::WallTime),
        Some(BudgetEnforcementStatus::Hard)
    );
    assert_eq!(
        report.status(BudgetDimension::NetworkBytes),
        Some(BudgetEnforcementStatus::Hard)
    );
    assert_eq!(
        report.status(BudgetDimension::CpuTime),
        Some(BudgetEnforcementStatus::Unenforced)
    );
    assert_eq!(
        report.status(BudgetDimension::InputTokens),
        Some(BudgetEnforcementStatus::Conditional)
    );
    assert!(matches!(
        BudgetController::new(
            budgets(),
            report,
            UnenforcedBudgetPolicy::FailClosed,
            Arc::new(FixedClock)
        ),
        Err(BudgetError::UnsupportedPlatform)
    ));
}

#[test]
fn normalized_accounting_orders_limit_events_without_overflow() {
    let controller = controller(budgets());
    controller.begin_attempt().unwrap();
    controller.begin_attempt().unwrap();
    assert_eq!(controller.begin_attempt(), Err(BudgetError::Exceeded));
    controller.record_tool_calls(2).unwrap();
    assert_eq!(
        controller.record_tool_calls(usize::MAX),
        Err(BudgetError::Exceeded)
    );
    controller
        .record_reported_usage(Some(5), Some(6), Some(7))
        .unwrap();
    assert_eq!(
        controller.record_reported_usage(Some(6), None, None),
        Err(BudgetError::Exceeded)
    );
    let telemetry = controller.snapshot().unwrap();
    assert_eq!(telemetry.schema_version, BUDGET_TELEMETRY_SCHEMA_VERSION);
    assert_eq!(telemetry.consumption.attempts, 3);
    assert_eq!(telemetry.limit_events.len(), 3);
    for (sequence, event) in telemetry.limit_events.iter().enumerate() {
        assert_eq!(event.sequence, sequence as u64);
        assert_eq!(event.at_unix_ms, 10_000);
    }
    let json = serde_json::to_value(telemetry).unwrap();
    assert_eq!(json["limit_events"][0]["dimension"], json!("attempts"));
}

#[derive(Debug)]
struct FakeProcessAdapter {
    events: Mutex<VecDeque<ProcessEvent>>,
    terminations: Arc<AtomicUsize>,
    pending: bool,
}

impl ProcessAdapter for FakeProcessAdapter {
    fn spawn<'a>(
        &'a self,
        _spec: &'a ProcessSpec,
    ) -> DriverFuture<'a, Result<Box<dyn ProcessInstance>, ProcessError>> {
        Box::pin(async move {
            Ok(Box::new(FakeProcess {
                events: Mutex::new(self.events.lock().unwrap().clone()),
                terminations: Arc::clone(&self.terminations),
                pending: self.pending,
            }) as Box<dyn ProcessInstance>)
        })
    }

    fn reattach<'a>(
        &'a self,
        _handle: &'a ProcessHandle,
    ) -> DriverFuture<'a, Result<Box<dyn ProcessInstance>, ProcessError>> {
        Box::pin(async { Err(ProcessError::ReattachUnsupported) })
    }
}

#[derive(Debug)]
struct FakeProcess {
    events: Mutex<VecDeque<ProcessEvent>>,
    terminations: Arc<AtomicUsize>,
    pending: bool,
}

impl ProcessInstance for FakeProcess {
    fn handle(&self) -> ProcessHandle {
        ProcessHandle("fake-budget".to_owned())
    }

    fn write<'a>(&'a mut self, _bytes: &'a [u8]) -> DriverFuture<'a, Result<(), ProcessError>> {
        Box::pin(async { Ok(()) })
    }

    fn close_input(&mut self) -> DriverFuture<'_, Result<(), ProcessError>> {
        Box::pin(async { Ok(()) })
    }

    fn next_event(&mut self) -> DriverFuture<'_, Result<ProcessEvent, ProcessError>> {
        Box::pin(async move {
            if self.pending {
                std::future::pending().await
            } else {
                self.events
                    .lock()
                    .unwrap()
                    .pop_front()
                    .ok_or(ProcessError::Read)
            }
        })
    }

    fn terminate(&mut self) -> DriverFuture<'_, Result<ExitStatus, ProcessError>> {
        Box::pin(async move {
            self.terminations.fetch_add(1, Ordering::SeqCst);
            Ok(ExitStatus {
                success: false,
                code: None,
                signal: Some(9),
            })
        })
    }
}

fn spec() -> ProcessSpec {
    ProcessSpec {
        executable: "/usr/bin/true".to_owned(),
        arguments: Vec::new(),
        working_directory: Some(PathBuf::from("/tmp")),
    }
}

#[tokio::test]
async fn raw_output_flood_and_wall_timeout_terminate_before_more_events() {
    let terminations = Arc::new(AtomicUsize::new(0));
    let adapter = BudgetedProcessAdapter::new(
        Arc::new(FakeProcessAdapter {
            events: Mutex::new(VecDeque::from([ProcessEvent::Stdout(vec![b'x'; 9])])),
            terminations: Arc::clone(&terminations),
            pending: false,
        }),
        controller(budgets()),
    );
    let mut process = adapter.spawn(&spec()).await.unwrap();
    assert_eq!(process.next_event().await, Err(ProcessError::LimitExceeded));
    assert_eq!(terminations.load(Ordering::SeqCst), 1);

    let mut timeout_budgets = budgets();
    timeout_budgets.wall_time_ms = 20;
    let timeout_controller = controller(timeout_budgets);
    let timeout_terminations = Arc::new(AtomicUsize::new(0));
    let adapter = BudgetedProcessAdapter::new(
        Arc::new(FakeProcessAdapter {
            events: Mutex::new(VecDeque::new()),
            terminations: Arc::clone(&timeout_terminations),
            pending: true,
        }),
        Arc::clone(&timeout_controller),
    );
    let mut process = adapter.spawn(&spec()).await.unwrap();
    assert_eq!(process.next_event().await, Err(ProcessError::LimitExceeded));
    assert_eq!(timeout_terminations.load(Ordering::SeqCst), 1);
    assert_eq!(
        timeout_controller.snapshot().unwrap().limit_events[0].dimension,
        BudgetDimension::WallTime
    );
}

#[derive(Debug)]
struct FakeAgentDriver {
    state: DriverLifecycleState,
    telemetry: DriverTelemetry,
    output: VisibleTurnOutput,
    terminations: Vec<TerminationReason>,
}

impl FakeAgentDriver {
    fn new(tool_calls: usize) -> Self {
        Self {
            state: DriverLifecycleState::Ready,
            telemetry: DriverTelemetry {
                schema_version: 1,
                run_id: "run-budget".to_owned(),
                manifest: word_arena_agent_runtime::AgentManifestIdentity {
                    schema_version: 1,
                    hash_algorithm: "test".to_owned(),
                    manifest_sha256: "a".repeat(64),
                },
                restarts: 0,
                lifecycle: Vec::new(),
                turns: Vec::new(),
                diagnostics: Vec::<DiagnosticRecord>::new(),
            },
            output: VisibleTurnOutput {
                schema_version: 1,
                turn_id: "turn".to_owned(),
                visible_output: "done".to_owned(),
                tool_calls: (0..tool_calls)
                    .map(|number| VisibleToolCall {
                        tool: format!("tool-{number}"),
                        arguments: json!({}),
                        result: json!({}),
                    })
                    .collect(),
            },
            terminations: Vec::new(),
        }
    }
}

impl AgentDriver for FakeAgentDriver {
    fn state(&self) -> &DriverLifecycleState {
        &self.state
    }

    fn telemetry(&self) -> &DriverTelemetry {
        &self.telemetry
    }

    fn start<'a>(
        &'a mut self,
        _cancel: &'a CancellationToken,
    ) -> DriverFuture<'a, Result<(), DriverError>> {
        Box::pin(async { Ok(()) })
    }

    fn request_turn<'a>(
        &'a mut self,
        request: TurnRequest,
        _cancel: &'a CancellationToken,
    ) -> DriverFuture<'a, Result<VisibleTurnOutput, DriverError>> {
        Box::pin(async move {
            let mut output = self.output.clone();
            output.turn_id = request.turn_id;
            Ok(output)
        })
    }

    fn resume<'a>(
        &'a mut self,
        _cancel: &'a CancellationToken,
    ) -> DriverFuture<'a, Result<(), DriverError>> {
        Box::pin(async { Ok(()) })
    }

    fn terminate(
        &mut self,
        reason: TerminationReason,
    ) -> DriverFuture<'_, Result<(), DriverError>> {
        Box::pin(async move {
            self.terminations.push(reason);
            self.state = DriverLifecycleState::Terminated { reason };
            Ok(())
        })
    }
}

#[tokio::test]
async fn common_driver_wrapper_enforces_attempts_and_tool_calls() {
    let mut tool_budgets = budgets();
    tool_budgets.tool_calls = 1;
    let tool_controller = controller(tool_budgets);
    let mut driver = BudgetedAgentDriver::new(FakeAgentDriver::new(2), tool_controller);
    assert!(matches!(
        driver
            .request_turn(
                TurnRequest {
                    turn_id: "turn-1".to_owned(),
                    visible_input: "play".to_owned(),
                },
                &CancellationToken::new()
            )
            .await,
        Err(DriverError::BudgetExceeded)
    ));
    assert_eq!(
        driver.into_inner().terminations,
        [TerminationReason::BudgetExceeded]
    );

    let mut attempt_budgets = budgets();
    attempt_budgets.attempts = 1;
    let mut driver = BudgetedAgentDriver::new(FakeAgentDriver::new(0), controller(attempt_budgets));
    let request = || TurnRequest {
        turn_id: "turn".to_owned(),
        visible_input: "play".to_owned(),
    };
    driver
        .request_turn(request(), &CancellationToken::new())
        .await
        .unwrap();
    assert!(matches!(
        driver
            .request_turn(request(), &CancellationToken::new())
            .await,
        Err(DriverError::BudgetExceeded)
    ));
    assert_eq!(
        driver.into_inner().terminations,
        [TerminationReason::BudgetExceeded]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn tokio_termination_kills_the_complete_process_group() {
    let mut process = TokioProcessAdapter
        .spawn(&ProcessSpec {
            executable: "/bin/sh".to_owned(),
            arguments: vec![
                "-c".to_owned(),
                "sleep 30 & child=$!; printf '%s\\n' \"$child\"; wait".to_owned(),
            ],
            working_directory: None,
        })
        .await
        .unwrap();
    let child_pid = loop {
        if let ProcessEvent::Stdout(bytes) = process.next_event().await.unwrap() {
            break String::from_utf8(bytes)
                .unwrap()
                .trim()
                .parse::<u32>()
                .unwrap();
        }
    };
    let exit = process.terminate().await.unwrap();
    assert!(!exit.success);
    for _ in 0..50 {
        let alive = tokio::process::Command::new("/bin/kill")
            .arg("-0")
            .arg(child_pid.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .is_ok_and(|status| status.success());
        if !alive {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("grandchild process {child_pid} survived process-group termination");
}

#[test]
fn opt_in_platform_pressure_contract_is_explicit() {
    if std::env::var_os("WORD_ARENA_RUN_PLATFORM_BUDGET_SMOKE").is_none() {
        return;
    }
    let report = PlatformBudgetCapabilities::detect(&NetworkPolicy::McpOnly);
    assert_eq!(
        report.status(BudgetDimension::CpuTime),
        Some(BudgetEnforcementStatus::Unenforced)
    );
    assert_eq!(
        report.status(BudgetDimension::Memory),
        Some(BudgetEnforcementStatus::Unenforced)
    );
    assert_eq!(
        report.status(BudgetDimension::NetworkBytes),
        Some(BudgetEnforcementStatus::Unenforced)
    );
}
