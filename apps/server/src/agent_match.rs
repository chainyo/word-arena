use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    fmt, fs,
    path::PathBuf,
    process::Command,
    sync::Arc,
};

use semver::Version;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use word_arena_agent_runtime::{
    AGENT_MANIFEST_SCHEMA_VERSION, AgentDriver, AgentManifest, AuthorityAuditError,
    AuthorityBoundaryAuditEvent, AuthorityBoundaryAuditSink, AuthorityBoundaryConfig,
    CLAUDE_CODE_MINIMUM_VERSION, CLINE_MINIMUM_VERSION, CODEX_MINIMUM_VERSION, DiagnosticRecord,
    EnvironmentIdentity, ForbiddenAuthorityFingerprint, ForbiddenAuthorityKind,
    ForbiddenAuthorityPolicy, HarnessConfig, HarnessExecutables, ModelConfig, ModelProvider,
    ModelSource, NetworkPolicy, PI_MINIMUM_VERSION, PromptIdentity, ResourceBudgets,
    SeatCapability, SeatWorkspaceLease, SeatWorkspaceManager, SeatWorkspaceRequest,
    SupportedAgentDriver, SystemDriverClock, TerminationReason, ToolPolicy, TurnRequest,
    ValidatedAgentManifest, WorkspaceManagerConfig, WorkspaceOutcome, WorkspacePersistence,
    WorkspacePolicy, WorkspaceRetention,
};
use word_arena_application::{
    CreatedGameAccess, GameActionCommand, GameId, IdempotencyKey, PublicGameQuery, UnixMillis,
};
use word_arena_engine::{GameMode, GamePhase, Language, Move, PublicProjection, Seat, Turn};
use word_arena_persistence::{SqliteLocalMatchRepository, StoredLocalAgentMatch};

use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::{API_SCHEMA_VERSION, ServerState};

const MAX_ACTIVITY_EVENTS: usize = 256;
const MAX_ACTIVITY_MESSAGE_CHARS: usize = 1_000;

/// Stable local harness identifiers accepted by the operator match API.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentHarnessId {
    Codex,
    ClaudeCode,
    Cline,
    Pi,
}

impl AgentHarnessId {
    pub const ALL: [Self; 4] = [Self::Codex, Self::ClaudeCode, Self::Cline, Self::Pi];

    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude_code",
            Self::Cline => "cline",
            Self::Pi => "pi",
        }
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::ClaudeCode => "Claude Code",
            Self::Cline => "Cline",
            Self::Pi => "Pi",
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

    #[must_use]
    pub const fn logo(self) -> &'static str {
        match self {
            Self::Codex => "openai",
            Self::ClaudeCode => "claude",
            Self::Cline => "cline",
            Self::Pi => "pi",
        }
    }
}

/// Safe result of probing one local CLI with `--version`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCatalogEntry {
    pub id: AgentHarnessId,
    pub display_name: String,
    pub logo: String,
    pub available: bool,
    pub compatible: bool,
    pub version: Option<String>,
    pub minimum_version: String,
    pub diagnostic: String,
}

/// One competitive seat selected by the local operator.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentSeatSelection {
    Agent {
        harness: AgentHarnessId,
        #[serde(default)]
        model: Option<String>,
    },
    Human {
        name: String,
    },
}

/// Agent-first local match creation request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAgentMatchRequest {
    pub language: Language,
    #[serde(default)]
    pub mode: GameMode,
    pub seats: Vec<AgentSeatSelection>,
    pub idempotency_key: IdempotencyKey,
}

/// Public orchestration state for one seat. It never contains rack or prompt data.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentSeatStatusKind {
    Queued,
    Starting,
    Ready,
    Thinking,
    WaitingForHuman,
    Finished,
    Failed { code: String },
}

/// Public participant and lifecycle metadata for one seat.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSeatStatus {
    pub seat: Seat,
    pub participant: AgentSeatSelection,
    pub status: AgentSeatStatusKind,
}

/// Public, content-free status for an agent-managed match.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentMatchStatus {
    pub schema_version: u16,
    pub game_id: GameId,
    pub language: Language,
    pub mode: GameMode,
    pub phase: GamePhase,
    pub orchestration: AgentMatchLifecycle,
    pub version: u64,
    pub current_seat: Seat,
    pub scores: Vec<i32>,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
    pub seats: Vec<AgentSeatStatus>,
}

/// Whether the local runner is available for a persisted match.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMatchLifecycle {
    Active,
    Finished,
    Interrupted,
}

/// Spectator-only, bounded orchestration activity for debugging a local match.
///
/// Messages are derived from lifecycle state, already-redacted diagnostics, and
/// the agent's explicit visible output. Prompts, tool arguments/results, raw
/// command lines, credentials, and hidden reasoning are never represented.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentMatchActivity {
    pub schema_version: u16,
    pub game_id: GameId,
    pub events: Vec<AgentActivityEvent>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentActivityEvent {
    pub sequence: u64,
    pub at_unix_ms: i64,
    pub seat: Option<Seat>,
    pub kind: AgentActivityKind,
    pub message: String,
    pub turn_id: Option<String>,
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentActivityKind {
    MatchStarted,
    AgentStarting,
    AgentReady,
    AgentFailed,
    TurnStarted,
    ToolCalled,
    Diagnostic,
    TurnCompleted,
    TurnFailed,
    AgentFinished,
    MatchFinished,
}

/// Created match plus one-time observer and optional human-seat capabilities.
#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAgentMatchResponse {
    pub game_id: GameId,
    pub public: PublicProjection,
    pub public_capability: String,
    pub spectator_capability: String,
    pub human_capability: Option<String>,
    pub status: AgentMatchStatus,
}

