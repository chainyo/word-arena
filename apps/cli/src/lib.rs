pub mod args;
pub mod bridge;
pub mod client;
pub mod config;
pub mod error;

use std::{fs::OpenOptions, io::Write as _, path::Path};

use args::{Command, Invocation};
use client::RemoteClient;
use config::ResolvedConfig;
use error::CliError;
use serde_json::Value;

#[derive(Debug)]
pub enum CommandOutput {
    Json {
        value: Value,
        output: Option<std::path::PathBuf>,
    },
    Help,
    None,
}

/// Executes one parsed command and returns its explicit output disposition.
///
/// # Errors
///
/// Returns configuration, authentication, remote, protocol, I/O, or interruption errors.
pub async fn execute(invocation: Invocation) -> Result<CommandOutput, CliError> {
    if invocation.command == Command::Help {
        return Ok(CommandOutput::Help);
    }
    let config = ResolvedConfig::load(invocation.overrides, invocation.config_path)?;
    let client = RemoteClient::new(config)?;
    match invocation.command {
        Command::Health => Ok(json_output(client.health().await?)),
        Command::Auth => Ok(json_output(client.authenticate().await?)),
        Command::Observe => Ok(json_output(client.observe().await?)),
        Command::Action {
            expected_version,
            turn_id,
            idempotency_key,
            action,
        } => Ok(json_output(
            client
                .action(expected_version, turn_id, idempotency_key, action)
                .await?,
        )),
        Command::ReplayExport { output } => Ok(CommandOutput::Json {
            value: client.replay_export().await?,
            output,
        }),
        Command::McpStdio => {
            bridge::run_stdio(client).await?;
            Ok(CommandOutput::None)
        }
        Command::Help => Ok(CommandOutput::Help),
    }
}

/// Writes command output to stdout or a newly created permission-restricted file.
///
/// # Errors
///
/// Returns an I/O error when output cannot be serialized, created, or flushed.
pub fn write_output(output: CommandOutput) -> Result<(), CliError> {
    match output {
        CommandOutput::Json {
            value,
            output: Some(path),
        } => write_private_json(&path, &value),
        CommandOutput::Json {
            value,
            output: None,
        } => {
            let stdout = std::io::stdout();
            let mut stdout = stdout.lock();
            serde_json::to_writer_pretty(&mut stdout, &value)
                .map_err(|error| CliError::Io(error.to_string()))?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
            Ok(())
        }
        CommandOutput::Help => {
            print!("{}", args::HELP);
            std::io::stdout().flush()?;
            Ok(())
        }
        CommandOutput::None => Ok(()),
    }
}

fn json_output(value: Value) -> CommandOutput {
    CommandOutput::Json {
        value,
        output: None,
    }
}

fn write_private_json(path: &Path, value: &Value) -> Result<(), CliError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| CliError::Io(format!("failed to create {}: {error}", path.display())))?;
    serde_json::to_writer_pretty(&mut file, value)
        .map_err(|error| CliError::Io(error.to_string()))?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}
