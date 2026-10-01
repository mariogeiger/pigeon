//! The `pigeon` program: parses the command line and runs it.

use std::process::ExitCode;

fn main() -> ExitCode {
    let matches = pigeon::cli::command().get_matches();
    match pigeon::cli::run(&matches) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("pigeon: {error:#}");
            ExitCode::FAILURE
        }
    }
}
