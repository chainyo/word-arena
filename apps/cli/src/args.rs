use std::{fmt, path::PathBuf};

use serde_json::Value;

use crate::error::CliError;

pub const HELP: &str = "\
Word Arena CLI

Usage:
  word-arena [GLOBAL OPTIONS] health
  word-arena [GLOBAL OPTIONS] auth
  word-arena [GLOBAL OPTIONS] observe
  word-arena [GLOBAL OPTIONS] action --expected-version N --turn-id N \\
    --idempotency-key KEY --action-json JSON
  word-arena [GLOBAL OPTIONS] replay export [--output PATH]
  word-arena [GLOBAL OPTIONS] mcp stdio

Global options:
  --server URL       Server base URL
  --game-id ID       Bound game ID
  --token TOKEN      Scoped capability (never printed)
  --config PATH      Permission-restricted TOML config
  --timeout-ms N     HTTP timeout in milliseconds
  -h, --help         Show this help
";

#[derive(Clone, Default, Eq, PartialEq)]
pub struct ConfigOverrides {
    pub server_url: Option<String>,
    pub game_id: Option<String>,
    pub token: Option<String>,
    pub timeout_ms: Option<u64>,
}

impl fmt::Debug for ConfigOverrides {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfigOverrides")
            .field("server_url", &self.server_url)
            .field("game_id", &self.game_id)
            .field("token", &self.token.as_ref().map(|_| "[REDACTED]"))
            .field("timeout_ms", &self.timeout_ms)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Health,
    Auth,
    Observe,
    Action {
        expected_version: u64,
        turn_id: u64,
        idempotency_key: String,
        action: Value,
    },
    ReplayExport {
        output: Option<PathBuf>,
    },
    McpStdio,
    Help,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Invocation {
    pub overrides: ConfigOverrides,
    pub config_path: Option<PathBuf>,
    pub command: Command,
}

/// Parses one complete CLI invocation without loading configuration or making I/O.
///
/// # Errors
///
/// Returns a usage error for missing, unknown, duplicate, or malformed arguments.
pub fn parse<I, S>(arguments: I) -> Result<Invocation, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut arguments = arguments.into_iter().map(Into::into).peekable();
    let _program = arguments.next();
    let mut overrides = ConfigOverrides::default();
    let mut config_path = None;

    let command = loop {
        let argument = arguments
            .next()
            .ok_or_else(|| CliError::usage("a command is required"))?;
        match argument.as_str() {
            "--server" => overrides.server_url = Some(next_value(&mut arguments, "--server")?),
            "--game-id" => overrides.game_id = Some(next_value(&mut arguments, "--game-id")?),
            "--token" => overrides.token = Some(next_value(&mut arguments, "--token")?),
            "--config" => {
                config_path = Some(PathBuf::from(next_value(&mut arguments, "--config")?));
            }
            "--timeout-ms" => {
                overrides.timeout_ms = Some(
                    next_value(&mut arguments, "--timeout-ms")?
                        .parse()
                        .map_err(|_| CliError::usage("--timeout-ms must be an integer"))?,
                );
            }
            "-h" | "--help" => break Command::Help,
            "health" => break no_trailing(arguments, Command::Health)?,
            "auth" => break no_trailing(arguments, Command::Auth)?,
            "observe" => break no_trailing(arguments, Command::Observe)?,
            "action" => break parse_action(arguments)?,
            "replay" => break parse_replay(arguments)?,
            "mcp" => break parse_mcp(arguments)?,
            unknown => {
                return Err(CliError::usage(format!(
                    "unknown command or option: {unknown}"
                )));
            }
        }
    };
    Ok(Invocation {
        overrides,
        config_path,
        command,
    })
}

