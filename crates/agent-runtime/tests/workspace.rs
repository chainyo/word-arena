#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    sync::Arc,
};

use serde_json::{Value, json};
use tempfile::TempDir;
use word_arena_agent_runtime::{
    AuthorityAuditError, AuthorityBoundaryAuditEvent, AuthorityBoundaryAuditSink,
    AuthorityBoundaryConfig, DriverClock, ForbiddenAuthorityPolicy, HarnessExecutables,
    MAX_SEAT_CAPABILITY_TTL_MS, ProcessAdapter, ProcessError, ProcessEvent, ProcessSpec,
    SEAT_CAPABILITY_ENV, SEAT_WORKSPACE_SCHEMA_VERSION, SeatCapability, SeatSandboxBackend,
    SeatWorkspaceManager, SeatWorkspaceRequest, SystemDriverClock, ValidatedAgentManifest,
    WorkspaceDisposition, WorkspaceError, WorkspaceManagerConfig, WorkspaceOutcome,
};

#[derive(Debug)]
struct FixedClock(i64);

impl DriverClock for FixedClock {
    fn now_unix_ms(&self) -> i64 {
        self.0
    }
}

#[derive(Debug)]
struct AcceptAudit;

impl AuthorityBoundaryAuditSink for AcceptAudit {
    fn record(&self, _event: AuthorityBoundaryAuditEvent) -> Result<(), AuthorityAuditError> {
        Ok(())
    }
}

fn authority_config() -> AuthorityBoundaryConfig {
    AuthorityBoundaryConfig::new(
        Arc::new(ForbiddenAuthorityPolicy::default()),
        Arc::new(AcceptAudit),
    )
}

fn example() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/agents/generic-command-v1.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn manifest(retention: &str) -> ValidatedAgentManifest {
    let mut value = example();
    value["workspace"]["retention"] = json!(retention);
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

fn capability(character: char) -> SeatCapability {
    SeatCapability::new(token(character), 4_000, 1_000).unwrap()
}

fn request(run: &str, seat: &str, game: &str, character: char) -> SeatWorkspaceRequest {
    SeatWorkspaceRequest {
        run_id: run.to_owned(),
        seat_id: seat.to_owned(),
        game_id: game.to_owned(),
        mcp_url: format!("http://127.0.0.1:3000/mcp/games/{game}"),
        capability: capability(character),
    }
}

fn test_backend() -> SeatSandboxBackend {
    SeatSandboxBackend::Bubblewrap {
        executable: PathBuf::from("/usr/bin/true"),
        runtime_read_roots: vec![PathBuf::from("/usr/bin")],
    }
}

fn manager(temporary: &TempDir, sandbox: SeatSandboxBackend) -> SeatWorkspaceManager {
    SeatWorkspaceManager::new(
        WorkspaceManagerConfig {
            root: temporary.path().join("arena-workspaces"),
            safe_path: "/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin".to_owned(),
            sandbox,
            authority: authority_config(),
        },
        Arc::new(FixedClock(1_000)),
    )
    .unwrap()
}

#[test]
fn reviewed_contract_matches_public_runtime_constants() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/agent-workspace-v1.json");
    let contract: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(
        contract["schema_version"],
        json!(SEAT_WORKSPACE_SCHEMA_VERSION)
    );
    assert_eq!(
        contract["credential"]["maximum_ttl_ms"],
        json!(MAX_SEAT_CAPABILITY_TTL_MS)
    );
    assert_eq!(
        contract["credential"]["environment"],
        json!(SEAT_CAPABILITY_ENV)
    );
    assert_eq!(
        contract["sandbox"]["unsupported_platform_behavior"],
        json!("fail_closed")
    );
    assert_eq!(
        contract["authority_boundary_contract"],
        json!("agent-authority-boundary-v1")
    );
}

