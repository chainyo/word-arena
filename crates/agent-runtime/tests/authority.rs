#![cfg(unix)]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde_json::{Value, json};
use tempfile::TempDir;
use word_arena_agent_runtime::{
    AUTHORITY_BOUNDARY_AUDIT_SCHEMA_VERSION, AuthorityAuditError, AuthorityBoundaryAuditEvent,
    AuthorityBoundaryAuditSink, AuthorityBoundaryConfig, AuthorityBoundaryOutcome,
    AuthorityBoundarySurface, AuthorityPolicyError, DriverClock, ForbiddenAuthorityFingerprint,
    ForbiddenAuthorityKind, ForbiddenAuthorityPolicy, ManifestError, ProcessAdapter, ProcessError,
    ProcessSpec, SeatCapability, SeatSandboxBackend, SeatWorkspaceManager, SeatWorkspaceRequest,
    ValidatedAgentManifest, WorkspaceError, WorkspaceManagerConfig,
};

macro_rules! assert_not_serializable {
    ($value:ty) => {
        const _: fn() = || {
            struct Check<T: ?Sized>(std::marker::PhantomData<T>);
            trait AmbiguousIfSerializable<Marker> {
                fn marker() {}
            }
            impl<T: ?Sized> AmbiguousIfSerializable<()> for Check<T> {}
            impl<T: ?Sized + serde::Serialize> AmbiguousIfSerializable<u8> for Check<T> {}
            let _ = <Check<$value> as AmbiguousIfSerializable<_>>::marker;
        };
    };
}

assert_not_serializable!(SeatCapability);
assert_not_serializable!(ForbiddenAuthorityFingerprint);
assert_not_serializable!(ForbiddenAuthorityPolicy);

#[derive(Debug)]
struct FixedClock;

impl DriverClock for FixedClock {
    fn now_unix_ms(&self) -> i64 {
        1_000
    }
}

#[derive(Debug, Default)]
struct RecordingAudit {
    events: Mutex<Vec<AuthorityBoundaryAuditEvent>>,
}

impl RecordingAudit {
    fn events(&self) -> Vec<AuthorityBoundaryAuditEvent> {
        self.events.lock().unwrap().clone()
    }
}

impl AuthorityBoundaryAuditSink for RecordingAudit {
    fn record(&self, event: AuthorityBoundaryAuditEvent) -> Result<(), AuthorityAuditError> {
        self.events.lock().unwrap().push(event);
        Ok(())
    }
}

#[derive(Debug)]
struct FailingAudit;

impl AuthorityBoundaryAuditSink for FailingAudit {
    fn record(&self, _event: AuthorityBoundaryAuditEvent) -> Result<(), AuthorityAuditError> {
        Err(AuthorityAuditError)
    }
}

fn example() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/agents/generic-command-v1.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn manifest() -> ValidatedAgentManifest {
    let mut value = example();
    value["workspace"]["retention"] = json!("retain_on_failure");
    value["tool_policy"]["network"] = json!({ "kind": "deny" });
    ValidatedAgentManifest::from_json(&serde_json::to_vec(&value).unwrap()).unwrap()
}

fn token(character: char) -> String {
    format!(
        "wa_cap_v1.{}.{}",
        character.to_string().repeat(32),
        character.to_string().repeat(64)
    )
}

fn policy(spectator: &str, administrator: &str) -> Arc<ForbiddenAuthorityPolicy> {
    Arc::new(
        ForbiddenAuthorityPolicy::new([
            ForbiddenAuthorityFingerprint::new(
                ForbiddenAuthorityKind::HumanSpectator,
                spectator.to_owned(),
            )
            .unwrap(),
            ForbiddenAuthorityFingerprint::new(
                ForbiddenAuthorityKind::Administrator,
                administrator.to_owned(),
            )
            .unwrap(),
        ])
        .unwrap(),
    )
}