fn parse_action<I>(mut arguments: std::iter::Peekable<I>) -> Result<Command, CliError>
where
    I: Iterator<Item = String>,
{
    let mut expected_version = None;
    let mut turn_id = None;
    let mut idempotency_key = None;
    let mut action = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--expected-version" => {
                expected_version = Some(parse_u64(
                    &next_value(&mut arguments, "--expected-version")?,
                    "--expected-version",
                )?);
            }
            "--turn-id" => {
                turn_id = Some(parse_u64(
                    &next_value(&mut arguments, "--turn-id")?,
                    "--turn-id",
                )?);
            }
            "--idempotency-key" => {
                idempotency_key = Some(next_value(&mut arguments, "--idempotency-key")?);
            }
            "--action-json" => {
                let raw = next_value(&mut arguments, "--action-json")?;
                action = Some(serde_json::from_str(&raw).map_err(|error| {
                    CliError::usage(format!("--action-json is not valid JSON: {error}"))
                })?);
            }
            unknown => return Err(CliError::usage(format!("unknown action option: {unknown}"))),
        }
    }
    Ok(Command::Action {
        expected_version: expected_version
            .ok_or_else(|| CliError::usage("--expected-version is required"))?,
        turn_id: turn_id.ok_or_else(|| CliError::usage("--turn-id is required"))?,
        idempotency_key: idempotency_key
            .filter(|key| !key.is_empty())
            .ok_or_else(|| CliError::usage("--idempotency-key is required"))?,
        action: action.ok_or_else(|| CliError::usage("--action-json is required"))?,
    })
}

fn parse_replay<I>(mut arguments: std::iter::Peekable<I>) -> Result<Command, CliError>
where
    I: Iterator<Item = String>,
{
    if arguments.next().as_deref() != Some("export") {
        return Err(CliError::usage("replay requires the export subcommand"));
    }
    let mut output = None;
    while let Some(argument) = arguments.next() {
        if argument == "--output" && output.is_none() {
            output = Some(PathBuf::from(next_value(&mut arguments, "--output")?));
        } else {
            return Err(CliError::usage(format!(
                "unknown replay option: {argument}"
            )));
        }
    }
    Ok(Command::ReplayExport { output })
}

fn parse_mcp<I>(mut arguments: std::iter::Peekable<I>) -> Result<Command, CliError>
where
    I: Iterator<Item = String>,
{
    if arguments.next().as_deref() != Some("stdio") || arguments.next().is_some() {
        return Err(CliError::usage("mcp requires exactly the stdio subcommand"));
    }
    Ok(Command::McpStdio)
}

fn no_trailing<I>(
    mut arguments: std::iter::Peekable<I>,
    command: Command,
) -> Result<Command, CliError>
where
    I: Iterator<Item = String>,
{
    if let Some(argument) = arguments.next() {
        Err(CliError::usage(format!("unexpected argument: {argument}")))
    } else {
        Ok(command)
    }
}

fn next_value<I>(arguments: &mut std::iter::Peekable<I>, option: &str) -> Result<String, CliError>
where
    I: Iterator<Item = String>,
{
    arguments
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CliError::usage(format!("{option} requires a value")))
}

fn parse_u64(value: &str, option: &str) -> Result<u64, CliError> {
    value
        .parse()
        .map_err(|_| CliError::usage(format!("{option} must be an unsigned integer")))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Command, parse};

    #[test]
    fn parses_every_command_without_interpreting_action_rules() {
        assert_eq!(
            parse(["word-arena", "health"]).unwrap().command,
            Command::Health
        );
        assert_eq!(
            parse(["word-arena", "replay", "export", "--output", "game.json"])
                .unwrap()
                .command,
            Command::ReplayExport {
                output: Some("game.json".into())
            }
        );
        assert_eq!(
            parse([
                "word-arena",
                "--server",
                "http://localhost:3000",
                "--game-id",
                "game-1",
                "action",
                "--expected-version",
                "4",
                "--turn-id",
                "4",
                "--idempotency-key",
                "attempt-4",
                "--action-json",
                r#"{"type":"pass"}"#,
            ])
            .unwrap()
            .command,
            Command::Action {
                expected_version: 4,
                turn_id: 4,
                idempotency_key: "attempt-4".to_owned(),
                action: json!({"type":"pass"})
            }
        );
    }

    #[test]
    fn rejects_unknown_missing_and_malformed_arguments() {
        for arguments in [
            vec!["word-arena"],
            vec!["word-arena", "unknown"],
            vec!["word-arena", "health", "extra"],
            vec!["word-arena", "action", "--expected-version", "zero"],
            vec!["word-arena", "mcp", "http"],
        ] {
            assert!(parse(arguments).is_err());
        }
    }

    #[test]
    fn debug_output_always_redacts_flag_tokens() {
        let invocation = parse([
            "word-arena",
            "--token",
            "wa_cap_v1.private.secret",
            "health",
        ])
        .unwrap();
        let debug = format!("{invocation:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("wa_cap_v1.private.secret"));
    }
}