/// Privacy-safe local operator index of current and completed matches.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentMatchList {
    pub matches: Vec<AgentMatchStatus>,
}

/// Fresh spectator access issued from the trusted local operator boundary.
#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentMatchRecovery {
    pub game_id: GameId,
    pub spectator_capability: String,
}

/// Trusted local process configuration. Browser requests cannot override it.
#[derive(Clone)]
pub struct AgentMatchManagerConfig {
    pub executables: HarnessExecutables,
    pub workspace_root: PathBuf,
    pub mcp_origin: String,
    pub codex_auth_file: Option<PathBuf>,
    pub match_repository: Option<SqliteLocalMatchRepository>,
}

impl fmt::Debug for AgentMatchManagerConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentMatchManagerConfig")
            .field("executables", &self.executables)
            .field("workspace_root", &"<local-data>")
            .field("mcp_origin", &self.mcp_origin)
            .field(
                "codex_auth_file",
                &self.codex_auth_file.as_ref().map(|_| "<configured>"),
            )
            .field(
                "match_repository",
                &self.match_repository.as_ref().map(|_| "<configured>"),
            )
            .finish()
    }
}

impl Default for AgentMatchManagerConfig {
    fn default() -> Self {
        Self {
            executables: HarnessExecutables::default(),
            workspace_root: std::env::temp_dir().join("word-arena-agent-runs"),
            mcp_origin: "http://127.0.0.1:3000".to_owned(),
            codex_auth_file: None,
            match_repository: None,
        }
    }
}

#[derive(Debug)]
struct AgentMatchManagerInner {
    config: AgentMatchManagerConfig,
    matches: RwLock<HashMap<GameId, AgentMatchStatus>>,
    activity: RwLock<HashMap<GameId, AgentActivityLog>>,
    capabilities: RwLock<HashMap<(GameId, u8), PendingAgentCapability>>,
}

#[derive(Debug, Default)]
struct AgentActivityLog {
    next_sequence: u64,
    events: VecDeque<AgentActivityEvent>,
}

#[derive(Debug)]
struct PendingAgentCapability {
    raw: String,
    expires_at: UnixMillis,
}

/// Local agent discovery and match-lifecycle registry.
#[derive(Clone, Debug)]
pub struct AgentMatchManager {
    inner: Arc<AgentMatchManagerInner>,
}

impl AgentMatchManager {
    #[must_use]
    pub fn new(config: AgentMatchManagerConfig) -> Self {
        Self {
            inner: Arc::new(AgentMatchManagerInner {
                config,
                matches: RwLock::new(HashMap::new()),
                activity: RwLock::new(HashMap::new()),
                capabilities: RwLock::new(HashMap::new()),
            }),
        }
    }