#[test]
fn allocation_is_private_non_overlapping_and_contains_only_own_env_reference() {
    let temporary = tempfile::tempdir().unwrap();
    let manager = manager(&temporary, test_backend());
    let manifest = manifest("delete_on_finish");
    let first_token = token('a');
    let second_token = token('b');
    let first = manager
        .allocate(&manifest, request("run-1", "seat-one", "game-1", 'a'))
        .unwrap();
    let second = manager
        .allocate(&manifest, request("run-1", "seat-two", "game-1", 'b'))
        .unwrap();

    assert_ne!(first.root(), second.root());
    assert!(!first.root().starts_with(second.root()));
    assert!(!second.root().starts_with(first.root()));
    for path in [
        manager.root(),
        first.root(),
        first.workspace(),
        first.state_directory(),
    ] {
        assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o077, 0);
    }
    for file in all_files(first.root())
        .into_iter()
        .chain(all_files(second.root()))
    {
        let bytes = fs::read(&file).unwrap();
        assert!(
            !bytes
                .windows(first_token.len())
                .any(|window| window == first_token.as_bytes())
        );
        assert!(
            !bytes
                .windows(second_token.len())
                .any(|window| window == second_token.as_bytes())
        );
        assert_eq!(fs::metadata(file).unwrap().permissions().mode() & 0o077, 0);
    }
    let keys = first.environment_keys();
    assert!(keys.contains(&"WORD_ARENA_SEAT_CAPABILITY"));
    assert!(keys.contains(&"WORD_ARENA_MCP_CONFIG"));
    assert!(!keys.contains(&"DATABASE_URL"));
    assert!(!keys.iter().any(|key| key.contains("SPECTATOR")));
    assert!(!keys.iter().any(|key| key.contains("ADMIN")));
    let debug = format!("{first:?}");
    assert!(!debug.contains(&first_token));
    assert!(!debug.contains(&second_token));

    let codex_config = fs::read_to_string(first.state_directory().join("config.toml")).unwrap();
    let workspace_key = serde_json::to_string(&first.workspace().to_string_lossy()).unwrap();
    assert!(
        codex_config.contains(&format!(
            "[projects.{workspace_key}]\ntrust_level = \"trusted\""
        )),
        "Codex must receive its exact isolated workspace trust entry before the first turn"
    );
    assert_eq!(
        codex_config,
        fs::read_to_string(first.root().join("config/codex.toml")).unwrap()
    );

    assert_eq!(
        first.finish(WorkspaceOutcome::Completed).unwrap(),
        WorkspaceDisposition::Deleted
    );
    assert_eq!(
        second.finish(WorkspaceOutcome::Completed).unwrap(),
        WorkspaceDisposition::Deleted
    );
}

#[test]
fn resume_preserves_allowed_state_rotates_capability_and_validates_config() {
    let temporary = tempfile::tempdir().unwrap();
    let manifest = manifest("retain_on_failure");
    let initial_manager = manager(&temporary, test_backend());
    let lease = initial_manager
        .allocate(&manifest, request("run-resume", "seat-one", "game-1", 'a'))
        .unwrap();
    let root = lease.root().to_owned();
    fs::write(lease.workspace().join("solver-state.txt"), b"persistent\n").unwrap();
    assert!(matches!(
        lease.finish(WorkspaceOutcome::Failed).unwrap(),
        WorkspaceDisposition::Retained(path) if path == root
    ));

    let restarted = manager(&temporary, test_backend());
    let resumed = restarted
        .resume(&manifest, request("run-resume", "seat-one", "game-1", 'b'))
        .unwrap();
    assert_eq!(
        fs::read(resumed.workspace().join("solver-state.txt")).unwrap(),
        b"persistent\n"
    );
    assert_eq!(
        resumed.finish(WorkspaceOutcome::Completed).unwrap(),
        WorkspaceDisposition::Deleted
    );
    assert!(!root.exists());

    let lease = restarted
        .allocate(&manifest, request("run-tamper", "seat-one", "game-2", 'c'))
        .unwrap();
    let root = lease.root().to_owned();
    let mcp = lease.mcp_config().to_owned();
    lease.finish(WorkspaceOutcome::Failed).unwrap();
    fs::write(&mcp, b"{}\n").unwrap();
    assert!(matches!(
        restarted.resume(&manifest, request("run-tamper", "seat-one", "game-2", 'd')),
        Err(WorkspaceError::Corrupt)
    ));
    assert!(root.exists());
}

