#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use serde_json::{Value, json};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use word_arena_agent_runtime::{
    AgentDriver, CLAUDE_CODE_MINIMUM_VERSION, CLINE_MINIMUM_VERSION, CODEX_MINIMUM_VERSION,
    DriverError, HarnessExecutables, HarnessRuntimeConfig, NativeHarnessKind, PI_MINIMUM_VERSION,
    ProcessSpec, SupportedAgentDriver, SystemDriverClock, TerminationReason, TokioProcessAdapter,
    TurnRequest, ValidatedAgentManifest,
};

const NATIVE_EXAMPLES: [(&str, NativeHarnessKind); 4] = [
    ("codex-v1.json", NativeHarnessKind::Codex),
    ("claude-code-v1.json", NativeHarnessKind::ClaudeCode),
    ("cline-v1.json", NativeHarnessKind::Cline),
    ("pi-v1.json", NativeHarnessKind::Pi),
];

fn example(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/agents")
        .join(name);
    fs::read(path).unwrap()
}

fn manifest(name: &str) -> ValidatedAgentManifest {
    ValidatedAgentManifest::from_json(&example(name)).unwrap()
}

struct FixtureRuntime {
    _temporary: TempDir,
    config: HarnessRuntimeConfig,
    generic_executable: PathBuf,
}

impl FixtureRuntime {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let bin = temporary.path().join("bin");
        let workspace = temporary.path().join("workspace");
        let state = temporary.path().join("state");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&state).unwrap();
        let mcp_config = state.join("mcp.json");
        fs::write(&mcp_config, b"{}\n").unwrap();
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-harness");
        for name in [
            "codex",
            "codex-old",
            "claude",
            "cline",
            "pi",
            "generic-agent",
        ] {
            symlink(&fixture, bin.join(name)).unwrap();
        }
        let generic_executable = bin.join("generic-agent");
        Self {
            config: HarnessRuntimeConfig {
                workspace,
                state_directory: state,
                mcp_config,
                executables: HarnessExecutables {
                    codex: path_text(&bin.join("codex")),
                    claude_code: path_text(&bin.join("claude")),
                    cline: path_text(&bin.join("cline")),
                    pi: path_text(&bin.join("pi")),
                },
            },
            generic_executable,
            _temporary: temporary,
        }
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn request() -> TurnRequest {
    TurnRequest {
        turn_id: "turn-1".to_owned(),
        visible_input: "Use only the visible game state and play one move.".to_owned(),
    }
}

#[tokio::test]
async fn every_native_fake_binary_completes_the_common_lifecycle_offline() {
    for (example, expected_kind) in NATIVE_EXAMPLES {
        let fixture = FixtureRuntime::new();
        let manifest = manifest(example);
        let mut driver = SupportedAgentDriver::new(
            format!("run-{}", expected_kind.name()),
            &manifest,
            fixture.config.clone(),
            Arc::new(TokioProcessAdapter),
            Arc::new(SystemDriverClock),
        )
        .unwrap();
        let cancel = CancellationToken::new();

        driver.start(&cancel).await.unwrap();
        let output = tokio::time::timeout(
            Duration::from_secs(2),
            driver.request_turn(request(), &cancel),
        )
        .await
        .expect("native harness must not wait for stdin")
        .unwrap();
        assert_eq!(output.visible_output, "Placed ETE.");
        assert_eq!(output.tool_calls.len(), 1);
        assert_eq!(driver.telemetry().turns.len(), 1);
        let SupportedAgentDriver::Native(native) = &driver else {
            panic!("native manifest selected generic driver");
        };
        assert_eq!(native.kind(), expected_kind);
        assert!(native.policy().allowed_tools.contains("mcp.word_arena"));
        assert!(driver.telemetry().diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "harness_stderr_redacted"
                && diagnostic.visible_text.contains("redacted stderr bytes")
        }));

        let serialized = serde_json::to_string(driver.telemetry()).unwrap();
        assert!(!serialized.contains("hidden-"));
        assert!(!serialized.contains("must-be-redacted"));
        assert!(!serialized.contains("reasoning"));

        let checkpoint_json = serde_json::to_vec(&driver.checkpoint().unwrap()).unwrap();
        drop(driver);
        let mut restored = SupportedAgentDriver::restore(
            &manifest,
            serde_json::from_slice(&checkpoint_json).unwrap(),
            fixture.config,
            Arc::new(TokioProcessAdapter),
            Arc::new(SystemDriverClock),
        )
        .unwrap();
        restored.resume(&cancel).await.unwrap();
        assert_eq!(restored.telemetry().turns.len(), 1);
        restored
            .terminate(TerminationReason::Completed)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn generic_fake_binary_uses_bound_runtime_placeholders_through_same_factory() {
    let fixture = FixtureRuntime::new();
    let mut value: Value = serde_json::from_slice(&example("generic-command-v1.json")).unwrap();
    value["harness"]["executable"] = json!(path_text(&fixture.generic_executable));
    value["harness"]["arguments"] = json!([
        "--workspace",
        "{workspace}",
        "--mcp-config",
        "{mcp_config}",
        "--state",
        "{state_directory}"
    ]);
    let manifest = ValidatedAgentManifest::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let mut driver = SupportedAgentDriver::new(
        "run-generic",
        &manifest,
        fixture.config,
        Arc::new(TokioProcessAdapter),
        Arc::new(SystemDriverClock),
    )
    .unwrap();
    let cancel = CancellationToken::new();

    driver.start(&cancel).await.unwrap();
    let output = driver.request_turn(request(), &cancel).await.unwrap();
    assert_eq!(output.visible_output, "Placed ETE.");
    assert_eq!(driver.telemetry().turns.len(), 1);
    driver
        .terminate(TerminationReason::Completed)
        .await
        .unwrap();
}