    /// Restores persisted finished matches and marks abandoned live runners as interrupted.
    ///
    /// # Errors
    ///
    /// Returns a stable archive category when persisted status cannot be read,
    /// strictly decoded, or updated after detecting an interrupted runner.
    pub async fn restore(&self) -> Result<(), &'static str> {
        let Some(repository) = &self.inner.config.match_repository else {
            return Ok(());
        };
        let records = repository.list().await.map_err(|_| "match_archive_read")?;
        let mut restored = HashMap::with_capacity(records.len());
        for stored in records {
            let mut status: AgentMatchStatus =
                serde_json::from_slice(&stored.status_json).map_err(|_| "match_archive_corrupt")?;
            if status.schema_version != API_SCHEMA_VERSION
                || status.game_id.as_str() != stored.game_id
                || status.created_at_unix_ms != stored.created_at_ms
                || status.updated_at_unix_ms != stored.updated_at_ms
            {
                return Err("match_archive_corrupt");
            }
            if status.orchestration == AgentMatchLifecycle::Active {
                status.orchestration = AgentMatchLifecycle::Interrupted;
                status.updated_at_unix_ms = unix_millis();
                for seat in &mut status.seats {
                    if matches!(seat.participant, AgentSeatSelection::Agent { .. }) {
                        seat.status = AgentSeatStatusKind::Failed {
                            code: "server_restarted".to_owned(),
                        };
                    }
                }
                persist_match(repository, &status)
                    .await
                    .map_err(|()| "match_archive_write")?;
            }
            restored.insert(status.game_id.clone(), status);
        }
        *self.inner.matches.write().await = restored;
        Ok(())
    }

    /// Returns every local match newest first without exposing capabilities or racks.
    pub async fn list(&self) -> Vec<AgentMatchStatus> {
        let mut matches = self
            .inner
            .matches
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| {
            right
                .created_at_unix_ms
                .cmp(&left.created_at_unix_ms)
                .then_with(|| right.game_id.as_str().cmp(left.game_id.as_str()))
        });
        matches
    }

    /// Probes all supported executables without issuing a model request.
    pub async fn catalog(&self) -> Vec<AgentCatalogEntry> {
        let config = self.inner.config.clone();
        tokio::task::spawn_blocking(move || {
            AgentHarnessId::ALL
                .into_iter()
                .map(|harness| probe_harness(harness, &config.executables))
                .collect()
        })
        .await
        .unwrap_or_else(|_| AgentHarnessId::ALL.into_iter().map(probe_failure).collect())
    }

    pub(crate) async fn insert(&self, status: AgentMatchStatus) -> Result<(), &'static str> {
        let game_id = status.game_id.clone();
        if let Some(repository) = &self.inner.config.match_repository {
            persist_match(repository, &status)
                .await
                .map_err(|()| "match_archive_write")?;
        }
        self.inner
            .matches
            .write()
            .await
            .insert(game_id.clone(), status);
        self.inner
            .activity
            .write()
            .await
            .entry(game_id.clone())
            .or_default();
        self.record_activity(
            &game_id,
            None,
            AgentActivityKind::MatchStarted,
            "Agent-managed match created",
            None,
            None,
        )
        .await;
        Ok(())
    }

    pub(crate) async fn status(&self, game_id: &GameId) -> Option<AgentMatchStatus> {
        self.inner.matches.read().await.get(game_id).cloned()
    }

    pub(crate) async fn activity(&self, game_id: &GameId) -> Option<AgentMatchActivity> {
        let activity = self.inner.activity.read().await;
        let log = activity.get(game_id)?;
        Some(AgentMatchActivity {
            schema_version: API_SCHEMA_VERSION,
            game_id: game_id.clone(),
            events: log.events.iter().cloned().collect(),
        })
    }

    async fn record_activity(
        &self,
        game_id: &GameId,
        seat: Option<Seat>,
        kind: AgentActivityKind,
        message: impl AsRef<str>,
        turn_id: Option<String>,
        duration_ms: Option<u64>,
    ) {
        let mut activity = self.inner.activity.write().await;
        let log = activity.entry(game_id.clone()).or_default();
        let event = AgentActivityEvent {
            sequence: log.next_sequence,
            at_unix_ms: unix_millis(),
            seat,
            kind,
            message: bounded_activity_message(message.as_ref()),
            turn_id,
            duration_ms,
        };
        log.next_sequence = log.next_sequence.saturating_add(1);
        if log.events.len() == MAX_ACTIVITY_EVENTS {
            log.events.pop_front();
        }
        log.events.push_back(event);
    }

    async fn record_agent_failure(
        &self,
        game_id: &GameId,
        seat: Option<Seat>,
        code: &str,
        message: &str,
    ) {
        self.record_activity(
            game_id,
            seat,
            AgentActivityKind::AgentFailed,
            message,
            None,
            None,
        )
        .await;
        tracing::warn!(
            game_id = %game_id,
            seat = seat.map(seat_number),
            failure_code = code,
            "agent orchestration failed"
        );
    }

    pub(crate) async fn register_agent_capability(
        &self,
        game_id: GameId,
        seat: Seat,
        raw: String,
        expires_at: UnixMillis,
    ) {
        self.inner.capabilities.write().await.insert(
            (game_id, seat_number(seat)),
            PendingAgentCapability { raw, expires_at },
        );
    }

    pub(crate) fn start(
        &self,
        state: Arc<ServerState>,
        access: CreatedGameAccess,
        spectator_capability: String,
    ) {
        let manager = self.clone();
        tokio::spawn(async move {
            manager.run_match(state, access, spectator_capability).await;
        });
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the orchestration narrative keeps startup, turn ownership, and terminal cleanup auditable"
    )]
    async fn run_match(
        &self,
        state: Arc<ServerState>,
        access: CreatedGameAccess,
        spectator_capability: String,
    ) {
        let game_id = access.public.game_id().clone();
        let Some(initial) = self.status(&game_id).await else {
            return;
        };
        let Ok(policy) = ForbiddenAuthorityFingerprint::new(
            ForbiddenAuthorityKind::HumanSpectator,
            spectator_capability,
        )
        .and_then(|fingerprint| ForbiddenAuthorityPolicy::new([fingerprint])) else {
            self.fail_agents(&game_id, "authority_policy").await;
            self.record_agent_failure(
                &game_id,
                None,
                "authority_policy",
                "Agent authority boundary could not be initialized",
            )
            .await;
            self.abort_match(&state, &access, "authority-policy").await;
            return;
        };
        let authority =
            AuthorityBoundaryConfig::new(Arc::new(policy), Arc::new(TracingAuthorityAudit));
        let Ok(workspace_config) =
            WorkspaceManagerConfig::detect(self.inner.config.workspace_root.clone(), authority)
        else {
            self.fail_agents(&game_id, "sandbox_unavailable").await;
            self.record_agent_failure(
                &game_id,
                None,
                "sandbox_unavailable",
                "Agent sandbox is unavailable",
            )
            .await;
            self.abort_match(&state, &access, "sandbox-unavailable")
                .await;
            return;
        };
        let Ok(workspace_manager) =
            SeatWorkspaceManager::new(workspace_config, Arc::new(SystemDriverClock))
        else {
            self.fail_agents(&game_id, "workspace_unavailable").await;
            self.record_agent_failure(
                &game_id,
                None,
                "workspace_unavailable",
                "Agent workspace manager is unavailable",
            )
            .await;
            self.abort_match(&state, &access, "workspace-unavailable")
                .await;
            return;
        };
        let catalog = self.catalog().await;
        let mut agents: Vec<Option<RunningAgent>> = std::iter::repeat_with(|| None)
            .take(initial.seats.len())
            .collect();
        for index in 0..initial.seats.len() {
            let selection = &initial.seats[index].participant;
            let AgentSeatSelection::Agent { harness, model } = selection else {
                continue;
            };
            let seat = Seat::ALL[index];
            self.set_seat_status(&game_id, seat, AgentSeatStatusKind::Starting)
                .await;
            self.record_activity(
                &game_id,
                Some(seat),
                AgentActivityKind::AgentStarting,
                format!("Starting {}", harness.display_name()),
                None,
                None,
            )
            .await;
            tracing::info!(
                game_id = %game_id,
                seat = seat_number(seat),
                harness = harness.id(),
                "starting agent seat"
            );
            let Some(entry) = catalog
                .iter()
                .find(|entry| entry.id == *harness && entry.compatible)
            else {
                self.set_seat_status(
                    &game_id,
                    seat,
                    AgentSeatStatusKind::Failed {
                        code: "agent_unavailable".to_owned(),
                    },
                )
                .await;
                self.record_agent_failure(
                    &game_id,
                    Some(seat),
                    "agent_unavailable",
                    "Selected agent is unavailable or incompatible",
                )
                .await;
                self.abort_match(&state, &access, "agent-unavailable").await;
                self.finish_agents(&game_id, &mut agents).await;
                return;
            };
            match self
                .start_agent(
                    &workspace_manager,
                    &game_id,
                    seat,
                    *harness,
                    model.as_deref(),
                    entry.version.as_deref().unwrap_or_default(),
                )
                .await
            {
                Ok(agent) => {
                    agents[index] = Some(agent);
                    self.set_seat_status(&game_id, seat, AgentSeatStatusKind::Ready)
                        .await;
                    self.record_activity(
                        &game_id,
                        Some(seat),
                        AgentActivityKind::AgentReady,
                        format!("{} is ready", harness.display_name()),
                        None,
                        None,
                    )
                    .await;
                    tracing::info!(
                        game_id = %game_id,
                        seat = seat_number(seat),
                        harness = harness.id(),
                        "agent seat ready"
                    );
                }
                Err(code) => {
                    self.set_seat_status(
                        &game_id,
                        seat,
                        AgentSeatStatusKind::Failed {
                            code: code.to_owned(),
                        },
                    )
                    .await;
                    self.record_agent_failure(
                        &game_id,
                        Some(seat),
                        code,
                        &format!("Agent startup failed: {code}"),
                    )
                    .await;
                    self.abort_match(&state, &access, code).await;
                    self.finish_agents(&game_id, &mut agents).await;
                    return;
                }
            }
        }

        loop {
            let Ok(view) = state
                .runtime()
                .service()
                .public_game(
                    &access.public,
                    PublicGameQuery {
                        game_id: game_id.clone(),
                    },
                )
                .await
            else {
                self.fail_agents(&game_id, "game_unavailable").await;
                self.record_agent_failure(
                    &game_id,
                    None,
                    "game_unavailable",
                    "Authoritative game state is unavailable",
                )
                .await;
                break;
            };
            let public = view.game;
            self.update_public(&game_id, &public).await;
            if public.state.phase == GamePhase::Finished {
                self.finish_agents(&game_id, &mut agents).await;
                self.record_activity(
                    &game_id,
                    None,
                    AgentActivityKind::MatchFinished,
                    "Match finished",
                    None,
                    None,
                )
                .await;
                tracing::info!(game_id = %game_id, "agent-managed match finished");
                break;
            }
            let seat = public.state.current_player;
            let index = seat_index(seat);
            let Some(agent) = agents[index].as_mut() else {
                self.set_seat_status(&game_id, seat, AgentSeatStatusKind::WaitingForHuman)
                    .await;
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                continue;
            };
            self.set_seat_status(&game_id, seat, AgentSeatStatusKind::Thinking)
                .await;
            let before = public.state.version;
            let request = TurnRequest {
                turn_id: format!("{}-{before}", seat_number(seat)),
                visible_input: turn_prompt(&game_id, seat, before),
            };
            let turn_id = request.turn_id.clone();
            let started_at = unix_millis();
            self.record_activity(
                &game_id,
                Some(seat),
                AgentActivityKind::TurnStarted,
                format!("Turn {before} started"),
                Some(turn_id.clone()),
                None,
            )
            .await;
            tracing::info!(
                game_id = %game_id,
                seat = seat_number(seat),
                turn_id,
                game_version = before,
                "agent turn started"
            );
            let turn_result = agent
                .driver
                .request_turn(request, &agent.cancellation)
                .await;
            let duration_ms = elapsed_millis(started_at);
            for diagnostic in agent.take_new_diagnostics() {
                self.record_activity(
                    &game_id,
                    Some(seat),
                    AgentActivityKind::Diagnostic,
                    format!("{}: {}", diagnostic.code, diagnostic.visible_text),
                    Some(turn_id.clone()),
                    None,
                )
                .await;
            }
            let output = match turn_result {
                Ok(output) => output,
                Err(error) => {
                    self.set_seat_status(
                        &game_id,
                        seat,
                        AgentSeatStatusKind::Failed {
                            code: "agent_turn_failed".to_owned(),
                        },
                    )
                    .await;
                    self.record_activity(
                        &game_id,
                        Some(seat),
                        AgentActivityKind::TurnFailed,
                        format!("Agent turn failed: {error}"),
                        Some(turn_id.clone()),
                        Some(duration_ms),
                    )
                    .await;
                    tracing::warn!(
                        game_id = %game_id,
                        seat = seat_number(seat),
                        turn_id,
                        duration_ms,
                        error = %error,
                        "agent turn failed"
                    );
                    self.resign(&state, &access, seat, "agent-turn-failed")
                        .await;
                    continue;
                }
            };
            for tool_call in &output.tool_calls {
                self.record_activity(
                    &game_id,
                    Some(seat),
                    AgentActivityKind::ToolCalled,
                    format!("Called {}", tool_call.tool),
                    Some(turn_id.clone()),
                    None,
                )
                .await;
            }
            self.record_activity(
                &game_id,
                Some(seat),
                AgentActivityKind::TurnCompleted,
                if output.visible_output.trim().is_empty() {
                    "Agent turn completed"
                } else {
                    output.visible_output.trim()
                },
                Some(turn_id.clone()),
                Some(duration_ms),
            )
            .await;
            tracing::info!(
                game_id = %game_id,
                seat = seat_number(seat),
                turn_id,
                duration_ms,
                tool_calls = output.tool_calls.len(),
                "agent turn process completed"
            );
            let after = state
                .runtime()
                .service()
                .public_game(
                    &access.public,
                    PublicGameQuery {
                        game_id: game_id.clone(),
                    },
                )
                .await;
            match after {
                Ok(view) if view.game.state.version > before => {
                    self.update_public(&game_id, &view.game).await;
                    self.set_seat_status(&game_id, seat, AgentSeatStatusKind::Ready)
                        .await;
                    state
                        .publish_game_update(game_id.clone(), view.game.state.version)
                        .await;
                }
                _ => {
                    self.set_seat_status(
                        &game_id,
                        seat,
                        AgentSeatStatusKind::Failed {
                            code: "agent_did_not_move".to_owned(),
                        },
                    )
                    .await;
                    self.record_activity(
                        &game_id,
                        Some(seat),
                        AgentActivityKind::TurnFailed,
                        "Agent completed without committing an authoritative move",
                        Some(turn_id),
                        Some(duration_ms),
                    )
                    .await;
                    tracing::warn!(
                        game_id = %game_id,
                        seat = seat_number(seat),
                        game_version = before,
                        "agent completed without committing a move"
                    );
                    self.resign(&state, &access, seat, "agent-did-not-move")
                        .await;
                }
            }
        }
    }

    async fn start_agent(
        &self,
        workspace_manager: &SeatWorkspaceManager,
        game_id: &GameId,
        seat: Seat,
        harness: AgentHarnessId,
        model: Option<&str>,
        version: &str,
    ) -> Result<RunningAgent, &'static str> {
        let pending = self
            .inner
            .capabilities
            .write()
            .await
            .remove(&(game_id.clone(), seat_number(seat)))
            .ok_or("seat_capability_missing")?;
        let now = unix_millis();
        let capability = SeatCapability::new(pending.raw, pending.expires_at.0, now)
            .map_err(|_| "seat_capability_invalid")?;
        let run_id = format!("run-{game_id}-{}", seat_number(seat));
        let manifest =
            build_manifest(harness, model, version, seat).map_err(|_| "agent_manifest_invalid")?;
        let mcp_url = format!(
            "{}/api/v1/games/{game_id}/mcp",
            self.inner.config.mcp_origin.trim_end_matches('/')
        );
        let mut lease = workspace_manager
            .allocate(
                &manifest,
                SeatWorkspaceRequest {
                    run_id: run_id.clone(),
                    seat_id: format!("seat-{}", seat_number(seat)),
                    game_id: game_id.to_string(),
                    mcp_url,
                    capability,
                },
            )
            .map_err(|_| "workspace_allocation_failed")?;
        if harness == AgentHarnessId::Codex {
            let source = self
                .inner
                .config
                .codex_auth_file
                .as_ref()
                .ok_or("provider_auth_missing")?;
            let auth = fs::read(source).map_err(|_| "provider_auth_missing")?;
            lease
                .install_provider_json("auth.json", &auth)
                .map_err(|_| "provider_auth_invalid")?;
        }
        let runtime = lease.harness_runtime(self.inner.config.executables.clone());
        let mut driver = SupportedAgentDriver::new(
            run_id,
            &manifest,
            runtime,
            lease.process_adapter(),
            Arc::new(SystemDriverClock),
        )
        .map_err(|_| "agent_driver_invalid")?;
        let cancellation = CancellationToken::new();
        driver
            .start(&cancellation)
            .await
            .map_err(|_| "agent_start_failed")?;
        Ok(RunningAgent {
            seat,
            driver,
            lease,
            cancellation,
            reported_diagnostics: 0,
        })
    }

    async fn set_seat_status(&self, game_id: &GameId, seat: Seat, status: AgentSeatStatusKind) {
        if let Some(record) = self.inner.matches.write().await.get_mut(game_id) {
            record.seats[seat_index(seat)].status = status;
            record.updated_at_unix_ms = unix_millis();
        }
        self.persist_status(game_id).await;
    }

    async fn update_public(&self, game_id: &GameId, public: &PublicProjection) {
        if let Some(record) = self.inner.matches.write().await.get_mut(game_id) {
            record.phase = public.state.phase;
            record.orchestration = if public.state.phase == GamePhase::Finished {
                AgentMatchLifecycle::Finished
            } else {
                record.orchestration
            };
            record.version = public.state.version;
            record.current_seat = public.state.current_player;
            record.scores = public
                .state
                .scores
                .iter()
                .copied()
                .map(word_arena_engine::Score::value)
                .collect();
            record.updated_at_unix_ms = unix_millis();
        }
        self.persist_status(game_id).await;
    }

    async fn fail_agents(&self, game_id: &GameId, code: &str) {
        if let Some(record) = self.inner.matches.write().await.get_mut(game_id) {
            for seat in &mut record.seats {
                if matches!(seat.participant, AgentSeatSelection::Agent { .. }) {
                    seat.status = AgentSeatStatusKind::Failed {
                        code: code.to_owned(),
                    };
                }
            }
            record.updated_at_unix_ms = unix_millis();
        }
        self.persist_status(game_id).await;
    }

    async fn finish_agents(&self, game_id: &GameId, agents: &mut [Option<RunningAgent>]) {
        let outcomes = self.inner.matches.read().await.get(game_id).map_or_else(
            || vec![WorkspaceOutcome::Failed; agents.len()],
            |record| {
                record
                    .seats
                    .iter()
                    .map(|seat| workspace_outcome(Some(&seat.status)))
                    .collect()
            },
        );
        for agent in agents.iter_mut().filter_map(Option::take) {
            let seat = agent.seat;
            agent.finish(outcomes[seat_index(seat)]).await;
            self.record_activity(
                game_id,
                Some(seat),
                AgentActivityKind::AgentFinished,
                "Agent process stopped",
                None,
                None,
            )
            .await;
        }
        if let Some(record) = self.inner.matches.write().await.get_mut(game_id) {
            for seat in &mut record.seats {
                if !matches!(seat.status, AgentSeatStatusKind::Failed { .. }) {
                    seat.status = AgentSeatStatusKind::Finished;
                }
            }
            record.updated_at_unix_ms = unix_millis();
        }
        self.persist_status(game_id).await;
    }

    async fn persist_status(&self, game_id: &GameId) {
        let Some(repository) = &self.inner.config.match_repository else {
            return;
        };
        let Some(status) = self.inner.matches.read().await.get(game_id).cloned() else {
            return;
        };
        if persist_match(repository, &status).await.is_err() {
            tracing::error!(game_id = %game_id, "local match status persistence failed");
        }
    }

    async fn resign(
        &self,
        state: &ServerState,
        access: &CreatedGameAccess,
        seat: Seat,
        reason: &str,
    ) {
        let Ok(public) = state
            .runtime()
            .service()
            .public_game(
                &access.public,
                PublicGameQuery {
                    game_id: access.public.game_id().clone(),
                },
            )
            .await
        else {
            return;
        };
        if public.game.state.phase == GamePhase::Finished
            || public.game.state.current_player != seat
        {
            return;
        }
        let Some(credential) = access.seat(seat) else {
            return;
        };
        let key = IdempotencyKey::new(format!(
            "agent-orchestrator-{reason}-{}",
            public.game.state.version
        ));
        let Ok(key) = key else {
            return;
        };
        let result = state
            .runtime()
            .service()
            .act(
                credential,
                GameActionCommand {
                    game_id: access.public.game_id().clone(),
                    expected_version: public.game.state.version,
                    turn: Turn {
                        number: public.game.state.version,
                        seat,
                    },
                    idempotency_key: key,
                    action: Move::Resign,
                },
            )
            .await;
        if let Ok(result) = result {
            self.update_public(access.public.game_id(), &result.game.public)
                .await;
            state
                .publish_game_update(
                    access.public.game_id().clone(),
                    result.game.public.state.version,
                )
                .await;
        }
    }

    async fn abort_match(&self, state: &ServerState, access: &CreatedGameAccess, reason: &str) {
        let Ok(public) = state
            .runtime()
            .service()
            .public_game(
                &access.public,
                PublicGameQuery {
                    game_id: access.public.game_id().clone(),
                },
            )
            .await
        else {
            return;
        };
        self.resign(state, access, public.game.state.current_player, reason)
            .await;
    }
}

