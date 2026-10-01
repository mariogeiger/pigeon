//! `pigeon setup`: sets pigeon up on this machine step by step,
//! redrawing a checklist that each step marks from the machine's real
//! state, so that running it again resumes. It is a client of the daemon's
//! API like the command line, each step a command one may run alone:
//! start at login, run the daemon, keep, join or found a group, choose
//! what to follow, as a server follows everything, and open the web UI.

mod ask;
mod checklist;
mod group;
mod installation;
mod root;
mod tree;

use std::io::IsTerminal;

use anyhow::{Context, Result, bail};
use dialoguer::console::Term;
use pigeon_core::path::GroupPath;
use pigeon_core::statement::STATEMENTS;
use serde_json::{Map, Value, json};

use self::checklist::{Checklist, Mark};
use self::group::{Membership, Origin};
use self::tree::{File, Tree};
use crate::home::Home;
use crate::{api, client, render, service};

const RUST: &str = "Rust";
const BUILT: &str = "pigeon built";
const INSTALLED: &str = "Installed";
const SERVICE: &str = "Start at login";
const DAEMON: &str = "Daemon running";
const GROUP: &str = "Group";
const NAME: &str = "Name";
const ROOT: &str = "Root folder";
const FOLLOW: &str = "What to follow";
const UI: &str = "Web UI";

/// Calls `noun verb` with the object `args` on the daemon of `home`.
fn call(home: &Home, noun: &str, verb: &str, args: Value) -> Result<Value> {
    let args: Map<String, Value> = serde_json::from_value(args)?;
    client::call(home, noun, verb, &args)
}

/// Sets pigeon up on this machine, asking on this terminal.
///
/// # Errors
///
/// Fails without a terminal, or if the daemon cannot run.
pub fn run(home: &Home) -> Result<()> {
    let term = Term::stdout();
    if !term.is_term() || !std::io::stdin().is_terminal() {
        bail!("pigeon setup asks questions: run it in a terminal");
    }
    let mut list = Checklist::new(
        term,
        &[
            RUST, BUILT, INSTALLED, SERVICE, DAEMON, GROUP, NAME, ROOT, FOLLOW, UI,
        ],
    );
    installation(&mut list)?;
    start_at_login(home, &mut list)?;
    run_daemon(home, &mut list)?;
    let membership = group::step(home, &mut list)?;
    follow(home, &mut list, &membership)?;
    open_ui(home, &mut list)
}

/// Marks what the shell finds of pigeon's installation.
fn installation(list: &mut Checklist) -> Result<()> {
    match installation::cargo_version() {
        Some(version) => list.set(RUST, Mark::Done, version)?,
        None => list.set(RUST, Mark::Failed, "curl https://sh.rustup.rs -sSf | sh")?,
    }
    list.set(BUILT, Mark::Done, crate::VERSION)?;
    let running = std::env::current_exe().context("finding the running program")?;
    match installation::on_path() {
        Some(found) if installation::same_file(&found, &running) => {
            list.set(INSTALLED, Mark::Done, found.display().to_string())
        }
        Some(found) => list.set(
            INSTALLED,
            Mark::Failed,
            format!("the shell finds {} first", found.display()),
        ),
        None => list.set(
            INSTALLED,
            Mark::Failed,
            format!(
                "{} is not on the PATH: . \"$HOME/.cargo/env\"",
                running.parent().unwrap_or(&running).display()
            ),
        ),
    }
}

/// Describes when the service starts the daemon.
fn service_detail() -> &'static str {
    if service::lingers() {
        "at login and at boot"
    } else {
        "at login"
    }
}

/// Offers to start the daemon at login.
fn start_at_login(home: &Home, list: &mut Checklist) -> Result<()> {
    if service::enabled() {
        return list.set(SERVICE, Mark::Done, service_detail());
    }
    let term = list.term().clone();
    if client::answers(home) {
        term.write_line("A daemon started by hand is running: the service will replace it.")?;
    }
    if !ask::yes(&term, "Start pigeon automatically at login?", true)? {
        return list.set(SERVICE, Mark::Skipped, "pigeon service install");
    }
    install_service(home, list, false)
}

/// Installs the service, starting at boot too when `linger`.
fn install_service(home: &Home, list: &mut Checklist, linger: bool) -> Result<()> {
    list.set(SERVICE, Mark::Running, "")?;
    match service::install(home, linger) {
        Ok(()) => list.set(SERVICE, Mark::Done, service_detail()),
        Err(error) => list.set(SERVICE, Mark::Failed, format!("{error:#}")),
    }
}

