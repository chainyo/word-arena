use std::{fs, path::PathBuf};

use proptest::prelude::*;
use serde_json::json;
use word_arena_agent_runtime::{
    AgentManifestIdentity, DiagnosticRecord, DiagnosticStream, DriverLifecycleState,
    DriverTelemetry, LifecycleTransition, MAX_TELEMETRY_TEXT_BYTES, REDACTION_MARKER,
    RUN_TELEMETRY_SCHEMA_VERSION, RunTelemetryArchive, RunTelemetryCorrelation, RunUsageTelemetry,
    SourcedU64, TELEMETRY_REDACTION_POLICY_VERSION, TRUNCATION_MARKER, TelemetryAvailability,
    TelemetryRetentionPolicy, TelemetrySanitizer, TurnTelemetry, VisibleToolCall,
};

fn identity() -> AgentManifestIdentity {
    AgentManifestIdentity {
        schema_version: 1,
        hash_algorithm: "sha256-canonical-json-v1".to_owned(),
        manifest_sha256: "a".repeat(64),
    }
}

fn correlation() -> RunTelemetryCorrelation {
    RunTelemetryCorrelation {
        tournament_id: Some("tournament-1".to_owned()),
        match_id: Some("match-1".to_owned()),
        game_id: "game-1".to_owned(),
        run_id: "run-1".to_owned(),
        seat_number: 1,
    }
}

fn driver(secret: &str) -> DriverTelemetry {
    DriverTelemetry {
        schema_version: 1,
        run_id: "run-1".to_owned(),
        manifest: identity(),
        restarts: 2,
        lifecycle: vec![
            LifecycleTransition {
                sequence: 0,
                at_unix_ms: 10,
                state: DriverLifecycleState::Pending,
            },
            LifecycleTransition {
                sequence: 1,
                at_unix_ms: 11,
                state: DriverLifecycleState::TurnRunning {
                    turn_id: "turn-1".to_owned(),
                },
            },
            LifecycleTransition {
                sequence: 2,
                at_unix_ms: 15,
                state: DriverLifecycleState::Ready,
            },
        ],
        turns: vec![TurnTelemetry {
            turn_id: "turn-1".to_owned(),
            started_at_unix_ms: 11,
            completed_at_unix_ms: 15,
            visible_input: format!("Private rack ETE; configured={secret}"),
            visible_output: "Placed ETE; bearer=Bearer hidden-token".to_owned(),
            tool_calls: vec![VisibleToolCall {
                tool: "word_arena.place_tiles".to_owned(),
                arguments: json!({
                    "tiles": "ETE",
                    "authorization": secret,
                    "nested": {"token": "another-secret"}
                }),
                result: json!({"accepted": true, "credential": secret}),
            }],
        }],
        diagnostics: vec![DiagnosticRecord {
            sequence: 0,
            at_unix_ms: 16,
            stream: DiagnosticStream::Stderr,
            code: "provider_failure".to_owned(),
            visible_text: format!("failure\0 token={secret}"),
        }],
    }
}

fn usage() -> RunUsageTelemetry {
    RunUsageTelemetry {
        input_tokens: SourcedU64::new(TelemetryAvailability::Exact, Some(100), "provider_usage")
            .unwrap(),
        output_tokens: SourcedU64::new(
            TelemetryAvailability::Estimated,
            Some(20),
            "tokenizer_estimate",
        )
        .unwrap(),
        cost_microusd: SourcedU64::new(
            TelemetryAvailability::Unavailable,
            None,
            "provider_omitted",
        )
        .unwrap(),
    }
}

fn archive(secret: &str) -> RunTelemetryArchive {
    RunTelemetryArchive::capture(
        correlation(),
        identity(),
        &driver(secret),
        usage(),
        TelemetryRetentionPolicy::expire_at(1_000),
        20,
        &TelemetrySanitizer::new([secret.as_bytes().to_vec()]),
    )
    .unwrap()
}

#[test]
fn published_contract_matches_runtime_limits_and_versions() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/agent-run-telemetry-v1.json");
    let contract: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(contract["schema_version"], RUN_TELEMETRY_SCHEMA_VERSION);
    assert_eq!(
        contract["redaction_policy_version"],
        TELEMETRY_REDACTION_POLICY_VERSION
    );
    assert_eq!(contract["limits"]["text_bytes"], MAX_TELEMETRY_TEXT_BYTES);
    assert_eq!(contract["public_content_fields_omitted"], true);
}