#[tokio::test]
async fn missing_and_incompatible_native_executables_fail_actionably() {
    let fixture = FixtureRuntime::new();
    let manifest = manifest("codex-v1.json");
    let mut missing_runtime = fixture.config.clone();
    missing_runtime.executables.codex = "/definitely/missing/word-arena-codex".to_owned();
    let mut missing = SupportedAgentDriver::new(
        "run-missing",
        &manifest,
        missing_runtime,
        Arc::new(TokioProcessAdapter),
        Arc::new(SystemDriverClock),
    )
    .unwrap();
    assert!(matches!(
        missing.start(&CancellationToken::new()).await,
        Err(DriverError::HarnessUnavailable {
            harness: "codex",
            ..
        })
    ));

    let mut value: Value = serde_json::from_slice(&example("codex-v1.json")).unwrap();
    value["harness"]["version"] = json!("0.144.1");
    let incompatible =
        ValidatedAgentManifest::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let mut driver = SupportedAgentDriver::new(
        "run-mismatch",
        &incompatible,
        fixture.config,
        Arc::new(TokioProcessAdapter),
        Arc::new(SystemDriverClock),
    )
    .unwrap();
    assert!(matches!(
        driver.start(&CancellationToken::new()).await,
        Err(DriverError::HarnessVersionMismatch {
            harness: "codex",
            expected,
            installed
        }) if expected == "0.144.1" && installed == "0.144.0"
    ));

    let mut value: Value = serde_json::from_slice(&example("codex-v1.json")).unwrap();
    value["harness"]["version"] = json!("0.143.0");
    let below_minimum =
        ValidatedAgentManifest::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let mut old_fixture = FixtureRuntime::new();
    let old_executable = PathBuf::from(&old_fixture.config.executables.codex)
        .parent()
        .unwrap()
        .join("codex-old");
    old_fixture.config.executables.codex = path_text(&old_executable);
    let mut driver = SupportedAgentDriver::new(
        "run-old",
        &below_minimum,
        old_fixture.config,
        Arc::new(TokioProcessAdapter),
        Arc::new(SystemDriverClock),
    )
    .unwrap();
    assert!(matches!(
        driver.start(&CancellationToken::new()).await,
        Err(DriverError::HarnessVersionUnsupported {
            harness: "codex",
            installed,
            minimum: CODEX_MINIMUM_VERSION
        }) if installed == "0.143.0"
    ));
}

#[test]
fn reviewed_minimums_examples_and_redacted_debug_contract_stay_aligned() {
    let expected = [
        ("codex-v1.json", CODEX_MINIMUM_VERSION),
        ("claude-code-v1.json", CLAUDE_CODE_MINIMUM_VERSION),
        ("cline-v1.json", CLINE_MINIMUM_VERSION),
        ("pi-v1.json", PI_MINIMUM_VERSION),
    ];
    for (name, minimum) in expected {
        let value: Value = serde_json::from_slice(&example(name)).unwrap();
        assert_eq!(value["harness"]["version"], minimum);
    }

    let contract_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/agent-harnesses-v1.json");
    let contract: Value = serde_json::from_slice(&fs::read(contract_path).unwrap()).unwrap();
    assert_eq!(
        contract["harnesses"]["codex"]["minimum_version"],
        CODEX_MINIMUM_VERSION
    );
    assert_eq!(
        contract["harnesses"]["claude_code"]["minimum_version"],
        CLAUDE_CODE_MINIMUM_VERSION
    );
    assert_eq!(
        contract["harnesses"]["cline"]["minimum_version"],
        CLINE_MINIMUM_VERSION
    );
    assert_eq!(
        contract["harnesses"]["pi"]["minimum_version"],
        PI_MINIMUM_VERSION
    );
    assert_eq!(
        contract["native_output_policy"]["discarded"][0],
        "reasoning"
    );

    let process = ProcessSpec {
        executable: "codex".to_owned(),
        arguments: vec![
            "--config".to_owned(),
            "/secret/config".to_owned(),
            "private prompt".to_owned(),
        ],
        working_directory: Some(PathBuf::from("/secret/workspace")),
    };
    let debug = format!("{process:?}");
    assert!(!debug.contains("private prompt"));
    assert!(!debug.contains("/secret/config"));
    assert!(!debug.contains("/secret/workspace"));
    assert!(debug.contains("argument_count"));
}