#[derive(Debug)]
struct RunningAgent {
    seat: Seat,
    driver: SupportedAgentDriver,
    lease: SeatWorkspaceLease,
    cancellation: CancellationToken,
    reported_diagnostics: usize,
}

impl RunningAgent {
    fn take_new_diagnostics(&mut self) -> Vec<DiagnosticRecord> {
        let diagnostics = &self.driver.telemetry().diagnostics;
        let new = diagnostics
            .get(self.reported_diagnostics..)
            .unwrap_or_default()
            .to_vec();
        self.reported_diagnostics = diagnostics.len();
        new
    }

    async fn finish(mut self, outcome: WorkspaceOutcome) {
        self.cancellation.cancel();
        let reason = if outcome == WorkspaceOutcome::Failed {
            TerminationReason::GameEnded
        } else {
            TerminationReason::Completed
        };
        let _ = self.driver.terminate(reason).await;
        let _ = self.lease.finish(outcome);
    }
}

fn workspace_outcome(status: Option<&AgentSeatStatusKind>) -> WorkspaceOutcome {
    if matches!(status, Some(AgentSeatStatusKind::Failed { .. })) {
        WorkspaceOutcome::Failed
    } else if status.is_some() {
        WorkspaceOutcome::Completed
    } else {
        WorkspaceOutcome::Failed
    }
}

