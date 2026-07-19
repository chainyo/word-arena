//! Versioned, privacy-safe contracts for autonomous agent execution.
//!
//! This crate owns immutable agent manifests and their content identities. It
//! does not start processes, read provider credentials, or know game rules.

use std::collections::{BTreeMap, BTreeSet};

use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Current agent manifest schema.
pub const AGENT_MANIFEST_SCHEMA_VERSION: u32 = 1;
/// Current terminal run-result attribution schema.
pub const AGENT_RUN_RESULT_SCHEMA_VERSION: u32 = 1;
/// Stable algorithm label used in persisted manifest identities.
pub const MANIFEST_HASH_ALGORITHM: &str = "sha256-canonical-json-v1";

/// Complete immutable description of one agent execution configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManifest {
    /// Exact schema understood by this build.
    pub schema_version: u32,
    /// Stable operator-facing entrant name, not a credential or database ID.
    pub name: String,
    /// Harness and exact installed-version expectation.
    pub harness: HarnessConfig,
    /// Model identity and mutually exclusive execution source.
    pub model: ModelConfig,
    /// Hash-only prompt identity; prompt content is supplied outside the manifest.
    pub prompt: PromptIdentity,
    /// Competitive tool and network policy.
    pub tool_policy: ToolPolicy,
    /// Immutable execution environment identity.
    pub environment: EnvironmentIdentity,
    /// Word Arena driver implementation version.
    pub driver_version: String,
    /// Seat-workspace persistence and cleanup behavior.
    pub workspace: WorkspacePolicy,
    /// Complete operator-selected resource budget.
    pub budgets: ResourceBudgets,
    /// Non-secret labels included in attribution and filtering.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// Supported execution harness and its exact compatibility input.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HarnessConfig {
    /// `OpenAI` Codex CLI.
    Codex { version: String },
    /// Anthropic Claude Code CLI.
    ClaudeCode { version: String },
    /// Cline CLI/driver integration.
    Cline { version: String },
    /// Pi coding agent.
    Pi { version: String },
    /// Direct process integration for any non-shell executable.
    GenericCommand {
        version: String,
        executable: String,
        #[serde(default)]
        arguments: Vec<String>,
    },
}

impl HarnessConfig {
    fn version(&self) -> &str {
        match self {
            Self::Codex { version }
            | Self::ClaudeCode { version }
            | Self::Cline { version }
            | Self::Pi { version }
            | Self::GenericCommand { version, .. } => version,
        }
    }
}

/// Model name plus exactly one source of execution semantics.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    /// Provider/model identifier shown in results.
    pub id: String,
    /// Provider, local runtime, or harness-default selection.
    pub source: ModelSource,
}

/// Mutually exclusive model execution sources. Provider credentials are never
/// representable here.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelSource {
    /// The harness resolves its configured default while retaining the model ID.
    HarnessDefault,
    /// A named remote provider selected outside any secret-bearing config.
    Provider { provider: ModelProvider },
    /// A local runtime whose immutable implementation is part of the image.
    Local { runtime: LocalModelRuntime },
}

/// Supported remote provider identities without credentials or endpoints.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelProvider {
    OpenAi,
    Anthropic,
    Google,
    OpenRouter,
}

/// Supported local model runtimes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalModelRuntime {
    Ollama,
    LlamaCpp,
    Vllm,
}

/// Immutable hash of the exact visible prompt bytes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PromptIdentity {
    pub format_version: u32,
    pub sha256: String,
}

/// Competitive tool boundary, including the only permitted network mode.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolPolicy {
    pub policy_version: u32,
    #[serde(default)]
    pub allowed_tools: BTreeSet<String>,
    #[serde(default)]
    pub denied_tools: BTreeSet<String>,
    pub network: NetworkPolicy,
}

/// Network access is explicit and deny-by-default.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NetworkPolicy {
    Deny,
    McpOnly,
    Allowlisted { hosts: BTreeSet<String> },
}