fn manager(
    temporary: &TempDir,
    policy: Arc<ForbiddenAuthorityPolicy>,
    audit: Arc<dyn AuthorityBoundaryAuditSink>,
) -> SeatWorkspaceManager {
    let executable = std::env::current_exe().unwrap();
    let runtime_root = executable.parent().unwrap().to_owned();
    SeatWorkspaceManager::new(
        WorkspaceManagerConfig {
            root: temporary.path().join("arena-workspaces"),
            safe_path: "/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin".to_owned(),
            sandbox: SeatSandboxBackend::Bubblewrap {
                executable,
                runtime_read_roots: vec![runtime_root],
            },
            authority: AuthorityBoundaryConfig::new(policy, audit),
        },
        Arc::new(FixedClock),
    )
    .unwrap()
}

fn request(raw: String) -> SeatWorkspaceRequest {
    SeatWorkspaceRequest {
        run_id: "run-authority".to_owned(),
        seat_id: "seat-one".to_owned(),
        game_id: "game-one".to_owned(),
        mcp_url: "http://127.0.0.1:3000/mcp/games/game-one".to_owned(),
        capability: SeatCapability::new(raw, 4_000, 1_000).unwrap(),
    }
}

#[test]
fn manifests_cannot_name_request_or_embed_privileged_authority() {
    let mut administrator_label = example();
    administrator_label["labels"]["requested_role"] = json!("administrator");
    let mut spectator_label = example();
    spectator_label["labels"]["access"] = json!("human-spectator");
    let mut command_argument = example();
    command_argument["harness"]["arguments"] = json!(["--administrator-capability=from-operator"]);
    for candidate in [administrator_label, spectator_label, command_argument] {
        assert!(matches!(
            ValidatedAgentManifest::from_json(&serde_json::to_vec(&candidate).unwrap()),
            Err(ManifestError::ForbiddenAuthority)
        ));
    }

    let mut raw_token = example();
    raw_token["labels"]["operator"] = json!(token('a'));
    assert!(matches!(
        ValidatedAgentManifest::from_json(&serde_json::to_vec(&raw_token).unwrap()),
        Err(ManifestError::SecretBearing)
    ));

    let canonical = String::from_utf8(manifest().canonical_json().to_vec()).unwrap();
    assert!(!canonical.contains("human_spectator"));
    assert!(!canonical.contains("administrator"));
    assert!(!canonical.contains("wa_cap_v1"));
}

#[test]
fn reviewed_authority_contract_matches_runtime() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/agent-authority-boundary-v1.json");
    let contract: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(
        contract["schema_version"],
        json!(AUTHORITY_BOUNDARY_AUDIT_SCHEMA_VERSION)
    );
    assert_eq!(
        contract["startup_scan_surfaces"],
        json!(["process_argument", "process_environment", "workspace_file"])
    );
    assert_eq!(contract["audit"]["outcome"], json!("denied_before_spawn"));
    assert_eq!(contract["audit"]["failure_behavior"], json!("fail_closed"));
}