#[derive(Debug)]
struct TracingAuthorityAudit;

impl AuthorityBoundaryAuditSink for TracingAuthorityAudit {
    fn record(&self, event: AuthorityBoundaryAuditEvent) -> Result<(), AuthorityAuditError> {
        tracing::warn!(
            run_id = %event.run_id,
            seat_id = %event.seat_id,
            authority = ?event.authority,
            surface = ?event.surface,
            "forbidden authority blocked at agent boundary"
        );
        Ok(())
    }
}

fn build_manifest(
    harness: AgentHarnessId,
    model: Option<&str>,
    version: &str,
    seat: Seat,
) -> Result<ValidatedAgentManifest, word_arena_agent_runtime::ManifestError> {
    let model_source = model.map_or(ModelSource::HarnessDefault, |_| ModelSource::Provider {
        provider: match harness {
            AgentHarnessId::Codex => ModelProvider::OpenAi,
            AgentHarnessId::ClaudeCode => ModelProvider::Anthropic,
            AgentHarnessId::Cline => ModelProvider::OpenRouter,
            AgentHarnessId::Pi => ModelProvider::Google,
        },
    });
    let harness_config = match harness {
        AgentHarnessId::Codex => HarnessConfig::Codex {
            version: version.to_owned(),
        },
        AgentHarnessId::ClaudeCode => HarnessConfig::ClaudeCode {
            version: version.to_owned(),
        },
        AgentHarnessId::Cline => HarnessConfig::Cline {
            version: version.to_owned(),
        },
        AgentHarnessId::Pi => HarnessConfig::Pi {
            version: version.to_owned(),
        },
    };
    let environment_digest = hex_digest(format!(
        "local-agent:{}:{version}:{}:{}",
        harness.id(),
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    ValidatedAgentManifest::new(AgentManifest {
        schema_version: AGENT_MANIFEST_SCHEMA_VERSION,
        name: harness.display_name().to_owned(),
        harness: harness_config,
        model: ModelConfig {
            id: model.unwrap_or("harness-default").to_owned(),
            source: model_source,
        },
        prompt: PromptIdentity {
            format_version: 1,
            sha256: hex_digest(turn_prompt_template()),
        },
        tool_policy: ToolPolicy {
            policy_version: 1,
            allowed_tools: BTreeSet::from(["mcp.word_arena".to_owned()]),
            denied_tools: BTreeSet::from(["shell.network".to_owned()]),
            network: NetworkPolicy::McpOnly,
        },
        environment: EnvironmentIdentity {
            image: format!(
                "local/word-arena-{}@sha256:{environment_digest}",
                harness.id()
            ),
            platform: local_platform(),
        },
        driver_version: env!("CARGO_PKG_VERSION").to_owned(),
        workspace: WorkspacePolicy {
            policy_version: 1,
            persistence: WorkspacePersistence::PersistentForRun,
            retention: WorkspaceRetention::RetainOnFailure,
            max_bytes: 1_073_741_824,
        },
        budgets: ResourceBudgets {
            wall_time_ms: 600_000,
            cpu_time_ms: 480_000,
            memory_bytes: 2_147_483_648,
            network_bytes: 10_485_760,
            input_tokens: 100_000,
            output_tokens: 20_000,
            attempts: 3,
            tool_calls: 256,
            output_bytes: 10_485_760,
            cost_microusd: 5_000_000,
        },
        labels: BTreeMap::from([("seat".to_owned(), seat_number(seat).to_string())]),
    })
}

fn turn_prompt_template() -> &'static str {
    "You are playing one Word Arena turn. Use only the word_arena MCP server. Observe the game and your private rack, inspect the rules when useful, then submit exactly one legal action. Keep trying legal actions until one is accepted. Never ask for human input and never stop before the referee accepts a move."
}

fn turn_prompt(game_id: &GameId, seat: Seat, version: u64) -> String {
    format!(
        "{}\n\nGame: {game_id}\nSeat: {}\nAuthoritative version: {version}",
        turn_prompt_template(),
        seat_number(seat)
    )
}

fn local_platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        value => value,
    };
    let architecture = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        value => value,
    };
    format!("{os}/{architecture}")
}