/// Starts the daemon unless it answers.
fn run_daemon(home: &Home, list: &mut Checklist) -> Result<()> {
    if client::answers(home) {
        return list.set(DAEMON, Mark::Done, home.address()?.to_string());
    }
    list.set(DAEMON, Mark::Running, "")?;
    match service::start_detached(home) {
        Ok(log) => {
            list.note(format!(
                "The daemon runs until the next reboot; its log: {}",
                log.display()
            ));
            list.set(DAEMON, Mark::Done, home.address()?.to_string())
        }
        Err(error) => {
            list.set(DAEMON, Mark::Failed, format!("{error:#}"))?;
            Err(error)
        }
    }
}

/// Chooses what this machine follows: everything for a server, or what
/// one checks in the tree of the group's files.
fn follow(home: &Home, list: &mut Checklist, membership: &Membership) -> Result<()> {
    let term = list.term().clone();
    let group = &membership.group;
    if ask::yes(&term, "Is this machine an always-on server?", false)? {
        install_service(home, list, true)?;
        call(
            home,
            "selection",
            "follow",
            json!({ "group": group, "pattern": "*" }),
        )?;
        call(
            home,
            "retention",
            "set",
            json!({ "group": group, "everything": "on" }),
        )?;
        return list.set(FOLLOW, Mark::Done, "everything, with its history (server)");
    }
    let tag = format!("+{}", membership.member);
    let own = format!("{tag}/");
    match membership.origin {
        Origin::Founded | Origin::Joined { heard: false } => {
            return list.set(
                FOLLOW,
                Mark::Skipped,
                format!("{own}; the rest from the web UI"),
            );
        }
        Origin::Kept if !ask::yes(&term, "Change what this machine follows?", false)? => {
            return list.set(FOLLOW, Mark::Done, "unchanged");
        }
        _ => {}
    }
    let files = call(home, "file", "list", json!({ "group": group }))?;
    let files: Vec<File> = files
        .as_array()
        .into_iter()
        .flatten()
        .filter(|file| {
            GroupPath::parse(file["path"].as_str().unwrap_or_default())
                .is_ok_and(|path| !path.is_inside(STATEMENTS))
        })
        .map(|file| {
            let path = file["path"].as_str().unwrap_or_default().to_owned();
            let mine = path.split('/').rev().skip(1).any(|folder| folder == tag);
            File {
                size: file["content"]["size"].as_u64().unwrap_or(0),
                followed: file["cutoff"] == "PlusInfinity",
                held: file["held"].as_bool().unwrap_or(false),
                own: mine,
                path,
            }
        })
        .collect();
    if files.is_empty() {
        return list.set(FOLLOW, Mark::Skipped, format!("{own}; the group is empty"));
    }
    let tree = Tree::new(files);
    let Some(chosen) = tree.choose(&term)? else {
        return list.set(FOLLOW, Mark::Skipped, "unchanged");
    };
    let free = chosen.unchecked_here > 0
        && {
            let question = format!(
                "{} files you unchecked are on this machine ({}): keep them, frozen, or free the space?",
                chosen.unchecked_here,
                render::size(chosen.unchecked_bytes)
            );
            let choices = ["Keep a frozen copy".to_owned(), "Free the space".to_owned()];
            ask::choose(&term, &question, &choices)? == 1
        };
    for (pattern, follows) in &chosen.toggles {
        let args = if *follows {
            json!({ "group": group, "pattern": pattern })
        } else {
            json!({ "group": group, "pattern": pattern, "free": free })
        };
        let verb = if *follows { "follow" } else { "unfollow" };
        call(home, "selection", verb, args)?;
    }
    list.set(
        FOLLOW,
        Mark::Done,
        format!("{} to download", render::size(chosen.download)),
    )
}

/// Shows the link to the web UI, and opens it in a browser if one can.
fn open_ui(home: &Home, list: &mut Checklist) -> Result<()> {
    let link = api::open_link(home.address()?, &home.token()?);
    let opened = installation::open_in_browser(&link);
    list.note("To update pigeon later: pigeon update");
    let detail = if opened {
        format!("{link} (opened)")
    } else {
        link
    };
    list.set(UI, Mark::Done, detail)
}
