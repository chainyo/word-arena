use std::{
    env,
    path::{Path, PathBuf},
    process::ExitCode,
};

use word_arena_lexicon::WordArenaPaths;
use xtask::{
    ArtifactBuildSummary, InstallStatus, PackInstaller, PackRegistry, XtaskError,
    build_from_source, run_setup,
};

const USAGE: &str = "usage:\n  cargo xtask setup [--offline]\n  cargo xtask lexicon list\n  cargo xtask lexicon verify [<pack-id>]\n  cargo xtask lexicon install <pack-id> [--offline]\n  cargo xtask lexicon remove <pack-id>\n  cargo xtask lexicon build --from-source [<pack-id>] --output <directory> [--allow-registry-mismatch]";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), XtaskError> {
    let workspace_root = workspace_root();
    let registry_path = env::var_os("WORD_ARENA_PACK_REGISTRY").map_or_else(
        || workspace_root.join("lexicons/registry.toml"),
        PathBuf::from,
    );
    let registry = PackRegistry::load(&registry_path)?;
    let installer = PackInstaller::new(registry, WordArenaPaths::discover()?);
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    match arguments.as_slice() {
        [command] if command == "setup" => {
            print_setup(run_setup(&installer, &workspace_root, false)?);
        }
        [command, offline] if command == "setup" && offline == "--offline" => {
            print_setup(run_setup(&installer, &workspace_root, true)?);
        }
        [group, command] if group == "lexicon" && command == "list" => list_packs(&installer),
        [group, command] if group == "lexicon" && command == "verify" => {
            verify_packs(&installer, None)?;
        }
        [group, command, pack_id] if group == "lexicon" && command == "verify" => {
            verify_packs(&installer, Some(pack_id))?;
        }
        [group, command, pack_id] if group == "lexicon" && command == "install" => {
            print_install(pack_id, installer.install(pack_id, false)?);
        }
        [group, command, pack_id, offline]
            if group == "lexicon" && command == "install" && offline == "--offline" =>
        {
            print_install(pack_id, installer.install(pack_id, true)?);
        }
        [group, command, pack_id] if group == "lexicon" && command == "remove" => {
            let trash = installer.remove(pack_id)?;
            println!("removed={pack_id}");
            println!("recoverable_at={}", trash.display());
        }
        [group, command, from_source, rest @ ..]
            if group == "lexicon" && command == "build" && from_source == "--from-source" =>
        {
            let options = parse_source_build_options(rest)?;
            let summaries = build_from_source(
                &workspace_root,
                installer.registry(),
                options.pack_id.as_deref(),
                &options.output,
                options.allow_registry_mismatch,
            )?;
            for summary in summaries {
                print_artifact(&summary);
            }
        }
        _ => return Err(XtaskError::Usage(USAGE.to_owned())),
    }
    Ok(())
}

struct SourceBuildOptions {
    pack_id: Option<String>,
    output: PathBuf,
    allow_registry_mismatch: bool,
}

fn parse_source_build_options(arguments: &[String]) -> Result<SourceBuildOptions, XtaskError> {
    let mut pack_id = None;
    let mut output = None;
    let mut allow_registry_mismatch = false;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--output" if output.is_none() => {
                index += 1;
                output = arguments.get(index).map(PathBuf::from);
                if output.is_none() {
                    return Err(XtaskError::Usage(USAGE.to_owned()));
                }
            }
            "--allow-registry-mismatch" if !allow_registry_mismatch => {
                allow_registry_mismatch = true;
            }
            value if !value.starts_with('-') && pack_id.is_none() => {
                pack_id = Some(value.to_owned());
            }
            _ => return Err(XtaskError::Usage(USAGE.to_owned())),
        }
        index += 1;
    }
    let output = output.ok_or_else(|| XtaskError::Usage(USAGE.to_owned()))?;
    Ok(SourceBuildOptions {
        pack_id,
        output,
        allow_registry_mismatch,
    })
}

fn print_artifact(summary: &ArtifactBuildSummary) {
    println!("pack={}", summary.pack_id);
    println!("content_sha256={}", summary.content_sha256);
    println!("artifact={}", summary.archive_path.display());
    println!("artifact_size_bytes={}", summary.archive_size_bytes);
    println!("artifact_sha256={}", summary.archive_sha256);
    println!("word_count={}", summary.word_count);
}

fn list_packs(installer: &PackInstaller) {
    println!("data_dir={}", installer.paths().data_dir().display());
    for record in &installer.registry().packs {
        let path = installer.pack_path(record);
        let state = if path.exists() {
            "installed"
        } else {
            "missing"
        };
        println!(
            "{}@{} {} {}",
            record.pack_id,
            record.pack_version,
            state,
            path.display()
        );
    }
}

fn verify_packs(installer: &PackInstaller, selected: Option<&str>) -> Result<(), XtaskError> {
    if let Some(pack_id) = selected {
        installer.verify(pack_id)?;
        println!("verified={pack_id}");
        return Ok(());
    }
    for record in &installer.registry().packs {
        installer.verify(&record.pack_id)?;
        println!("verified={}", record.pack_id);
    }
    Ok(())
}

fn print_setup(report: xtask::SetupReport) {
    for (pack_id, status) in report.packs {
        print_install(&pack_id, status);
    }
}

fn print_install(pack_id: &str, status: InstallStatus) {
    let state = match status {
        InstallStatus::Installed => "installed",
        InstallStatus::AlreadyInstalled => "already_installed",
    };
    println!("pack={pack_id} status={state}");
}

fn workspace_root() -> PathBuf {
    env::var_os("WORD_ARENA_WORKSPACE_ROOT").map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("xtask is directly beneath the workspace root")
                .to_path_buf()
        },
        PathBuf::from,
    )
}
