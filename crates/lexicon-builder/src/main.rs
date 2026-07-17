use std::{env, path::Path, process::ExitCode};

use word_arena_lexicon_builder::{
    EnglishPolicy, build_english_from_archive, build_english_from_final,
};

const USAGE: &str = "usage:\n  word-arena-lexicon-builder english-archive <scowl.tar.gz> <output-dir> <policy.toml>\n  word-arena-lexicon-builder english-final <scowl-final-dir> <output-dir> <policy.toml>";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    let [command, input, output, policy_path] = arguments.as_slice() else {
        return Err(USAGE.to_owned());
    };
    let policy = EnglishPolicy::load(Path::new(policy_path)).map_err(|error| error.to_string())?;
    let summary = match command.as_str() {
        "english-archive" => {
            build_english_from_archive(Path::new(input), Path::new(output), &policy)
        }
        "english-final" => build_english_from_final(Path::new(input), Path::new(output), &policy),
        _ => return Err(USAGE.to_owned()),
    }
    .map_err(|error| error.to_string())?;

    println!("output={}", summary.output_directory.display());
    println!("source_rows={}", summary.report.source_rows);
    println!("accepted_rows={}", summary.report.accepted_rows);
    println!("rejected_rows={}", summary.report.rejected_rows);
    println!("unique_keys={}", summary.report.unique_keys);
    println!("keys_sha256={}", summary.metadata.keys_sha256);
    Ok(())
}
