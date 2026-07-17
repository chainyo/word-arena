use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Component, Path, PathBuf},
    process::{Command, Output},
};

use flate2::read::GzDecoder;

use crate::{
    BuilderError, EnglishPolicy,
    util::{diagnostic_tail, sha256_file},
};

const BUILD_LEVELS: [u8; 9] = [10, 20, 35, 40, 50, 55, 60, 70, 80];
const PORTABLE_SYMBOLIC_DEPENDENCY_HELPER: &str = r#"#!/bin/sh
set -eu

find l/ -type l -print \
  | LC_ALL=C sort \
  | while IFS= read -r path; do
      target=$(readlink "$path")
      dependency=$(printf '%s\n' "$target" | sed 's#^.*/r/#r/#')
      if [ "$dependency" != "$target" ]; then
        printf '%s: %s\n' "$path" "$dependency"
      fi
    done > .symbolic-deps
"#;

/// Prepared upstream `SCOWLv1` source tree and its generated classified lists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedScowl {
    source_root: PathBuf,
    final_directory: PathBuf,
}

impl PreparedScowl {
    /// Extracted upstream source root.
    #[must_use]
    pub fn source_root(&self) -> &Path {
        &self.source_root
    }

    /// Generated SCOWL `final/` directory consumed by the English importer.
    #[must_use]
    pub fn final_directory(&self) -> &Path {
        &self.final_directory
    }
}

/// Verifies, extracts, and runs the pinned upstream `SCOWLv1` source generator.
///
/// Upstream Make is deliberately sequential. On macOS the adapter requires
/// Homebrew GNU grep (`ggrep`) because the V1 scripts use GNU basic-regex
/// behavior. Symlink dependencies are generated portably instead of invoking
/// upstream's GNU `find -printf` script.
///
/// # Errors
///
/// Returns [`BuilderError`] for a mismatched archive, unsafe/unreadable
/// extraction, absent build tools, upstream command failure, or missing output.
pub fn prepare_scowl_archive(
    archive_path: &Path,
    work_directory: &Path,
    policy: &EnglishPolicy,
) -> Result<PreparedScowl, BuilderError> {
    policy.validate()?;
    #[cfg(not(unix))]
    return Err(BuilderError::UnsupportedSourceBuildPlatform);
    validate_archive(archive_path, policy)?;

    fs::create_dir_all(work_directory).map_err(|source| BuilderError::Io {
        path: work_directory.to_path_buf(),
        source,
    })?;
    let extraction_directory = work_directory.join("unpacked");
    if extraction_directory.exists() {
        return Err(BuilderError::OutputExists {
            path: extraction_directory,
        });
    }
    fs::create_dir(&extraction_directory).map_err(|source| BuilderError::Io {
        path: extraction_directory.clone(),
        source,
    })?;

    let archive_file = fs::File::open(archive_path).map_err(|source| BuilderError::Io {
        path: archive_path.to_path_buf(),
        source,
    })?;
    let decoder = GzDecoder::new(archive_file);
    let mut archive = tar::Archive::new(decoder);
    archive
        .unpack(&extraction_directory)
        .map_err(|source| BuilderError::Io {
            path: extraction_directory.clone(),
            source,
        })?;

    let source_root = extraction_directory.join(&policy.source_archive_root);
    if !source_root.join("scowl/Makefile").is_file() {
        return Err(BuilderError::MissingArchiveRoot {
            directory: extraction_directory,
            expected: policy.source_archive_root.clone(),
        });
    }

    let make = require_tool("make", "install a POSIX make implementation")?;
    for (tool, recovery) in [
        ("bash", "install Bash"),
        ("perl", "install Perl"),
        ("unzip", "install unzip"),
        ("tar", "install tar"),
        ("gunzip", "install gzip/gunzip"),
        ("g++", "install a C++ compiler"),
        ("find", "install a POSIX find implementation"),
        ("readlink", "install readlink"),
        ("sed", "install sed"),
        ("sort", "install sort"),
    ] {
        let _ = require_tool(tool, recovery)?;
    }
    let command_path = upstream_command_path(work_directory)?;

    let prep_args = [
        OsString::from("-C"),
        source_root.as_os_str().to_owned(),
        OsString::from("prep"),
    ];
    run_upstream(&make, &prep_args, &command_path)?;

    let scowl_root = source_root.join("scowl");
    // `prep` creates SCOWL's symlink directories. Generate this file afterward
    // so Make cannot consider it older than those directories and fall back to
    // upstream's GNU `find -printf` helper on BSD/macOS hosts.
    install_portable_symbolic_dependency_helper(&scowl_root)?;
    generate_symbolic_dependencies(&scowl_root)?;
    let mut build_args = vec![OsString::from("-C"), scowl_root.as_os_str().to_owned()];
    build_args.extend(
        BUILD_LEVELS
            .iter()
            .map(|level| OsString::from(format!("final/english-words.{level}"))),
    );
    run_upstream(&make, &build_args, &command_path)?;

    let final_directory = scowl_root.join("final");
    if !final_directory.is_dir() {
        return Err(BuilderError::MissingFinalDirectory {
            path: final_directory,
        });
    }
    for level in BUILD_LEVELS {
        let expected = final_directory.join(format!("english-words.{level}"));
        if !expected.is_file() {
            return Err(BuilderError::MissingFinalDirectory { path: expected });
        }
    }

    Ok(PreparedScowl {
        source_root,
        final_directory,
    })
}