/// Immutable OCI environment and target platform.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentIdentity {
    /// OCI reference containing an exact `@sha256:<64 lowercase hex>` digest.
    pub image: String,
    /// Normalized `os/architecture` pair.
    pub platform: String,
}

/// Persistent-seat workspace behavior; paths and credentials are assigned by
/// the driver and cannot be requested by a manifest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePolicy {
    pub policy_version: u32,
    pub persistence: WorkspacePersistence,
    pub retention: WorkspaceRetention,
    pub max_bytes: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspacePersistence {
    PerTurn,
    PersistentForRun,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRetention {
    DeleteOnFinish,
    RetainOnFailure,
}

/// V1 resource and behavior limits. Every dimension is mandatory so omitted
/// limits cannot silently become unbounded.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceBudgets {
    pub wall_time_ms: u64,
    pub cpu_time_ms: u64,
    pub memory_bytes: u64,
    pub network_bytes: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub attempts: u32,
    pub tool_calls: u32,
    pub output_bytes: u64,
    pub cost_microusd: u64,
}

/// Persisted content address of one canonical manifest.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManifestIdentity {
    pub schema_version: u32,
    pub hash_algorithm: String,
    pub manifest_sha256: String,
}

/// Validated manifest, canonical bytes, and immutable identity kept together so
/// callers cannot accidentally persist a digest for different content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedAgentManifest {
    manifest: AgentManifest,
    canonical_json: Vec<u8>,
    identity: AgentManifestIdentity,
}

impl ValidatedAgentManifest {
    /// Parses strict JSON, rejects secret-bearing input, validates every field,
    /// and calculates the canonical content identity.
    ///
    /// # Errors
    ///
    /// Returns a typed validation error before any process can start.
    pub fn from_json(input: &[u8]) -> Result<Self, ManifestError> {
        let value: Value = serde_json::from_slice(input).map_err(ManifestError::Json)?;
        reject_secrets(&value)?;
        let manifest: AgentManifest = serde_json::from_value(value).map_err(ManifestError::Json)?;
        Self::new(manifest)
    }

    /// Validates a typed manifest and calculates its canonical content identity.
    ///
    /// # Errors
    ///
    /// Returns a typed field error for unsafe or noncanonical input.
    pub fn new(manifest: AgentManifest) -> Result<Self, ManifestError> {
        let value = serde_json::to_value(&manifest).map_err(ManifestError::Json)?;
        reject_secrets(&value)?;
        validate_manifest(&manifest)?;
        let canonical_json = canonical_json(&value)?;
        let manifest_sha256 = hex(&Sha256::digest(&canonical_json));
        let identity = AgentManifestIdentity {
            schema_version: manifest.schema_version,
            hash_algorithm: MANIFEST_HASH_ALGORITHM.to_owned(),
            manifest_sha256,
        };
        Ok(Self {
            manifest,
            canonical_json,
            identity,
        })
    }

    #[must_use]
    pub const fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    #[must_use]
    pub fn canonical_json(&self) -> &[u8] {
        &self.canonical_json
    }

    #[must_use]
    pub const fn identity(&self) -> &AgentManifestIdentity {
        &self.identity
    }
}

/// Strict manifest parsing or safety failure.
#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("agent manifest JSON is invalid: {0}")]
    Json(serde_json::Error),
    #[error("unsupported agent manifest schema version {0}")]
    UnsupportedSchema(u32),
    #[error("agent manifest field {field} is invalid: {reason}")]
    InvalidField {
        field: &'static str,
        reason: &'static str,
    },
    #[error("agent manifest contains a forbidden secret-bearing key or value")]
    SecretBearing,
    #[error("generic command must name a direct non-shell executable")]
    UnsafeCommand,
    #[error("agent manifest canonical serialization failed: {0}")]
    Canonical(serde_json::Error),
}

