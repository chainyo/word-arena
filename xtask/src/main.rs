use std::{
    env,
    path::{Path, PathBuf},
    process::ExitCode,
};

use word_arena_lexicon::WordArenaPaths;
use xtask::{
    ArtifactBuildSummary, InstallStatus, PackInstaller, PackRegistry, XtaskError,
    build_from_source, package_release, run_setup,
};

const USAGE: &str = "usage:\n  cargo xtask setup [--offline]\n  cargo xtask lexicon list\n  cargo xtask lexicon inspect <pack-id>\n  cargo xtask lexicon verify [<pack-id>]\n  cargo xtask lexicon install <pack-id> [--offline]\n  cargo xtask lexicon remove <pack-id>\n  cargo xtask lexicon build --from-source [<pack-id>] --output <directory> [--release-materials] [--allow-registry-mismatch]\n  cargo xtask lexicon release-package --input <directory> --output <directory>";

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
        [group, command, pack_id] if group == "lexicon" && command == "inspect" => {
            inspect_pack(&installer, pack_id)?;
        }
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
                options.release_materials,
            )?;
            for summary in summaries {
                print_artifact(&summary);
            }
        }
        [group, command, input_flag, input, output_flag, output]
            if group == "lexicon"
                && command == "release-package"
                && input_flag == "--input"
                && output_flag == "--output" =>
        {
            let summary = package_release(&workspace_root, Path::new(input), Path::new(output))?;
            println!("release_tag={}", summary.tag);
            println!("release_directory={}", summary.output_directory.display());
            for asset in summary.assets {
                println!("release_asset={asset}");
            }
        }
        _ => return Err(XtaskError::Usage(USAGE.to_owned())),
    }
    Ok(())
}

fn inspect_pack(installer: &PackInstaller, pack_id: &str) -> Result<(), XtaskError> {
    let loaded = installer.load_installed(pack_id)?;
    let manifest = loaded.manifest();
    let root = installer.pack_path(installer.registry().require(pack_id)?);
    println!("pack_id={}", manifest.pack_id);
    println!("pack_version={}", manifest.pack_version);
    println!("content_sha256={}", manifest.content_sha256);
    println!("locale={}", manifest.locale);
    println!("word_count={}", manifest.word_count);
    println!("source_id={}", manifest.source.id);
    println!("source_revision={}", manifest.source.revision);
    println!("license_id={}", manifest.source.license_id);
    println!("license_file={}", root.join("LICENSE").display());
    println!("source_notice={}", root.join("SOURCE.md").display());
    println!(
        "third_party_notices={}",
        root.join("THIRD_PARTY_NOTICES").display()
    );
    Ok(())
}

struct SourceBuildOptions {
    pack_id: Option<String>,
    output: PathBuf,
    allow_registry_mismatch: bool,
    release_materials: bool,
}

fn parse_source_build_options(arguments: &[String]) -> Result<SourceBuildOptions, XtaskError> {
    let mut pack_id = None;
    let mut output = None;
    let mut allow_registry_mismatch = false;
    let mut release_materials = false;
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
            "--release-materials" if !release_materials => {
                release_materials = true;
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
        release_materials,
    })
}

fn print_artifact(summary: &ArtifactBuildSummary) {
    println!("pack={}", summary.pack_id);
    println!("content_sha256={}", summary.content_sha256);
    println!("artifact={}", summary.archive_path.display());
    println!("artifact_size_bytes={}", summary.archive_size_bytes);
    println!("artifact_sha256={}", summary.archive_sha256);
    println!("word_count={}", summary.word_count);
    for material in &summary.release_materials {
        println!("release_material={}", material.display());
    }
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