fn validate_archive(archive_path: &Path, policy: &EnglishPolicy) -> Result<(), BuilderError> {
    let metadata = fs::metadata(archive_path).map_err(|source| BuilderError::Io {
        path: archive_path.to_path_buf(),
        source,
    })?;
    if metadata.len() != policy.source_archive_size_bytes {
        return Err(BuilderError::ArchiveSizeMismatch {
            path: archive_path.to_path_buf(),
            expected: policy.source_archive_size_bytes,
            actual: metadata.len(),
        });
    }
    let actual = sha256_file(archive_path)?;
    if actual != policy.source_archive_sha256 {
        return Err(BuilderError::ArchiveChecksumMismatch {
            path: archive_path.to_path_buf(),
            expected: policy.source_archive_sha256.clone(),
            actual,
        });
    }
    Ok(())
}

fn generate_symbolic_dependencies(scowl_root: &Path) -> Result<(), BuilderError> {
    let links_root = scowl_root.join("l");
    let mut symlink_paths = Vec::new();
    collect_symlinks(&links_root, &mut symlink_paths)?;
    symlink_paths.sort_unstable();

    let mut dependency_lines = Vec::new();
    for link in symlink_paths {
        let target = fs::read_link(&link).map_err(|source| BuilderError::Io {
            path: link.clone(),
            source,
        })?;
        let components = target
            .components()
            .filter_map(|component| match component {
                Component::Normal(value) => value.to_str(),
                _ => None,
            })
            .collect::<Vec<_>>();
        let Some(r_index) = components.iter().position(|component| *component == "r") else {
            continue;
        };
        let dependency = components[r_index..].join("/");
        let relative_link = portable_relative(scowl_root, &link)?;
        dependency_lines.push(format!("{relative_link}: {dependency}"));
    }
    dependency_lines.sort_unstable();
    let mut output = dependency_lines.join("\n");
    output.push('\n');
    let destination = scowl_root.join(".symbolic-deps");
    fs::write(&destination, output).map_err(|source| BuilderError::Io {
        path: destination,
        source,
    })
}

fn install_portable_symbolic_dependency_helper(scowl_root: &Path) -> Result<(), BuilderError> {
    let helper = scowl_root.join("src/make-symbolic-deps");
    fs::write(&helper, PORTABLE_SYMBOLIC_DEPENDENCY_HELPER).map_err(|source| BuilderError::Io {
        path: helper.clone(),
        source,
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).map_err(|source| {
            BuilderError::Io {
                path: helper,
                source,
            }
        })?;
    }

    Ok(())
}

