use std::{env, path::Path, process::ExitCode};

use word_arena_lexicon_builder::{
    EnglishPolicy, FrenchPolicy, apply_curation, build_english_from_archive,
    build_english_from_final, build_french_from_archive, build_french_from_xml, compile_index,
};

const USAGE: &str = "usage:\n  word-arena-lexicon-builder english-archive <scowl.tar.gz> <output-dir> <policy.toml>\n  word-arena-lexicon-builder english-final <scowl-final-dir> <output-dir> <policy.toml>\n  word-arena-lexicon-builder french-archive <morphalou.zip> <output-dir> <policy.toml>\n  word-arena-lexicon-builder french-xml <morphalou.xml> <output-dir> <policy.toml>\n  word-arena-lexicon-builder curation-apply <base-keys.txt> <output-dir> <curation-dir>\n  word-arena-lexicon-builder index-compile <curated-keys.txt> <lexicon.fst> <normalization-profile>";

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
    match command.as_str() {
        "english-archive" | "english-final" => {
            let policy =
                EnglishPolicy::load(Path::new(policy_path)).map_err(|error| error.to_string())?;
            let summary = if command == "english-archive" {
                build_english_from_archive(Path::new(input), Path::new(output), &policy)
            } else {
                build_english_from_final(Path::new(input), Path::new(output), &policy)
            }
            .map_err(|error| error.to_string())?;
            print_summary(
                &summary.output_directory,
                summary.report.source_rows,
                summary.report.accepted_rows,
                summary.report.rejected_rows,
                summary.report.unique_keys,
                &summary.metadata.keys_sha256,
            );
        }
        "french-archive" | "french-xml" => {
            let policy =
                FrenchPolicy::load(Path::new(policy_path)).map_err(|error| error.to_string())?;
            let summary = if command == "french-archive" {
                build_french_from_archive(Path::new(input), Path::new(output), &policy)
            } else {
                build_french_from_xml(Path::new(input), Path::new(output), &policy)
            }
            .map_err(|error| error.to_string())?;
            print_summary(
                &summary.output_directory,
                summary.report.source_rows,
                summary.report.accepted_rows,
                summary.report.rejected_rows,
                summary.report.unique_keys,
                &summary.metadata.keys_sha256,
            );
        }
        "curation-apply" => {
            let summary =
                apply_curation(Path::new(input), Path::new(output), Path::new(policy_path))
                    .map_err(|error| error.to_string())?;
            println!("output={}", summary.output_directory.display());
            println!("base_words={}", summary.report.base_word_count);
            println!("added_words={}", summary.report.added_word_count);
            println!("removed_words={}", summary.report.removed_word_count);
            println!("final_words={}", summary.report.final_word_count);
            println!("keys_sha256={}", summary.report.curated_keys_sha256);
        }
        "index-compile" => {
            let summary = compile_index(Path::new(input), Path::new(output), policy_path)
                .map_err(|error| error.to_string())?;
            println!("output={}", summary.output_path.display());
            println!("word_count={}", summary.word_count);
            println!("size_bytes={}", summary.size_bytes);
            println!("sha256={}", summary.sha256);
        }
        _ => return Err(USAGE.to_owned()),
    }
    Ok(())
}

fn print_summary(
    output_directory: &Path,
    source_rows: u64,
    accepted_rows: u64,
    rejected_rows: u64,
    unique_keys: u64,
    keys_sha256: &str,
) {
    println!("output={}", output_directory.display());
    println!("source_rows={source_rows}");
    println!("accepted_rows={accepted_rows}");
    println!("rejected_rows={rejected_rows}");
    println!("unique_keys={unique_keys}");
    println!("keys_sha256={keys_sha256}");
}