#[test]
fn fingerprint_registry_is_digest_only_strict_and_non_disclosing() {
    assert!(matches!(
        ForbiddenAuthorityFingerprint::new(
            ForbiddenAuthorityKind::Administrator,
            "not-a-token".to_owned()
        ),
        Err(AuthorityPolicyError::InvalidCapability)
    ));
    let first =
        ForbiddenAuthorityFingerprint::new(ForbiddenAuthorityKind::Administrator, token('a'))
            .unwrap();
    let duplicate =
        ForbiddenAuthorityFingerprint::new(ForbiddenAuthorityKind::HumanSpectator, token('a'))
            .unwrap();
    assert!(matches!(
        ForbiddenAuthorityPolicy::new([first, duplicate]),
        Err(AuthorityPolicyError::DuplicateFingerprint)
    ));

    let fingerprint =
        ForbiddenAuthorityFingerprint::new(ForbiddenAuthorityKind::Administrator, token('b'))
            .unwrap();
    let debug = format!("{fingerprint:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains(&token('b')));
}

#[test]
fn privileged_environment_capability_is_audited_and_rejected_before_allocation() {
    let temporary = tempfile::tempdir().unwrap();
    let spectator = token('a');
    let administrator = token('b');
    let audit = Arc::new(RecordingAudit::default());
    let manager = manager(
        &temporary,
        policy(&spectator, &administrator),
        audit.clone(),
    );

    assert!(matches!(
        manager.allocate(&manifest(), request(spectator.clone())),
        Err(WorkspaceError::ForbiddenAuthority)
    ));
    assert!(!manager.root().join("runs/run-authority").exists());
    let events = audit.events();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        AuthorityBoundaryAuditEvent {
            schema_version: AUTHORITY_BOUNDARY_AUDIT_SCHEMA_VERSION,
            run_id: "run-authority".to_owned(),
            seat_id: "seat-one".to_owned(),
            authority: ForbiddenAuthorityKind::HumanSpectator,
            surface: AuthorityBoundarySurface::ProcessEnvironment,
            occurred_at_unix_ms: 1_000,
            outcome: AuthorityBoundaryOutcome::DeniedBeforeSpawn,
        }
    );
    let serialized = serde_json::to_string(&events).unwrap();
    assert!(!serialized.contains(&spectator));
    assert!(!serialized.contains(&administrator));
    assert!(!serialized.contains(&"a".repeat(32)));
    assert!(!serialized.contains(&"b".repeat(32)));
}

#[test]
fn audit_failure_also_fails_closed_before_allocation() {
    let temporary = tempfile::tempdir().unwrap();
    let spectator = token('a');
    let manager = manager(
        &temporary,
        policy(&spectator, &token('b')),
        Arc::new(FailingAudit),
    );
    assert!(matches!(
        manager.allocate(&manifest(), request(spectator)),
        Err(WorkspaceError::AuditUnavailable)
    ));
    assert!(!manager.root().join("runs/run-authority").exists());
}

#[tokio::test]
async fn process_arguments_and_recursive_workspace_files_are_denied_before_spawn() {
    let temporary = tempfile::tempdir().unwrap();
    let spectator = token('a');
    let administrator = token('b');
    let audit = Arc::new(RecordingAudit::default());
    let manager = manager(
        &temporary,
        policy(&spectator, &administrator),
        audit.clone(),
    );
    let lease = manager.allocate(&manifest(), request(token('c'))).unwrap();
    let adapter = lease.process_adapter();

    assert_spawn_denied(
        &adapter,
        lease.workspace(),
        vec![format!("--operator={administrator}")],
    )
    .await;
    assert_spawn_denied(
        &adapter,
        lease.workspace(),
        vec!["--human-spectator-capability".to_owned()],
    )
    .await;

    let nested = lease.workspace().join("solver/cache/deep");
    fs::create_dir_all(&nested).unwrap();
    fs::write(
        nested.join("candidate.bin"),
        format!("prefix:{spectator}:suffix"),
    )
    .unwrap();
    assert_spawn_denied(&adapter, lease.workspace(), Vec::new()).await;

    let events = audit.events();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].surface, AuthorityBoundarySurface::ProcessArgument);
    assert_eq!(events[0].authority, ForbiddenAuthorityKind::Administrator);
    assert_eq!(events[1].surface, AuthorityBoundarySurface::ProcessArgument);
    assert_eq!(events[1].authority, ForbiddenAuthorityKind::HumanSpectator);
    assert_eq!(events[2].surface, AuthorityBoundarySurface::WorkspaceFile);
    assert_eq!(events[2].authority, ForbiddenAuthorityKind::HumanSpectator);
    let serialized = serde_json::to_string(&events).unwrap();
    assert!(!serialized.contains(&spectator));
    assert!(!serialized.contains(&administrator));
}

async fn assert_spawn_denied(
    adapter: &Arc<dyn ProcessAdapter>,
    workspace: &Path,
    arguments: Vec<String>,
) {
    assert!(matches!(
        adapter
            .spawn(&ProcessSpec {
                executable: "/usr/bin/true".to_owned(),
                arguments,
                working_directory: Some(workspace.to_owned()),
            })
            .await,
        Err(ProcessError::Spawn)
    ));
}