#[test]
fn traversal_collisions_symlinks_and_invalid_credentials_fail_closed() {
    let temporary = tempfile::tempdir().unwrap();
    let manager = manager(&temporary, test_backend());
    let manifest = manifest("retain_on_failure");
    assert!(matches!(
        manager.allocate(&manifest, request("../operator", "seat-one", "game-1", 'a')),
        Err(WorkspaceError::InvalidIdentifier)
    ));

    let lease = manager
        .allocate(&manifest, request("run-safe", "seat-one", "game-1", 'a'))
        .unwrap();
    assert!(matches!(
        manager.allocate(&manifest, request("run-safe", "seat-one", "game-1", 'b')),
        Err(WorkspaceError::AlreadyExists)
    ));
    let state = lease.state_directory().to_owned();
    let moved = lease.root().join("state-original");
    let root = lease.root().to_owned();
    lease.finish(WorkspaceOutcome::Failed).unwrap();
    fs::rename(&state, &moved).unwrap();
    symlink(temporary.path(), &state).unwrap();
    assert!(matches!(
        manager.resume(&manifest, request("run-safe", "seat-one", "game-1", 'c')),
        Err(WorkspaceError::Corrupt)
    ));
    assert!(root.exists());

    assert!(SeatCapability::new("not-a-capability".to_owned(), 2_000, 1_000).is_err());
    assert!(SeatCapability::new(token('d'), 5_000_001, 1_000).is_err());
    let mut invalid = request("run-url", "seat-one", "game-1", 'e');
    invalid.mcp_url = "https://user:password@example.com/mcp".to_owned();
    assert!(matches!(
        manager.allocate(&manifest, invalid),
        Err(WorkspaceError::InvalidMcpEndpoint)
    ));
    let mut query_secret = request("run-query", "seat-one", "game-1", 'e');
    query_secret.mcp_url = "https://example.com/mcp?token=secret".to_owned();
    assert!(matches!(
        manager.allocate(&manifest, query_secret),
        Err(WorkspaceError::InvalidMcpEndpoint)
    ));

    let target = temporary.path().join("target");
    fs::create_dir(&target).unwrap();
    let linked_root = temporary.path().join("linked-root");
    symlink(&target, &linked_root).unwrap();
    assert!(matches!(
        SeatWorkspaceManager::new(
            WorkspaceManagerConfig {
                root: linked_root,
                safe_path: "/usr/bin:/bin".to_owned(),
                sandbox: test_backend(),
                authority: authority_config(),
            },
            Arc::new(SystemDriverClock)
        ),
        Err(WorkspaceError::InvalidPath)
    ));
}

#[test]
fn drop_cleanup_and_failure_retention_follow_manifest_policy() {
    let temporary = tempfile::tempdir().unwrap();
    let manager = manager(&temporary, test_backend());
    let deleting = manager
        .allocate(
            &manifest("delete_on_finish"),
            request("run-drop", "seat-one", "game-1", 'a'),
        )
        .unwrap();
    let deleted_root = deleting.root().to_owned();
    drop(deleting);
    assert!(!deleted_root.exists());

    let retained = manager
        .allocate(
            &manifest("retain_on_failure"),
            request("run-retain", "seat-one", "game-2", 'b'),
        )
        .unwrap();
    let retained_root = retained.root().to_owned();
    drop(retained);
    assert!(retained_root.exists());
}

#[tokio::test]
async fn hostile_seat_processes_cannot_cross_read_or_inherit_environment() {
    let Ok(sandbox) = SeatSandboxBackend::detect() else {
        eprintln!("supported OS sandbox is unavailable; fail-closed detection covered separately");
        return;
    };
    let temporary = tempfile::tempdir().unwrap();
    let manager = manager(&temporary, sandbox);
    let manifest = manifest("retain_on_failure");
    let first_token = token('a');
    let second_token = token('b');
    let first = manager
        .allocate(&manifest, request("run-hostile", "seat-one", "game-1", 'a'))
        .unwrap();
    let second = manager
        .allocate(&manifest, request("run-hostile", "seat-two", "game-1", 'b'))
        .unwrap();
    let first_private = first.workspace().join("private.txt");
    let second_private = second.workspace().join("private.txt");
    fs::write(&first_private, b"first-rack\n").unwrap();
    fs::write(&second_private, b"second-rack\n").unwrap();
    let linked_opponent = first.workspace().join("opponent-link");
    symlink(&second_private, &linked_opponent).unwrap();

    let first_run = hostile_process(first.process_adapter(), first.workspace(), &linked_opponent);
    let second_run = hostile_process(second.process_adapter(), second.workspace(), &first_private);
    let (first_output, second_output) = tokio::join!(first_run, second_run);
    for output in [first_output.unwrap(), second_output.unwrap()] {
        assert!(output.contains("cross-read=denied"), "{output}");
        assert!(output.contains("own-write=allowed"), "{output}");
        assert!(output.contains("capability=[REDACTED]"), "{output}");
        assert!(output.contains("inherited-user=absent"), "{output}");
        assert!(!output.contains("first-rack"));
        assert!(!output.contains("second-rack"));
        assert!(!output.contains(&first_token));
        assert!(!output.contains(&second_token));
    }

    first.finish(WorkspaceOutcome::Completed).unwrap();
    second.finish(WorkspaceOutcome::Completed).unwrap();
}

