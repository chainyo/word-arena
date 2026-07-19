use std::{
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::{
    AgentDriver, DriverClock, DriverError, DriverFuture, DriverLifecycleState, DriverTelemetry,
    NetworkPolicy, ProcessAdapter, ProcessError, ProcessEvent, ProcessHandle, ProcessInstance,
    ProcessSpec, ResourceBudgets, TerminationReason, TurnRequest, VisibleTurnOutput,
};

pub const BUDGET_CAPABILITY_SCHEMA_VERSION: u32 = 1;
pub const BUDGET_TELEMETRY_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetDimension {
    WallTime,
    CpuTime,
    Memory,
    NetworkBytes,
    InputTokens,
    OutputTokens,
    Attempts,
    ToolCalls,
    OutputBytes,
    Cost,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetEnforcementStatus {
    Hard,
    Conditional,
    Unenforced,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetCapability {
    pub dimension: BudgetDimension,
    pub status: BudgetEnforcementStatus,
    pub source: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformBudgetCapabilities {
    pub schema_version: u32,
    pub platform: String,
    pub capabilities: Vec<BudgetCapability>,
}

impl PlatformBudgetCapabilities {
    #[must_use]
    pub fn detect(network: &NetworkPolicy) -> Self {
        let network_status = if matches!(network, NetworkPolicy::Deny) {
            BudgetEnforcementStatus::Hard
        } else {
            BudgetEnforcementStatus::Unenforced
        };
        let network_source = if matches!(network, NetworkPolicy::Deny) {
            "os_sandbox_network_denial"
        } else {
            "byte_meter_unavailable"
        };
        Self {
            schema_version: BUDGET_CAPABILITY_SCHEMA_VERSION,
            platform: std::env::consts::OS.to_owned(),
            capabilities: vec![
                capability(
                    BudgetDimension::WallTime,
                    BudgetEnforcementStatus::Hard,
                    "tokio_deadline",
                ),
                capability(
                    BudgetDimension::CpuTime,
                    BudgetEnforcementStatus::Unenforced,
                    "platform_limiter_unavailable",
                ),
                capability(
                    BudgetDimension::Memory,
                    BudgetEnforcementStatus::Unenforced,
                    "platform_limiter_unavailable",
                ),
                capability(
                    BudgetDimension::NetworkBytes,
                    network_status,
                    network_source,
                ),
                capability(
                    BudgetDimension::InputTokens,
                    BudgetEnforcementStatus::Conditional,
                    "provider_reported_usage",
                ),
                capability(
                    BudgetDimension::OutputTokens,
                    BudgetEnforcementStatus::Conditional,
                    "provider_reported_usage",
                ),
                capability(
                    BudgetDimension::Attempts,
                    BudgetEnforcementStatus::Hard,
                    "driver_turn_boundary",
                ),
                capability(
                    BudgetDimension::ToolCalls,
                    BudgetEnforcementStatus::Hard,
                    "normalized_visible_tool_calls",
                ),
                capability(
                    BudgetDimension::OutputBytes,
                    BudgetEnforcementStatus::Hard,
                    "process_stream_meter",
                ),
                capability(
                    BudgetDimension::Cost,
                    BudgetEnforcementStatus::Conditional,
                    "provider_reported_usage",
                ),
            ],
        }
    }

    #[must_use]
    pub fn status(&self, dimension: BudgetDimension) -> Option<BudgetEnforcementStatus> {
        self.capabilities
            .iter()
            .find(|value| value.dimension == dimension)
            .map(|value| value.status)
    }
}

fn capability(
    dimension: BudgetDimension,
    status: BudgetEnforcementStatus,
    source: &str,
) -> BudgetCapability {
    BudgetCapability {
        dimension,
        status,
        source: source.to_owned(),
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnenforcedBudgetPolicy {
    AllowReported,
    FailClosed,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetConsumption {
    pub wall_time_ms: u64,
    pub cpu_time_ms: u64,
    pub peak_memory_bytes: u64,
    pub network_bytes: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub attempts: u64,
    pub tool_calls: u64,
    pub output_bytes: u64,
    pub cost_microusd: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetLimitEvent {
    pub sequence: u64,
    pub at_unix_ms: i64,
    pub dimension: BudgetDimension,
    pub limit: u64,
    pub observed: u64,
    pub source: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetTelemetry {
    pub schema_version: u32,
    pub capabilities: PlatformBudgetCapabilities,
    pub consumption: BudgetConsumption,
    pub limit_events: Vec<BudgetLimitEvent>,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum BudgetError {
    #[error("required budget dimensions cannot be enforced on this platform")]
    UnsupportedPlatform,
    #[error("agent resource budget was exceeded")]
    Exceeded,
    #[error("budget state is unavailable")]
    Unavailable,
}

#[derive(Debug)]
struct BudgetState {
    consumption: BudgetConsumption,
    limit_events: Vec<BudgetLimitEvent>,
}

pub struct BudgetController {
    budgets: ResourceBudgets,
    capabilities: PlatformBudgetCapabilities,
    clock: Arc<dyn DriverClock>,
    started: Instant,
    state: Mutex<BudgetState>,
}

impl fmt::Debug for BudgetController {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BudgetController")
            .field("budgets", &self.budgets)
            .field("capabilities", &self.capabilities)
            .finish_non_exhaustive()
    }
}

impl BudgetController {
    /// Creates one run-scoped controller with an immutable capability report.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetError::UnsupportedPlatform`] when policy requires hard
    /// enforcement for every dimension and the report contains weaker support.
    pub fn new(
        budgets: ResourceBudgets,
        capabilities: PlatformBudgetCapabilities,
        policy: UnenforcedBudgetPolicy,
        clock: Arc<dyn DriverClock>,
    ) -> Result<Self, BudgetError> {
        if policy == UnenforcedBudgetPolicy::FailClosed
            && capabilities
                .capabilities
                .iter()
                .any(|value| value.status != BudgetEnforcementStatus::Hard)
        {
            return Err(BudgetError::UnsupportedPlatform);
        }
        Ok(Self {
            budgets,
            capabilities,
            clock,
            started: Instant::now(),
            state: Mutex::new(BudgetState {
                consumption: BudgetConsumption::default(),
                limit_events: Vec::new(),
            }),
        })
    }

    #[must_use]
    pub fn capabilities(&self) -> &PlatformBudgetCapabilities {
        &self.capabilities
    }

    /// Records one turn attempt and rejects values beyond the manifest limit.
    ///
    /// # Errors
    ///
    /// Returns an exceeded or unavailable budget error.
    pub fn begin_attempt(&self) -> Result<(), BudgetError> {
        self.add_and_check(BudgetDimension::Attempts, 1, "driver_turn_boundary")
    }

    /// Records normalized visible tool calls.
    ///
    /// # Errors
    ///
    /// Returns an exceeded or unavailable budget error.
    pub fn record_tool_calls(&self, count: usize) -> Result<(), BudgetError> {
        self.add_and_check(
            BudgetDimension::ToolCalls,
            u64::try_from(count).unwrap_or(u64::MAX),
            "normalized_visible_tool_calls",
        )
    }

    /// Records raw process output bytes before parsing.
    ///
    /// # Errors
    ///
    /// Returns an exceeded or unavailable budget error.
    pub fn record_output_bytes(&self, count: usize) -> Result<(), BudgetError> {
        self.add_and_check(
            BudgetDimension::OutputBytes,
            u64::try_from(count).unwrap_or(u64::MAX),
            "process_stream_meter",
        )
    }

    /// Records bytes from an injected platform network meter when available.
    ///
    /// # Errors
    ///
    /// Returns an exceeded or unavailable budget error.
    pub fn record_network_bytes(&self, count: u64) -> Result<(), BudgetError> {
        self.add_and_check(
            BudgetDimension::NetworkBytes,
            count,
            "platform_network_meter",
        )
    }

    /// Records exact usage fields exposed by a provider or local runtime.
    ///
    /// # Errors
    ///
    /// Returns an exceeded or unavailable budget error.
    pub fn record_reported_usage(
        &self,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        cost_microusd: Option<u64>,
    ) -> Result<(), BudgetError> {
        for (dimension, value, source) in [
            (
                BudgetDimension::InputTokens,
                input_tokens,
                "provider_reported_usage",
            ),
            (
                BudgetDimension::OutputTokens,
                output_tokens,
                "provider_reported_usage",
            ),
            (
                BudgetDimension::Cost,
                cost_microusd,
                "provider_reported_usage",
            ),
        ] {
            if let Some(value) = value {
                self.add_and_check(dimension, value, source)?;
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn remaining_wall_time(&self) -> Duration {
        Duration::from_millis(self.budgets.wall_time_ms).saturating_sub(self.started.elapsed())
    }

    pub fn record_wall_timeout(&self) {
        let observed = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let _ = self.exceed(
            BudgetDimension::WallTime,
            self.budgets.wall_time_ms,
            observed.max(self.budgets.wall_time_ms.saturating_add(1)),
            "tokio_deadline",
        );
    }

    /// Captures normalized consumption, capabilities, and ordered limit events.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetError::Unavailable`] after mutex poisoning.
    pub fn snapshot(&self) -> Result<BudgetTelemetry, BudgetError> {
        let mut state = self.state.lock().map_err(|_| BudgetError::Unavailable)?;
        state.consumption.wall_time_ms =
            u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        Ok(BudgetTelemetry {
            schema_version: BUDGET_TELEMETRY_SCHEMA_VERSION,
            capabilities: self.capabilities.clone(),
            consumption: state.consumption.clone(),
            limit_events: state.limit_events.clone(),
        })
    }

    fn add_and_check(
        &self,
        dimension: BudgetDimension,
        amount: u64,
        source: &str,
    ) -> Result<(), BudgetError> {
        let mut state = self.state.lock().map_err(|_| BudgetError::Unavailable)?;
        let current = consumption_mut(&mut state.consumption, dimension);
        *current = current.saturating_add(amount);
        let observed = *current;
        let limit = self.limit(dimension);
        if observed > limit {
            record_limit(
                &mut state,
                self.clock.as_ref(),
                dimension,
                limit,
                observed,
                source,
            );
            Err(BudgetError::Exceeded)
        } else {
            Ok(())
        }
    }

    fn exceed(
        &self,
        dimension: BudgetDimension,
        limit: u64,
        observed: u64,
        source: &str,
    ) -> Result<(), BudgetError> {
        let mut state = self.state.lock().map_err(|_| BudgetError::Unavailable)?;
        if !state
            .limit_events
            .iter()
            .any(|event| event.dimension == dimension)
        {
            record_limit(
                &mut state,
                self.clock.as_ref(),
                dimension,
                limit,
                observed,
                source,
            );
        }
        Err(BudgetError::Exceeded)
    }

    const fn limit(&self, dimension: BudgetDimension) -> u64 {
        match dimension {
            BudgetDimension::WallTime => self.budgets.wall_time_ms,
            BudgetDimension::CpuTime => self.budgets.cpu_time_ms,
            BudgetDimension::Memory => self.budgets.memory_bytes,
            BudgetDimension::NetworkBytes => self.budgets.network_bytes,
            BudgetDimension::InputTokens => self.budgets.input_tokens,
            BudgetDimension::OutputTokens => self.budgets.output_tokens,
            BudgetDimension::Attempts => self.budgets.attempts as u64,
            BudgetDimension::ToolCalls => self.budgets.tool_calls as u64,
            BudgetDimension::OutputBytes => self.budgets.output_bytes,
            BudgetDimension::Cost => self.budgets.cost_microusd,
        }
    }
}

fn consumption_mut(consumption: &mut BudgetConsumption, dimension: BudgetDimension) -> &mut u64 {
    match dimension {
        BudgetDimension::WallTime => &mut consumption.wall_time_ms,
        BudgetDimension::CpuTime => &mut consumption.cpu_time_ms,
        BudgetDimension::Memory => &mut consumption.peak_memory_bytes,
        BudgetDimension::NetworkBytes => &mut consumption.network_bytes,
        BudgetDimension::InputTokens => &mut consumption.input_tokens,
        BudgetDimension::OutputTokens => &mut consumption.output_tokens,
        BudgetDimension::Attempts => &mut consumption.attempts,
        BudgetDimension::ToolCalls => &mut consumption.tool_calls,
        BudgetDimension::OutputBytes => &mut consumption.output_bytes,
        BudgetDimension::Cost => &mut consumption.cost_microusd,
    }
}

fn record_limit(
    state: &mut BudgetState,
    clock: &dyn DriverClock,
    dimension: BudgetDimension,
    limit: u64,
    observed: u64,
    source: &str,
) {
    state.limit_events.push(BudgetLimitEvent {
        sequence: state.limit_events.len() as u64,
        at_unix_ms: clock.now_unix_ms(),
        dimension,
        limit,
        observed,
        source: source.to_owned(),
    });
}

#[derive(Clone)]
pub struct BudgetedProcessAdapter {
    inner: Arc<dyn ProcessAdapter>,
    controller: Arc<BudgetController>,
}

impl BudgetedProcessAdapter {
    #[must_use]
    pub fn new(inner: Arc<dyn ProcessAdapter>, controller: Arc<BudgetController>) -> Self {
        Self { inner, controller }
    }
}

impl fmt::Debug for BudgetedProcessAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BudgetedProcessAdapter")
            .field("inner", &self.inner)
            .field("controller", &self.controller)
            .finish()
    }
}

impl ProcessAdapter for BudgetedProcessAdapter {
    fn spawn<'a>(
        &'a self,
        spec: &'a ProcessSpec,
    ) -> DriverFuture<'a, Result<Box<dyn ProcessInstance>, ProcessError>> {
        Box::pin(async move {
            let process = tokio::time::timeout(
                self.controller.remaining_wall_time(),
                self.inner.spawn(spec),
            )
            .await
            .map_err(|_| {
                self.controller.record_wall_timeout();
                ProcessError::LimitExceeded
            })??;
            Ok(Box::new(BudgetedProcess {
                inner: process,
                controller: Arc::clone(&self.controller),
            }) as Box<dyn ProcessInstance>)
        })
    }

    fn reattach<'a>(
        &'a self,
        handle: &'a ProcessHandle,
    ) -> DriverFuture<'a, Result<Box<dyn ProcessInstance>, ProcessError>> {
        Box::pin(async move {
            let process = self.inner.reattach(handle).await?;
            Ok(Box::new(BudgetedProcess {
                inner: process,
                controller: Arc::clone(&self.controller),
            }) as Box<dyn ProcessInstance>)
        })
    }
}

struct BudgetedProcess {
    inner: Box<dyn ProcessInstance>,
    controller: Arc<BudgetController>,
}

impl fmt::Debug for BudgetedProcess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BudgetedProcess")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl ProcessInstance for BudgetedProcess {
    fn handle(&self) -> ProcessHandle {
        self.inner.handle()
    }

    fn write<'a>(&'a mut self, bytes: &'a [u8]) -> DriverFuture<'a, Result<(), ProcessError>> {
        self.inner.write(bytes)
    }

    fn next_event(&mut self) -> DriverFuture<'_, Result<ProcessEvent, ProcessError>> {
        Box::pin(async move {
            let event = if let Ok(event) = tokio::time::timeout(
                self.controller.remaining_wall_time(),
                self.inner.next_event(),
            )
            .await
            {
                event?
            } else {
                self.controller.record_wall_timeout();
                let _ = self.inner.terminate().await;
                return Err(ProcessError::LimitExceeded);
            };
            if let ProcessEvent::Stdout(bytes) | ProcessEvent::Stderr(bytes) = &event
                && self.controller.record_output_bytes(bytes.len()).is_err()
            {
                let _ = self.inner.terminate().await;
                return Err(ProcessError::LimitExceeded);
            }
            Ok(event)
        })
    }

    fn terminate(&mut self) -> DriverFuture<'_, Result<crate::ExitStatus, ProcessError>> {
        self.inner.terminate()
    }
}

pub struct BudgetedAgentDriver<D> {
    inner: D,
    controller: Arc<BudgetController>,
}

impl<D> BudgetedAgentDriver<D> {
    #[must_use]
    pub fn new(inner: D, controller: Arc<BudgetController>) -> Self {
        Self { inner, controller }
    }

    #[must_use]
    pub fn controller(&self) -> &Arc<BudgetController> {
        &self.controller
    }

    #[must_use]
    pub fn into_inner(self) -> D {
        self.inner
    }
}

impl<D: fmt::Debug> fmt::Debug for BudgetedAgentDriver<D> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BudgetedAgentDriver")
            .field("inner", &self.inner)
            .field("controller", &self.controller)
            .finish()
    }
}

impl<D: AgentDriver + Send> AgentDriver for BudgetedAgentDriver<D> {
    fn state(&self) -> &DriverLifecycleState {
        self.inner.state()
    }

    fn telemetry(&self) -> &DriverTelemetry {
        self.inner.telemetry()
    }

    fn start<'a>(
        &'a mut self,
        cancel: &'a CancellationToken,
    ) -> DriverFuture<'a, Result<(), DriverError>> {
        self.inner.start(cancel)
    }

    fn request_turn<'a>(
        &'a mut self,
        request: TurnRequest,
        cancel: &'a CancellationToken,
    ) -> DriverFuture<'a, Result<VisibleTurnOutput, DriverError>> {
        Box::pin(async move {
            if self.controller.begin_attempt().is_err() {
                let _ = self
                    .inner
                    .terminate(TerminationReason::BudgetExceeded)
                    .await;
                return Err(DriverError::BudgetExceeded);
            }
            let result = tokio::time::timeout(
                self.controller.remaining_wall_time(),
                self.inner.request_turn(request, cancel),
            )
            .await;
            let output = if let Ok(output) = result {
                match output {
                    Err(DriverError::Process(ProcessError::LimitExceeded)) => {
                        return Err(DriverError::BudgetExceeded);
                    }
                    output => output?,
                }
            } else {
                self.controller.record_wall_timeout();
                let _ = self
                    .inner
                    .terminate(TerminationReason::BudgetExceeded)
                    .await;
                return Err(DriverError::BudgetExceeded);
            };
            if self
                .controller
                .record_tool_calls(output.tool_calls.len())
                .is_err()
            {
                let _ = self
                    .inner
                    .terminate(TerminationReason::BudgetExceeded)
                    .await;
                return Err(DriverError::BudgetExceeded);
            }
            Ok(output)
        })
    }

    fn resume<'a>(
        &'a mut self,
        cancel: &'a CancellationToken,
    ) -> DriverFuture<'a, Result<(), DriverError>> {
        self.inner.resume(cancel)
    }

    fn terminate(
        &mut self,
        reason: TerminationReason,
    ) -> DriverFuture<'_, Result<(), DriverError>> {
        self.inner.terminate(reason)
    }
}
