use std::{
    path::Path,
    process::{Command, Stdio},
};

use crate::{InstallStatus, PackInstaller, XtaskError, verify_tool};

/// Result for one canonical first-time setup run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetupReport {
    /// Pack IDs and whether each was newly installed or already present.
    pub packs: Vec<(String, InstallStatus)>,
}

/// Installs pinned web dependencies and both required lexicon packs.
///
/// Offline mode passes Bun its strict offline flag and permits the installer to
/// use only already installed packs or checksum-verified cached archives.
///
/// # Errors
///
/// Returns [`XtaskError`] for missing tools, dependency installation failure,
/// or any preflight/download/pack validation failure.
pub fn run_setup(
    installer: &PackInstaller,
    workspace_root: &Path,
    offline: bool,
) -> Result<SetupReport, XtaskError> {
    verify_tool(
        "bun",
        "install the Bun version pinned by .bun-version (https://bun.sh)",
    )?;
    if !offline {
        verify_tool(
            "curl",
            "install curl or rerun --offline after artifacts are cached",
        )?;
    }
    install_web_dependencies(workspace_root, offline)?;
    let packs = installer.install_many(&["word-arena-en-world-v1", "word-arena-fr-v1"], offline)?;
    Ok(SetupReport { packs })
}

fn install_web_dependencies(workspace_root: &Path, offline: bool) -> Result<(), XtaskError> {
    let mut command = Command::new("bun");
    command
        .current_dir(workspace_root)
        .args(["install", "--cwd", "web", "--frozen-lockfile"])
        .stdin(Stdio::null());
    if offline {
        command.arg("--offline");
    }
    let output = command.output().map_err(|source| XtaskError::Io {
        path: workspace_root.join("web"),
        source,
    })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(XtaskError::ToolFailed {
            command: if offline {
                "bun install --cwd web --frozen-lockfile --offline".to_owned()
            } else {
                "bun install --cwd web --frozen-lockfile".to_owned()
            },
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}