fn hex_digest(value: impl AsRef<[u8]>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(value.as_ref());
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn elapsed_millis(started_at: i64) -> u64 {
    u64::try_from(unix_millis().saturating_sub(started_at)).unwrap_or_default()
}

fn bounded_activity_message(message: &str) -> String {
    let mut bounded = String::new();
    let mut truncated = false;
    for character in message.chars() {
        if bounded.chars().count() == MAX_ACTIVITY_MESSAGE_CHARS {
            truncated = true;
            break;
        }
        match character {
            '\n' | '\t' => bounded.push(character),
            value if value.is_control() => bounded.push('\u{fffd}'),
            value => bounded.push(value),
        }
    }
    if truncated {
        bounded.push('…');
    }
    bounded
}

const fn seat_number(seat: Seat) -> u8 {
    seat.number()
}

const fn seat_index(seat: Seat) -> usize {
    seat.index()
}

fn probe_harness(harness: AgentHarnessId, executables: &HarnessExecutables) -> AgentCatalogEntry {
    let executable = match harness {
        AgentHarnessId::Codex => &executables.codex,
        AgentHarnessId::ClaudeCode => &executables.claude_code,
        AgentHarnessId::Cline => &executables.cline,
        AgentHarnessId::Pi => &executables.pi,
    };
    let output = Command::new(executable).arg("--version").output();
    let Ok(output) = output else {
        return unavailable(harness);
    };
    if !output.status.success() {
        return unavailable(harness);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let Some(version) = parse_version(&text) else {
        return AgentCatalogEntry {
            id: harness,
            display_name: harness.display_name().to_owned(),
            logo: harness.logo().to_owned(),
            available: true,
            compatible: false,
            version: None,
            minimum_version: harness.minimum_version().to_owned(),
            diagnostic: "Installed, but its version could not be read".to_owned(),
        };
    };
    let minimum = Version::parse(harness.minimum_version()).expect("reviewed version is valid");
    let compatible = version >= minimum;
    AgentCatalogEntry {
        id: harness,
        display_name: harness.display_name().to_owned(),
        logo: harness.logo().to_owned(),
        available: true,
        compatible,
        version: Some(version.to_string()),
        minimum_version: harness.minimum_version().to_owned(),
        diagnostic: if compatible {
            "Compatible CLI".to_owned()
        } else {
            format!("Requires {} or newer", harness.minimum_version())
        },
    }
}

fn parse_version(value: &str) -> Option<Version> {
    value
        .split(|character: char| character.is_whitespace() || character == ',')
        .find_map(|candidate| Version::parse(candidate.trim_start_matches('v')).ok())
}

fn unavailable(harness: AgentHarnessId) -> AgentCatalogEntry {
    AgentCatalogEntry {
        id: harness,
        display_name: harness.display_name().to_owned(),
        logo: harness.logo().to_owned(),
        available: false,
        compatible: false,
        version: None,
        minimum_version: harness.minimum_version().to_owned(),
        diagnostic: "Not installed".to_owned(),
    }
}

fn probe_failure(harness: AgentHarnessId) -> AgentCatalogEntry {
    let mut entry = unavailable(harness);
    "Version probe failed".clone_into(&mut entry.diagnostic);
    entry
}

pub(crate) fn initial_status(
    game_id: GameId,
    language: Language,
    public: &PublicProjection,
    seats: &[AgentSeatSelection],
    created_at: UnixMillis,
) -> AgentMatchStatus {
    AgentMatchStatus {
        schema_version: API_SCHEMA_VERSION,
        game_id,
        language,
        mode: public.state.mode,
        phase: public.state.phase,
        orchestration: AgentMatchLifecycle::Active,
        version: public.state.version,
        current_seat: public.state.current_player,
        scores: public
            .state
            .scores
            .iter()
            .copied()
            .map(word_arena_engine::Score::value)
            .collect(),
        created_at_unix_ms: created_at.0,
        updated_at_unix_ms: created_at.0,
        seats: seats
            .iter()
            .enumerate()
            .map(|(index, participant)| AgentSeatStatus {
                seat: Seat::ALL[index],
                status: initial_seat_status(participant),
                participant: participant.clone(),
            })
            .collect(),
    }
}

async fn persist_match(
    repository: &SqliteLocalMatchRepository,
    status: &AgentMatchStatus,
) -> Result<(), ()> {
    let status_json = serde_json::to_vec(status).map_err(|_| ())?;
    repository
        .upsert(StoredLocalAgentMatch {
            game_id: status.game_id.as_str().to_owned(),
            status_schema_version: status.schema_version,
            status_json,
            created_at_ms: status.created_at_unix_ms,
            updated_at_ms: status.updated_at_unix_ms,
        })
        .await
        .map_err(|_| ())
}

fn initial_seat_status(selection: &AgentSeatSelection) -> AgentSeatStatusKind {
    match selection {
        AgentSeatSelection::Agent { .. } => AgentSeatStatusKind::Queued,
        AgentSeatSelection::Human { .. } => AgentSeatStatusKind::WaitingForHuman,
    }
}

#[cfg(test)]
mod tests {
    use word_arena_agent_runtime::WorkspaceOutcome;
    use word_arena_application::GameId;

    use super::{
        AgentActivityKind, AgentMatchManager, AgentMatchManagerConfig, AgentSeatStatusKind,
        MAX_ACTIVITY_EVENTS, parse_version, workspace_outcome,
    };

    #[test]
    fn retains_failed_agent_workspaces_for_postmortem() {
        assert_eq!(
            workspace_outcome(Some(&AgentSeatStatusKind::Failed {
                code: "agent_turn_failed".to_owned(),
            })),
            WorkspaceOutcome::Failed
        );
        assert_eq!(
            workspace_outcome(Some(&AgentSeatStatusKind::Finished)),
            WorkspaceOutcome::Completed
        );
        assert_eq!(workspace_outcome(None), WorkspaceOutcome::Failed);
    }

    #[test]
    fn parses_supported_cli_version_shapes() {
        assert_eq!(
            parse_version("codex-cli 0.144.1\n").unwrap().to_string(),
            "0.144.1"
        );
        assert_eq!(
            parse_version("claude v2.1.205\n").unwrap().to_string(),
            "2.1.205"
        );
        assert!(parse_version("unknown").is_none());
    }

    #[tokio::test]
    async fn activity_log_is_bounded_and_replaces_unsafe_controls() {
        let manager = AgentMatchManager::new(AgentMatchManagerConfig::default());
        let game_id = GameId::new("game-activity").unwrap();
        for index in 0..(MAX_ACTIVITY_EVENTS + 4) {
            manager
                .record_activity(
                    &game_id,
                    None,
                    AgentActivityKind::Diagnostic,
                    format!("event-{index}\0"),
                    None,
                    None,
                )
                .await;
        }

        let activity = manager.activity(&game_id).await.unwrap();
        assert_eq!(activity.events.len(), MAX_ACTIVITY_EVENTS);
        assert_eq!(activity.events[0].sequence, 4);
        assert_eq!(activity.events.last().unwrap().message, "event-259�");
        assert!(!activity.events.last().unwrap().message.contains('\0'));
    }
}
