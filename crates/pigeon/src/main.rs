//! The `pigeon` program: completes the command line for the shell, or
//! parses it and runs it.

use std::process::ExitCode;

fn main() -> ExitCode {
    pigeon::complete::answer();
    let matches = pigeon::cli::command().get_matches();
    match pigeon::cli::run(&matches) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("pigeon: {error:#}");
            ExitCode::FAILURE
        }
    }
}