fn validate_manifest(manifest: &AgentManifest) -> Result<(), ManifestError> {
    if manifest.schema_version != AGENT_MANIFEST_SCHEMA_VERSION {
        return Err(ManifestError::UnsupportedSchema(manifest.schema_version));
    }
    validate_identifier("name", &manifest.name, 1, 96)?;
    validate_semver("harness.version", manifest.harness.version())?;
    validate_semver("driver_version", &manifest.driver_version)?;
    validate_identifier("model.id", &manifest.model.id, 1, 128)?;
    validate_digest("prompt.sha256", &manifest.prompt.sha256)?;
    positive(
        "prompt.format_version",
        u64::from(manifest.prompt.format_version),
    )?;
    positive(
        "tool_policy.policy_version",
        u64::from(manifest.tool_policy.policy_version),
    )?;
    if !manifest
        .tool_policy
        .allowed_tools
        .is_disjoint(&manifest.tool_policy.denied_tools)
    {
        return invalid("tool_policy", "allowed and denied tools overlap");
    }
    for tool in manifest
        .tool_policy
        .allowed_tools
        .iter()
        .chain(&manifest.tool_policy.denied_tools)
    {
        validate_machine_name("tool_policy tool", tool)?;
    }
    if let NetworkPolicy::Allowlisted { hosts } = &manifest.tool_policy.network {
        if hosts.is_empty() {
            return invalid("tool_policy.network.hosts", "allowlist is empty");
        }
        for host in hosts {
            validate_host(host)?;
        }
    }
    validate_image(&manifest.environment.image)?;
    validate_platform(&manifest.environment.platform)?;
    positive(
        "workspace.policy_version",
        u64::from(manifest.workspace.policy_version),
    )?;
    positive("workspace.max_bytes", manifest.workspace.max_bytes)?;
    validate_budgets(&manifest.budgets)?;
    for (key, value) in &manifest.labels {
        validate_machine_name("labels key", key)?;
        validate_identifier("labels value", value, 1, 128)?;
    }
    if let HarnessConfig::GenericCommand {
        executable,
        arguments,
        ..
    } = &manifest.harness
    {
        validate_command(executable, arguments)?;
    }
    Ok(())
}

fn validate_budgets(budgets: &ResourceBudgets) -> Result<(), ManifestError> {
    for (field, value) in [
        ("budgets.wall_time_ms", budgets.wall_time_ms),
        ("budgets.cpu_time_ms", budgets.cpu_time_ms),
        ("budgets.memory_bytes", budgets.memory_bytes),
        ("budgets.network_bytes", budgets.network_bytes),
        ("budgets.input_tokens", budgets.input_tokens),
        ("budgets.output_tokens", budgets.output_tokens),
        ("budgets.attempts", u64::from(budgets.attempts)),
        ("budgets.tool_calls", u64::from(budgets.tool_calls)),
        ("budgets.output_bytes", budgets.output_bytes),
        ("budgets.cost_microusd", budgets.cost_microusd),
    ] {
        positive(field, value)?;
    }
    if budgets.cpu_time_ms > budgets.wall_time_ms {
        return invalid("budgets.cpu_time_ms", "exceeds wall-time budget");
    }
    Ok(())
}

fn validate_command(executable: &str, arguments: &[String]) -> Result<(), ManifestError> {
    validate_identifier("harness.executable", executable, 1, 512)?;
    let basename = executable.rsplit(['/', '\\']).next().unwrap_or(executable);
    let normalized = basename.to_ascii_lowercase();
    if [
        "sh",
        "bash",
        "zsh",
        "fish",
        "cmd",
        "cmd.exe",
        "powershell",
        "powershell.exe",
        "pwsh",
    ]
    .contains(&normalized.as_str())
        || executable.chars().any(char::is_whitespace)
    {
        return Err(ManifestError::UnsafeCommand);
    }
    for argument in arguments {
        validate_identifier("harness.arguments", argument, 1, 1_024)?;
        if argument.contains(['\0', '\n', '\r'])
            || argument.contains("$(")
            || argument.contains('`')
            || argument.contains("&&")
            || argument.contains("||")
            || argument == ";"
        {
            return Err(ManifestError::UnsafeCommand);
        }
    }
    Ok(())
}