#[tokio::test]
async fn config_tampering_blocks_process_spawn_before_execution() {
    let temporary = tempfile::tempdir().unwrap();
    let manager = manager(&temporary, test_backend());
    let lease = manager
        .allocate(
            &manifest("retain_on_failure"),
            request("run-integrity", "seat-one", "game-1", 'a'),
        )
        .unwrap();
    let adapter = lease.process_adapter();
    fs::write(lease.mcp_config(), b"tampered\n").unwrap();
    assert!(matches!(
        adapter
            .spawn(&ProcessSpec {
                executable: "/bin/echo".to_owned(),
                arguments: vec!["must-not-run".to_owned()],
                working_directory: Some(lease.workspace().to_owned()),
            })
            .await,
        Err(ProcessError::Spawn)
    ));
}

async fn hostile_process(
    adapter: Arc<dyn ProcessAdapter>,
    workspace: &Path,
    opponent: &Path,
) -> Result<String, ProcessError> {
    let script = r#"
if IFS= read -r stolen < "$1"; then
  printf 'cross-read=leaked:%s\n' "$stolen"
else
  printf 'cross-read=denied\n'
fi
printf 'owned\n' > "$WORD_ARENA_WORKSPACE/own-output.txt"
if IFS= read -r own < "$WORD_ARENA_WORKSPACE/own-output.txt"; then
  printf 'own-write=allowed\n'
else
  printf 'own-write=denied\n'
fi
printf 'capability=%s\n' "$WORD_ARENA_SEAT_CAPABILITY"
if [ "${USER+x}" = x ]; then
  printf 'inherited-user=present\n'
else
  printf 'inherited-user=absent\n'
fi
"#;
    let mut process = adapter
        .spawn(&ProcessSpec {
            executable: "/bin/sh".to_owned(),
            arguments: vec![
                "-c".to_owned(),
                script.to_owned(),
                "hostile-agent".to_owned(),
                opponent.to_string_lossy().into_owned(),
            ],
            working_directory: Some(workspace.to_owned()),
        })
        .await?;
    let mut output = Vec::new();
    loop {
        match process.next_event().await? {
            ProcessEvent::Stdout(bytes) => output.extend(bytes),
            ProcessEvent::Stderr(_) => {}
            ProcessEvent::Exited(exit) => {
                if !exit.success {
                    return Err(ProcessError::Read);
                }
                return String::from_utf8(output).map_err(|_| ProcessError::Read);
            }
        }
    }
}

fn all_files(root: &Path) -> Vec<PathBuf> {
    let mut pending = vec![root.to_owned()];
    let mut files = Vec::new();
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let file_type = entry.file_type().unwrap();
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                files.push(entry.path());
            }
        }
    }
    files
}

#[test]
fn lease_runtime_paths_are_exactly_the_managed_seat_paths() {
    let temporary = tempfile::tempdir().unwrap();
    let manager = manager(&temporary, test_backend());
    let lease = manager
        .allocate(
            &manifest("delete_on_finish"),
            request("run-runtime", "seat-one", "game-1", 'a'),
        )
        .unwrap();
    let runtime = lease.harness_runtime(HarnessExecutables::default());
    assert_eq!(runtime.workspace, lease.workspace());
    assert_eq!(runtime.state_directory, lease.state_directory());
    assert_eq!(runtime.mcp_config, lease.mcp_config());
}
