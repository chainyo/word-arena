use std::process::ExitCode;

use word_arena_cli::{args, execute, write_output};

#[tokio::main]
async fn main() -> ExitCode {
    let invocation = match args::parse(std::env::args()) {
        Ok(invocation) => invocation,
        Err(error) => {
            eprintln!("word-arena-cli: {error}");
            eprintln!("run word-arena --help for usage");
            return error.exit_code();
        }
    };
    match execute(invocation).await.and_then(write_output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("word-arena-cli: {error}");
            error.exit_code()
        }
    }
}
