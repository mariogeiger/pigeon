//! Names the commit pigeon is built from, for `--version` and the daemon's
//! first line: the checkout's `HEAD`, or `unknown` outside a repository.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    let commit = git(&["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=PIGEON_COMMIT={commit}");
    let branch = git(&["symbolic-ref", "-q", "HEAD"]);
    let watched = ["HEAD", "packed-refs"]
        .map(str::to_owned)
        .into_iter()
        .chain(branch);
    for name in watched {
        if let Some(path) = git(&["rev-parse", "--path-format=absolute", "--git-path", &name]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}