#[test]
fn capture_redacts_secrets_sensitive_keys_controls_and_token_shapes() {
    let secret = "synthetic-provider-secret";
    let sanitizer = TelemetrySanitizer::new([secret.as_bytes().to_vec()]);
    assert!(!format!("{sanitizer:?}").contains(secret));
    let archive = RunTelemetryArchive::capture(
        correlation(),
        identity(),
        &driver(secret),
        usage(),
        TelemetryRetentionPolicy::expire_at(1_000),
        20,
        &sanitizer,
    )
    .unwrap();
    let bytes = serde_json::to_string(&archive).unwrap();
    assert!(!bytes.contains(secret));
    assert!(!bytes.contains("hidden-token"));
    assert!(!bytes.contains("another-secret"));
    assert!(bytes.contains(REDACTION_MARKER));
    assert!(archive.sanitization.redacted_values >= 5);
    assert_eq!(archive.sanitization.replaced_control_characters, 1);
    assert!(bytes.contains("Private rack ETE"));
    assert!(bytes.contains("word_arena.place_tiles"));

    let (binary, stats) = TelemetrySanitizer::new([secret.as_bytes().to_vec()])
        .sanitize_untrusted_bytes(&[0xff, b'x', 0, b'y']);
    assert!(binary.contains('\u{fffd}'));
    assert!(stats.replaced_invalid_utf8_sequences > 0);
    assert!(stats.replaced_control_characters > 0);
}

#[test]
fn public_projection_omits_every_content_bearing_field_by_construction() {
    let private = archive("private-secret");
    let public = private.public_projection();
    let bytes = serde_json::to_string(&public).unwrap();
    for forbidden in [
        "Private rack ETE",
        "visible_input",
        "visible_output",
        "arguments",
        "result",
        "visible_text",
        "private-secret",
    ] {
        assert!(
            !bytes.contains(forbidden),
            "public bytes leaked {forbidden}"
        );
    }
    assert!(bytes.contains("turn-1"));
    assert!(bytes.contains("word_arena.place_tiles"));
    assert_eq!(public.turns[0].duration_ms, 4);
    assert!(public.privacy.content_fields_omitted);
}

#[test]
fn truncation_availability_and_checked_cost_arithmetic_are_explicit() {
    let mut driver = driver("secret");
    driver.turns[0].visible_output = "x".repeat(MAX_TELEMETRY_TEXT_BYTES + 500);
    let archive = RunTelemetryArchive::capture(
        correlation(),
        identity(),
        &driver,
        usage(),
        TelemetryRetentionPolicy::retain(),
        20,
        &TelemetrySanitizer::new([b"secret".to_vec()]),
    )
    .unwrap();
    assert!(archive.turns[0].visible_output.ends_with(TRUNCATION_MARKER));
    assert!(archive.turns[0].visible_output.len() <= MAX_TELEMETRY_TEXT_BYTES);
    assert!(archive.sanitization.truncated_values > 0);

    let exact = SourcedU64::new(TelemetryAvailability::Exact, Some(4), "provider_a").unwrap();
    let estimated =
        SourcedU64::new(TelemetryAvailability::Estimated, Some(6), "provider_b").unwrap();
    let total = SourcedU64::checked_sum(&[exact, estimated], "match_total").unwrap();
    assert_eq!(total.value, Some(10));
    assert_eq!(total.availability, TelemetryAvailability::Estimated);
    let max = SourcedU64::new(TelemetryAvailability::Exact, Some(u64::MAX), "provider").unwrap();
    let one = SourcedU64::new(TelemetryAvailability::Exact, Some(1), "provider").unwrap();
    assert!(SourcedU64::checked_sum(&[max, one], "overflow").is_err());
    assert!(SourcedU64::new(TelemetryAvailability::Unavailable, Some(0), "invalid").is_err());
}

#[test]
fn sequence_identity_time_and_retention_drift_fail_closed() {
    let mut telemetry = archive("secret");
    telemetry.lifecycle[0].sequence = 99;
    assert!(telemetry.validate().is_err());

    let mut invalid_driver = driver("secret");
    invalid_driver.turns[0].completed_at_unix_ms = 1;
    assert!(
        RunTelemetryArchive::capture(
            correlation(),
            identity(),
            &invalid_driver,
            usage(),
            TelemetryRetentionPolicy::retain(),
            20,
            &TelemetrySanitizer::new(std::iter::empty::<Vec<u8>>()),
        )
        .is_err()
    );
    let mut wrong = correlation();
    wrong.run_id = "other".to_owned();
    assert!(
        RunTelemetryArchive::capture(
            wrong,
            identity(),
            &driver("secret"),
            usage(),
            TelemetryRetentionPolicy::expire_at(19),
            20,
            &TelemetrySanitizer::new(std::iter::empty::<Vec<u8>>()),
        )
        .is_err()
    );
}

proptest! {
    #[test]
    fn configured_secret_corpus_never_survives_private_or_public_serialization(
        secret in "secret_[A-Za-z0-9]{8,48}"
    ) {
        let telemetry = archive(&secret);
        let private = serde_json::to_string(&telemetry).unwrap();
        let public = serde_json::to_string(&telemetry.public_projection()).unwrap();
        prop_assert!(!private.contains(&secret));
        prop_assert!(!public.contains(&secret));
    }
}