fn validate_semver(field: &'static str, value: &str) -> Result<(), ManifestError> {
    Version::parse(value)
        .map(|_| ())
        .map_err(|_| ManifestError::InvalidField {
            field,
            reason: "must be one exact semantic version",
        })
}

fn validate_digest(field: &'static str, value: &str) -> Result<(), ManifestError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        invalid(field, "must be 64 lowercase hexadecimal characters")
    }
}

fn validate_image(value: &str) -> Result<(), ManifestError> {
    let Some((name, digest)) = value.rsplit_once("@sha256:") else {
        return invalid("environment.image", "must contain an OCI sha256 digest");
    };
    validate_identifier("environment.image", name, 1, 512)?;
    validate_digest("environment.image", digest)
}

fn validate_platform(value: &str) -> Result<(), ManifestError> {
    let Some((os, architecture)) = value.split_once('/') else {
        return invalid("environment.platform", "must be normalized os/architecture");
    };
    if !matches!(os, "linux" | "darwin" | "windows") || !matches!(architecture, "amd64" | "arm64") {
        return invalid("environment.platform", "unsupported os or architecture");
    }
    Ok(())
}

fn validate_host(value: &str) -> Result<(), ManifestError> {
    if value.is_empty()
        || value.len() > 253
        || value.starts_with('.')
        || value.ends_with('.')
        || value.contains(['/', ':', '@', ' '])
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    {
        return invalid(
            "tool_policy.network.hosts",
            "must be a DNS host without scheme or port",
        );
    }
    Ok(())
}

fn validate_machine_name(field: &'static str, value: &str) -> Result<(), ManifestError> {
    if value.is_empty()
        || value.len() > 64
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
    {
        return invalid(field, "must be a normalized lowercase machine name");
    }
    Ok(())
}

fn validate_identifier(
    field: &'static str,
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), ManifestError> {
    if value.len() < minimum
        || value.len() > maximum
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return invalid(
            field,
            "length, surrounding whitespace, or control characters are invalid",
        );
    }
    Ok(())
}

fn positive(field: &'static str, value: u64) -> Result<(), ManifestError> {
    if value == 0 {
        invalid(field, "must be greater than zero")
    } else {
        Ok(())
    }
}

fn invalid<T>(field: &'static str, reason: &'static str) -> Result<T, ManifestError> {
    Err(ManifestError::InvalidField { field, reason })
}

fn reject_secrets(value: &Value) -> Result<(), ManifestError> {
    match value {
        Value::Object(values) => {
            for (key, child) in values {
                let normalized = key.to_ascii_lowercase().replace('-', "_");
                if [
                    "api_key",
                    "authorization",
                    "credential",
                    "credentials",
                    "password",
                    "provider_secret",
                    "secret",
                ]
                .contains(&normalized.as_str())
                {
                    return Err(ManifestError::SecretBearing);
                }
                reject_secrets(child)?;
            }
        }
        Value::Array(values) => {
            for child in values {
                reject_secrets(child)?;
            }
        }
        Value::String(value) => {
            let lower = value.to_ascii_lowercase();
            if lower.starts_with("sk-")
                || lower.starts_with("ghp_")
                || lower.starts_with("bearer ")
                || lower.contains("api_key=")
                || lower.contains("api-key=")
                || lower.contains("_token=")
                || lower.contains("password=")
            {
                return Err(ManifestError::SecretBearing);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
    Ok(())
}

fn canonical_json(value: &Value) -> Result<Vec<u8>, ManifestError> {
    let normalized = canonical_value(value);
    serde_json::to_vec(&normalized).map_err(ManifestError::Canonical)
}

fn canonical_value(value: &Value) -> Value {
    match value {
        Value::Object(values) => {
            let sorted = values
                .iter()
                .map(|(key, value)| (key.clone(), canonical_value(value)))
                .collect::<BTreeMap<_, _>>();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(values) => Value::Array(values.iter().map(canonical_value).collect()),
        _ => value.clone(),
    }
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}