fn collect_symlinks(directory: &Path, links: &mut Vec<PathBuf>) -> Result<(), BuilderError> {
    let entries = fs::read_dir(directory).map_err(|source| BuilderError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| BuilderError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| BuilderError::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_dir() {
            collect_symlinks(&path, links)?;
        } else if file_type.is_symlink() {
            links.push(path);
        }
    }
    Ok(())
}

fn portable_relative(root: &Path, path: &Path) -> Result<String, BuilderError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| BuilderError::UnexpectedInputFile {
            path: path.to_path_buf(),
        })?;
    let mut components = Vec::new();
    for component in relative.components() {
        let Component::Normal(value) = component else {
            return Err(BuilderError::UnexpectedInputFile {
                path: path.to_path_buf(),
            });
        };
        let value = value
            .to_str()
            .ok_or_else(|| BuilderError::UnexpectedInputFile {
                path: path.to_path_buf(),
            })?;
        components.push(value);
    }
    Ok(components.join("/"))
}

fn run_upstream(
    program: &Path,
    args: &[OsString],
    command_path: &OsStr,
) -> Result<(), BuilderError> {
    let output = Command::new(program)
        .args(args)
        .env("PATH", command_path)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .output()
        .map_err(|source| BuilderError::Io {
            path: program.to_path_buf(),
            source,
        })?;
    if output.status.success() {
        Ok(())
    } else {
        let command = format_command(program, args);
        let mut diagnostics = output.stdout;
        diagnostics.extend_from_slice(&output.stderr);
        Err(BuilderError::UpstreamBuildFailed {
            command,
            status: output.status,
            stderr: diagnostic_tail(&diagnostics),
        })
    }
}

fn format_command(program: &Path, args: &[OsString]) -> String {
    std::iter::once(program.as_os_str())
        .chain(args.iter().map(OsString::as_os_str))
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

fn require_tool(tool: &'static str, recovery: &'static str) -> Result<PathBuf, BuilderError> {
    find_executable(tool).ok_or(BuilderError::MissingTool { tool, recovery })
}

fn find_executable(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(unix)]
fn upstream_command_path(work_directory: &Path) -> Result<OsString, BuilderError> {
    let system_grep = find_executable("grep");
    if system_grep.as_deref().is_some_and(is_gnu_grep) {
        return env::var_os("PATH").ok_or(BuilderError::MissingTool {
            tool: "PATH",
            recovery: "set PATH to the required source-build tools",
        });
    }

    let gnu_grep_path = find_executable("ggrep").filter(|path| is_gnu_grep(path));
    let gnu_grep_path = gnu_grep_path.ok_or(BuilderError::MissingTool {
        tool: "GNU grep (`grep` or `ggrep`)",
        recovery: "on macOS run `brew install grep`; Linux distributions normally provide GNU grep",
    })?;
    let shim_directory = work_directory.join("tool-shims");
    fs::create_dir_all(&shim_directory).map_err(|source| BuilderError::Io {
        path: shim_directory.clone(),
        source,
    })?;
    let shim = shim_directory.join("grep");
    std::os::unix::fs::symlink(&gnu_grep_path, &shim)
        .map_err(|source| BuilderError::Io { path: shim, source })?;

    let current_path = env::var_os("PATH").ok_or(BuilderError::MissingTool {
        tool: "PATH",
        recovery: "set PATH to the required source-build tools",
    })?;
    env::join_paths(std::iter::once(shim_directory).chain(env::split_paths(&current_path))).map_err(
        |source| BuilderError::Io {
            path: work_directory.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, source),
        },
    )
}

#[cfg(not(unix))]
fn upstream_command_path(_work_directory: &Path) -> Result<OsString, BuilderError> {
    Err(BuilderError::UnsupportedSourceBuildPlatform)
}

fn is_gnu_grep(path: &Path) -> bool {
    Command::new(path)
        .arg("--version")
        .output()
        .is_ok_and(|output: Output| {
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains("GNU grep")
        })
}
